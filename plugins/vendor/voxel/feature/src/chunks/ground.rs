//! The voxel planet as the ground foliage grows on (`helio_pass_foliage_place`'s
//! `contract::ground`).
//!
//! - **Chart**: the cube face under the eye (a plane's one face), tiled in its cell
//!   coordinates: a tile is `FOLIAGE_TILE_SIZE_METERS / voxel` cells a side (8 m at
//!   0.1 m), its frame the face's datum between its corners and its centre's up (radial
//!   on a sphere). The face keeps its chart until the eye is well past its edge, so a
//!   walk along an edge does not place every tile again and again.
//! - **Answers**: [`ChunkWorld::probe_ground`] walks each query's segment down through
//!   the drawn chunks (`ground_probe` in chunk_draw.wgsl): where it meets a top face, the
//!   hit's distance, the face's normal and its cell's material (the ground id foliage
//!   types are keyed by). Walls, rock and nothing drawn are no ground.
//! - **Changes**: every node whose drawn chunk changes (generated, dug, replaced by finer
//!   or coarser ones, and again when that switch's fade ends) is a world box whose tiles
//!   are placed again ([`ChunkWorld::take_ground_changes`]).
//!
//! [`ChunkWorld::probe_ground`]: super::ChunkWorld::probe_ground
//! [`ChunkWorld::take_ground_changes`]: super::ChunkWorld::take_ground_changes

use crate::grid::Grid;
use crate::volume::VolumeTransform;
use bytemuck::{Pod, Zeroable};
use glam::DVec3;
use helio_foliage_api::{GroundChart, GroundGeometry, TileFrame, FOLIAGE_TILE_SIZE_METERS};
use std::sync::{Mutex, RwLock};

/// Share of a face past its edges the eye may wander before the chart moves to the face
/// it is over.
const CHART_HYSTERESIS: f64 = 1.0 / 16.0;

/// The primary volume's tiling for foliage (in the world: its grid placed
/// by its transform). Its grid is set once the volume is known
/// ([`Self::set_volume`]); until then it has no chart.
#[derive(Default)]
pub struct PlanetGround {
    grid: RwLock<Option<(Grid, VolumeTransform)>>,
    face: Mutex<Option<u8>>,
    /// The scene has no voxel volume ([`Self::set_absent`]): foliage grows on its
    /// built-in ground meanwhile.
    absent: std::sync::atomic::AtomicBool,
}

impl PlanetGround {
    /// Foliage grows on `grid` placed by `transform`.
    pub fn set_volume(&self, grid: Grid, transform: VolumeTransform) {
        let mut current = self.grid.write().expect("planet ground");
        if current.map(|(g, _)| g) != Some(grid) {
            *self.face.lock().expect("planet ground") = None;
        }
        *current = Some((grid, transform));
    }

    /// Whether the scene has no voxel volume (the pass says each frame).
    pub fn set_absent(&self, absent: bool) {
        self.absent
            .store(absent, std::sync::atomic::Ordering::Relaxed);
    }

    /// Foliage grows on `grid` in the world's frame.
    pub fn set_grid(&self, grid: Grid) {
        self.set_volume(grid, VolumeTransform::IDENTITY);
    }

    fn grid(&self) -> Option<(Grid, VolumeTransform)> {
        *self.grid.read().expect("planet ground")
    }

    /// Cells a tile spans along each axis.
    fn tile_cells(grid: &Grid) -> f64 {
        f64::from(FOLIAGE_TILE_SIZE_METERS) / grid.voxel_size()
    }
}

impl GroundGeometry for PlanetGround {
    fn present(&self) -> bool {
        !self.absent.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn chart(&self, eye: DVec3) -> Option<GroundChart> {
        let (grid, transform) = self.grid()?;
        let eye = transform.to_local(eye);
        let n = f64::from(grid.cells());
        let mut face = self.face.lock().expect("planet ground");
        let within = |f: u8| {
            grid.face_coords(f, eye).is_some_and(|c| {
                (0..2).all(|a| c[a] > -CHART_HYSTERESIS * n && c[a] < (1.0 + CHART_HYSTERESIS) * n)
            })
        };
        let f = match *face {
            Some(f) if grid.is_plane() || within(f) => f,
            _ => grid.locate(eye).0.face,
        };
        *face = Some(f);
        let c = grid.face_coords(f, eye)?;
        let s = grid.voxel_size();
        Some(GroundChart {
            id: u64::from(f),
            eye: [c[0] * s, c[1] * s],
        })
    }

    fn tile_frame(&self, chart: u64, coord: [i32; 2], span: u32) -> Option<TileFrame> {
        let (grid, transform) = self.grid()?;
        let face = u8::try_from(chart).ok()?;
        let base = Self::tile_cells(&grid);
        let n = base * f64::from(span.max(1));
        let (i0, j0) = (f64::from(coord[0]) * base, f64::from(coord[1]) * base);
        let at = |i: f64, j: f64| grid.position(face, [i, j, 0.0]);
        let origin = at(i0, j0);
        Some(TileFrame {
            origin: transform.to_world(origin),
            axis_u: transform.dir_to_world(at(i0 + n, j0) - origin),
            axis_v: transform.dir_to_world(at(i0, j0 + n) - origin),
            up: transform.dir_to_world(grid.up(at(i0 + n * 0.5, j0 + n * 0.5))),
        })
    }

    fn locate(&self, chart: u64, p: DVec3) -> Option<[f64; 2]> {
        let (grid, transform) = self.grid()?;
        let c = grid.face_coords(u8::try_from(chart).ok()?, transform.to_local(p))?;
        let s = grid.voxel_size();
        Some([c[0] * s, c[1] * s])
    }
}

/// `ProbeTile` of chunk_draw.wgsl: where an asked block's queries start, located on the
/// CPU in f64 (as GI probes are): the base cell of a reference point (the block's centre
/// at its queries' start height), the point's continuous coordinates in it, the point
/// eye-relative (`w`: the block's first query, as bits), and the face coordinates'
/// derivatives by position, which place each query's start (metres from it) in its own
/// cell. Blocks are at most 16 m across, so the derivatives hold to millimetres.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct ProbeTile {
    pub cell: [i32; 4],
    pub frac: [f32; 4],
    pub at: [f32; 4],
    pub jacobian: [[f32; 4]; 3],
}

