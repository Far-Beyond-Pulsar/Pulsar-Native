//! The player's edits and the brushes that make them: a dig takes a ball out
//! of whatever is there; a build lays a cube of a block on the block
//! lattice (`BLOCK_M` cubes of voxels, aligned on the face's cells). Edits
//! are applied in order, later ones over earlier ones.

use crate::blocks::Block;
use crate::config::{BLOCK_M, DIG_RADIUS_M};
use glam::DVec3;
use helio_pass_voxel_planet::{Brush, BrushOp, BrushShape, Cell, Grid};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edit {
    /// A ball of `DIG_RADIUS_M` around a planet-centred point taken out.
    Dig(DVec3),
    /// The lattice cube holding `cell` filled with a block.
    Build(Cell, Block),
}

/// Cells a built block's side.
pub fn block_cells(grid: &Grid) -> i32 {
    (BLOCK_M / grid.voxel_size()).round().max(1.0) as i32
}

/// The lattice cube holding `cell`: its first cell.
pub fn block_origin(grid: &Grid, cell: Cell) -> Cell {
    let n = block_cells(grid);
    Cell::new(
        cell.face,
        cell.i.div_euclid(n) * n,
        cell.j.div_euclid(n) * n,
        cell.k.div_euclid(n) * n,
    )
}

impl Edit {
    /// The brush that makes this edit.
    pub fn brush(&self, grid: &Grid) -> Brush {
        match *self {
            Edit::Dig(centre) => Brush {
                center: centre.to_array(),
                radius: DIG_RADIUS_M,
                shape: BrushShape::Sphere,
                op: BrushOp::Remove,
                material: 0,
                height: 0.0,
            },
            Edit::Build(cell, block) => {
                let o = block_origin(grid, cell);
                let n = f64::from(block_cells(grid));
                let centre = grid.position(
                    o.face,
                    [
                        f64::from(o.i) + n * 0.5,
                        f64::from(o.j) + n * 0.5,
                        f64::from(o.k) + n * 0.5,
                    ],
                );
                let half = n * grid.voxel_size() * 0.5;
                Brush {
                    center: centre.to_array(),
                    radius: half,
                    shape: BrushShape::Cube,
                    op: BrushOp::Add,
                    material: block.material(),
                    height: half,
                }
            }
        }
    }

    /// The cells the edit makes (`None`: air).
    pub fn cells(&self, grid: &Grid) -> Vec<(Cell, Option<Block>)> {
        match *self {
            Edit::Dig(centre) => {
                let (at, _) = grid.locate(centre);
                let r = (DIG_RADIUS_M / grid.voxel_size()).ceil() as i32;
                let mut out = Vec::new();
                for k in -r..=r {
                    for j in -r..=r {
                        for i in -r..=r {
                            let cell = Cell::new(at.face, at.i + i, at.j + j, at.k + k);
                            if grid.cell_center(cell).distance(centre) <= DIG_RADIUS_M {
                                out.push((cell, None));
                            }
                        }
                    }
                }
                out
            }
            Edit::Build(cell, block) => {
                let o = block_origin(grid, cell);
                let n = block_cells(grid);
                (0..n * n * n)
                    .map(|c| {
                        (
                            Cell::new(o.face, o.i + c % n, o.j + c / n % n, o.k + c / (n * n)),
                            Some(block),
                        )
                    })
                    .collect()
            }
        }
    }
}
