//! What a brush does to a world's cells, counted from the cells it bakes
//! into: what gameplay reports as blocks broken, placed and recoloured.
//!
//! A brush is counted in the cells of one level, as baking holds them
//! (`EditStore::held`): base cells for one at most [`COUNT_RADIUS`] cells in
//! radius, else the first level where it spans that many, so counting
//! costs at most about 17 thousand cells a face whatever its size. Each
//! cell stands for its `8^level` base cells, with the material of the base
//! cell at its centre before the brush, and the brush's effect on it.
//! Brushes up to [`EXACT_CELLS`] changed base cells report each cell;
//! larger ones, each brick's cells by material.
use crate::edit_store::EditStore;
use crate::edits::{Brush, BrushOp};
use crate::grid::{Cell, BRICK};
use crate::planet::Planet;
use crate::terrain::material;
use glam::DVec3;

/// Changed base cells a brush reports one by one, at most.
pub const EXACT_CELLS: u64 = 4096;
/// Radius (cells of the level counted) a brush is counted at, at most.
pub const COUNT_RADIUS: u32 = 16;

/// Cells that changed from one material to another (0: air).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellChange {
    /// Planet-centred: the cell's centre, or for a region its brick's.
    pub centre: DVec3,
    pub before: u32,
    pub after: u32,
    /// Base cells it stands for (1: one cell).
    pub count: u64,
}

/// A brush's effect on the world it was applied to ([`Planet::edit_yield`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditYield {
    /// The level whose cells were counted (0: base cells, exact).
    pub level: u32,
    pub changes: Vec<CellChange>,
}

impl EditYield {
    /// Base cells changed.
    pub fn cells(&self) -> u64 {
        self.changes.iter().map(|c| c.count).sum()
    }
    /// Base cells of each material broken (solid before, air after).
    pub fn broken(&self) -> [u64; crate::terrain::MATERIALS] {
        self.by_material(|c| {
            (c.before != material::AIR && c.after == material::AIR).then_some(c.before)
        })
    }
    /// Base cells of each material placed (air before, solid after).
    pub fn placed(&self) -> [u64; crate::terrain::MATERIALS] {
        self.by_material(|c| {
            (c.before == material::AIR && c.after != material::AIR).then_some(c.after)
        })
    }
    fn by_material(
        &self,
        of: impl Fn(&CellChange) -> Option<u32>,
    ) -> [u64; crate::terrain::MATERIALS] {
        let mut out = [0; crate::terrain::MATERIALS];
        for c in &self.changes {
            if let Some(m) = of(c) {
                out[(m as usize).min(out.len() - 1)] += c.count;
            }
        }
        out
    }
}

impl std::fmt::Display for EditYield {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} cells changed (level {}, {} changes), {} broken, {} placed",
            self.cells(),
            self.level,
            self.changes.len(),
            self.broken().iter().sum::<u64>(),
            self.placed().iter().sum::<u64>()
        )
    }
}

/// A cell's material after `op` with `material` over `before` (0: air).
fn after(op: BrushOp, material: u32, before: u32) -> u32 {
    match op {
        BrushOp::Remove => material::AIR,
        BrushOp::Add => material,
        BrushOp::Paint if before != material::AIR => material,
        BrushOp::Paint => material::AIR,
    }
}

impl Planet {
    /// What applying `brush` to this world changes (see the module doc).
    /// Call it before applying the brush.
    pub fn edit_yield(&self, brush: &Brush) -> Result<EditYield, String> {
        let faces = brush.resolve(self.grid())?;
        // The first level whose cells it spans at most `COUNT_RADIUS` of.
        let radius_cells = faces.iter().map(|fb| fb.radius_half / 2).max().unwrap_or(0);
        let level = (0..31)
            .find(|&l| (radius_cells >> l) <= COUNT_RADIUS)
            .unwrap_or(31);
        self.edit_yield_at(brush, level)
    }