impl ProbeTile {
    /// The tile `frame` whose queries start `start_height` along its up, walked from
    /// `walk_height` (under it: where drawn chunks are near; `frac.w` holds the drop), for
    /// an eye at world `eye`. `None` where the grid has no cell there.
    pub fn new(
        grid: &Grid,
        frame: &TileFrame,
        start_height: f64,
        walk_height: f64,
        eye: DVec3,
        first: u32,
    ) -> Option<Self> {
        let up = frame.up.normalize_or(DVec3::Y);
        let walk_height = walk_height.min(start_height);
        let p = frame.origin + (frame.axis_u + frame.axis_v) * 0.5 + up * walk_height;
        let (cell, c) = grid.locate(p);
        let coords = |q: DVec3| grid.face_coords(cell.face, q);
        let mut jacobian = [[0.0f32; 4]; 3];
        for (axis, d) in [DVec3::X, DVec3::Y, DVec3::Z].into_iter().enumerate() {
            let (a, b) = (coords(p + d)?, coords(p - d)?);
            for row in 0..3 {
                jacobian[row][axis] = ((a[row] - b[row]) * 0.5) as f32;
            }
        }
        Some(Self {
            cell: [cell.i, cell.j, cell.k, i32::from(cell.face) | 8],
            frac: [
                (c[0] - f64::from(cell.i)) as f32,
                (c[1] - f64::from(cell.j)) as f32,
                (c[2] - f64::from(cell.k)) as f32,
                (start_height - walk_height) as f32,
            ],
            at: {
                let e = (p - eye).as_vec3();
                [e.x, e.y, e.z, f32::from_bits(first)]
            },
            jacobian,
        })
    }
}

/// `ProbeFrame` of chunk_draw.wgsl: the eye in the queries' space (render space), the
/// queries in all and the blocks.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct ProbeFrame {
    pub eye: [f32; 4],
    pub counts: [u32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Shape;

    #[test]
    fn sphere_tiles_are_eight_metres_on_the_face_under_the_eye() {
        let ground = PlanetGround::default();
        assert!(
            ground.chart(DVec3::new(0.0, 16_010.0, 0.0)).is_none(),
            "no chart before the grid"
        );
        let grid = Grid::new(16_000.0, 0.1).unwrap();
        ground.set_grid(grid);
        let eye = DVec3::new(30.0, 16_010.0, -40.0);
        let chart = ground.chart(eye).unwrap();
        assert_eq!(chart.id, u64::from(grid.locate(eye).0.face));
        let tile = [
            (chart.eye[0] / 8.0).floor() as i32,
            (chart.eye[1] / 8.0).floor() as i32,
        ];
        let frame = ground.tile_frame(chart.id, tile, 1).unwrap();
        assert!(
            (frame.axis_u.length() - 8.0).abs() < 0.1 && (frame.axis_v.length() - 8.0).abs() < 0.1,
            "{frame:?}"
        );
        assert!((frame.origin.length() - 16_000.0).abs() < 1e-6);
        assert!(frame.up.dot(eye.normalize()) > 0.999_99);
        // The eye's own tile holds it.
        let at = ground.locate(chart.id, eye).unwrap();
        assert_eq!(
            [(at[0] / 8.0).floor() as i32, (at[1] / 8.0).floor() as i32],
            tile
        );
        // A probe tile's reference point is its cell's.
        let probe = ProbeTile::new(&grid, &frame, 12.0, 12.0, eye, 0).unwrap();
        assert!(
            probe.frac[..3].iter().all(|f| (0.0..1.0).contains(f)),
            "{probe:?}"
        );
    }

    #[test]
    fn the_chart_stays_on_its_face_near_the_edge() {
        let ground = PlanetGround::default();
        let grid = Grid::new(16_000.0, 0.1).unwrap();
        ground.set_grid(grid);
        let n = f64::from(grid.cells());
        let face = 2;
        let a = ground
            .chart(grid.position(face, [n - 10.0, n * 0.5, 100.0]))
            .unwrap();
        // Just across the edge: still the first face.
        let b = ground
            .chart(grid.position(face, [n + 50.0, n * 0.5, 100.0]))
            .unwrap();
        assert_eq!(a.id, b.id);
        // Far across: the other face.
        let c = ground
            .chart(grid.position(face, [n * 1.5, n * 0.5, 100.0]))
            .unwrap();
        assert_ne!(a.id, c.id);
    }

    #[test]
    fn plane_tiles_are_world_xz() {
        let ground = PlanetGround::default();
        let grid = Grid::plane(Shape::Plane, 4_000.0, 0.1).unwrap();
        ground.set_grid(grid);
        let chart = ground.chart(DVec3::new(10.0, 5.0, -7.0)).unwrap();
        let frame = ground.tile_frame(chart.id, [0, 0], 1).unwrap();
        assert_eq!(frame.up, DVec3::Y);
        assert!((frame.axis_u.length() - 8.0).abs() < 1e-9 && frame.axis_u.y == 0.0);
    }
}