    /// [`Self::edit_yield`] counted in the cells of `level`.
    fn edit_yield_at(&self, brush: &Brush, level: u32) -> Result<EditYield, String> {
        use rayon::prelude::*;
        let grid = *self.grid();
        let faces = brush.resolve(&grid)?;
        let weight = 1u64 << (3 * level);
        let half = (1i32 << level) >> 1;
        let mut changes = Vec::new();
        for fb in &faces {
            let face = fb.face();
            let held = EditStore::held(&grid, fb, level);
            // Each brick's changes by (before, after), with its changed
            // cells while they are few enough to list.
            let bricks: Vec<Vec<(u32, u32, u64, Vec<Cell>)>> = held
                .par_iter()
                .map(|(key, mask)| {
                    let mut out: Vec<(u32, u32, u64, Vec<Cell>)> = Vec::new();
                    for (z, &bits) in mask.iter().enumerate() {
                        let mut bits = bits;
                        while bits != 0 {
                            let bit = bits.trailing_zeros() as i32;
                            bits &= bits - 1;
                            let (x, y) = (bit % BRICK, bit / BRICK);
                            let at = |b: i32, c: i32| ((b * BRICK + c) << level) + half;
                            let cell =
                                Cell::new(face, at(key.bi, x), at(key.bj, y), at(key.bk, z as i32));
                            let was = self.material(cell);
                            let now = after(brush.op, fb.material(), was);
                            if was == now {
                                continue;
                            }
                            match out.iter_mut().find(|c| c.0 == was && c.1 == now) {
                                Some(c) => {
                                    c.2 += weight;
                                    if level == 0 && c.3.len() < EXACT_CELLS as usize {
                                        c.3.push(cell);
                                    }
                                }
                                None => out.push((was, now, weight, vec![cell])),
                            }
                        }
                    }
                    out
                })
                .collect();
            let total: u64 = bricks.iter().flatten().map(|c| c.2).sum();
            let exact = level == 0 && total <= EXACT_CELLS;
            for (n, brick) in bricks.into_iter().enumerate() {
                let key = held[n].0;
                let centre = || {
                    let mid = |b: i32| f64::from((b * BRICK + BRICK / 2) << level);
                    grid.position(face, [mid(key.bi), mid(key.bj), mid(key.bk)])
                };
                for (before, after, count, cells) in brick {
                    if exact {
                        changes.extend(cells.into_iter().map(|cell| CellChange {
                            centre: grid.cell_center(cell),
                            before,
                            after,
                            count: 1,
                        }));
                    } else {
                        changes.push(CellChange {
                            centre: centre(),
                            before,
                            after,
                            count,
                        });
                    }
                }
            }
        }
        Ok(EditYield { level, changes })
    }

    /// Apply `brush` (as [`Planet::apply`]) and say what it changed.
    pub fn apply_counted(&mut self, brush: Brush) -> Result<(u32, EditYield), String> {
        let counted = self.edit_yield(&brush)?;
        Ok((self.apply(brush)?, counted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrushShape, PlanetRecipe};

    /// A small brush's yield is every cell it changes, exactly: each cell's
    /// material before and after, as the world answers.
    #[test]
    fn small_yields_are_the_cells_changed() {
        let mut planet = Planet::new(PlanetRecipe {
            shape: crate::grid::Shape::Plane,
            plane_size_m: 512.0,
            ..Default::default()
        })
        .unwrap();
        let ground = planet.surface_point(DVec3::ZERO, 0.0);
        let grid = *planet.grid();
        for (n, (op, radius)) in [
            (BrushOp::Remove, 0.6),
            (BrushOp::Add, 0.45),
            (BrushOp::Paint, 0.7),
            (BrushOp::Remove, 0.35),
        ]
        .into_iter()
        .enumerate()
        {
            let brush = Brush {
                center: (ground + DVec3::new(0.13 * n as f64, 0.2 - 0.1 * n as f64, 0.05))
                    .to_array(),
                radius,
                shape: if n % 2 == 0 {
                    BrushShape::Sphere
                } else {
                    BrushShape::Cube
                },
                op,
                material: 5 + n as u32,
                height: 0.0,
            };
            let before = planet.clone();
            let (_, counted) = planet.apply_counted(brush).unwrap();
            assert_eq!(counted.level, 0);
            let mut expected = Vec::new();
            let (c, _) = grid.locate(DVec3::from_array(brush.center));
            for k in -10..=10 {
                for j in -10..=10 {
                    for i in -10..=10 {
                        let cell = Cell::new(c.face, c.i + i, c.j + j, c.k + k);
                        let (was, now) = (before.material(cell), planet.material(cell));
                        if was != now {
                            expected.push((cell, was, now));
                        }
                    }
                }
            }
            let mut got: Vec<(Cell, u32, u32)> = counted
                .changes
                .iter()
                .map(|ch| (grid.locate(ch.centre).0, ch.before, ch.after))
                .collect();
            got.sort_by_key(|g| (g.0.k, g.0.j, g.0.i));
            expected.sort_by_key(|g| (g.0.k, g.0.j, g.0.i));
            assert!(!expected.is_empty());
            assert_eq!(got, expected, "brush {n}");
        }
    }

    /// A large brush's yield counts its cells at a coarse level: within a
    /// few percent of the base cells it changes.
    #[test]
    fn large_yields_count_their_cells() {
        let planet = Planet::new(PlanetRecipe {
            shape: crate::grid::Shape::Plane,
            plane_size_m: 2048.0,
            ..Default::default()
        })
        .unwrap();
        let ground = planet.surface_point(DVec3::ZERO, 0.0);
        let radius = 8.0;
        // Buried wholly: every cell it holds was solid.
        let brush = Brush {
            center: (ground - DVec3::Y * 20.0).to_array(),
            radius,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        };
        let counted = planet.edit_yield(&brush).unwrap();
        assert!(counted.level > 0);
        let exact: u64 = planet
            .edit_yield_at(&brush, 0)
            .unwrap()
            .broken()
            .iter()
            .sum();
        let broken: u64 = counted.broken().iter().sum();
        assert!(
            (broken as f64 - exact as f64).abs() < exact as f64 * 0.05,
            "{broken} broken against {exact} base cells"
        );
        assert_eq!(counted.placed().iter().sum::<u64>(), 0);
    }
}
