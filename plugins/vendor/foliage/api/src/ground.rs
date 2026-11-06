//! The ground foliage grows on.
//!
//! Foliage does not know what it stands on. A **ground** gives it two things:
//!
//! - **Geometry, on the CPU** ([`GroundGeometry`]): a *chart* of tiles around the eye (8 m
//!   base tiles, and the larger tiles of the far bands spanning several), each with a frame — an origin, two edge axes and an up direction — so a heightfield
//!   tiles world XZ with +Y up, and a planet tiles the cube face under the eye with its
//!   radial up. Blades are stored in their tile's frame (`u`, `v` across it, a height along
//!   its up), so the same arena, cull and draw serve every ground.
//! - **Answers, on the GPU**: placement writes one [`GpuGroundQuery`] per candidate (a
//!   segment down the tile's up through the candidate) and the ground writes a
//!   [`GpuGroundHit`] for it — where the segment meets the surface, the surface's normal,
//!   its **ground id** (a terrain material, a biome), which decides the foliage types
//!   that grow there ([`crate::GpuFoliageType::ground_mask`]), and its colour, which
//!   grass of [`crate::FOLIAGE_FLAG_GROUND_COLOUR`] types takes. The built-in plane answers
//!   in the placement pass itself; another pass (a voxel world's) answers by reading the
//!   [`GROUND_QUERIES`], [`GROUND_ARGS`] and [`GROUND_HITS`] resources the placement pass
//!   publishes, and placement consumes the answers the next frame.
//!
//! A [`GroundLink`] joins the two: the placement pass and the ground's pass share it.
//! Grounds that change (dug terrain, finer terrain arriving near the eye) report the
//! world boxes that changed ([`GroundLink::push_changes`]) and the tiles under them are
//! placed again, keeping their blades until the new ones land.

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Vec3};
use std::sync::Mutex;

/// Registry name of the queries buffer (`GpuGroundQuery[]`), written by placement.
pub const GROUND_QUERIES: &str = "foliage_ground_queries";
/// Registry name of the hits buffer (`GpuGroundHit[]`), one per query, written by the ground.
pub const GROUND_HITS: &str = "foliage_ground_hits";
/// Registry name of the queries' dispatch header ([`GpuGroundArgs`]).
pub const GROUND_ARGS: &str = "foliage_ground_args";

/// Ground id of a hit that met no ground (or ground no foliage may grow on).
pub const GROUND_NONE: u32 = u32::MAX;

/// Lanes of an answering dispatch's workgroup: [`GpuGroundArgs::groups`] counts
/// workgroups of this many queries.
pub const GROUND_WORKGROUP: u32 = 64;

/// A tile's frame in render space (the renderer's world space, relative to its world
/// origin). A blade at `(u, v)` with height `h` stands at
/// `origin + u * axis_u + v * axis_v + h * up`.
///
/// # Layout (64 bytes, four `vec3<f32>` each followed by a pad)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuTileFrame {
    /// The tile's `(0, 0)` corner on its datum (height 0 along `up`).
    pub origin: [f32; 3],
    pub _pad0: f32,
    /// The tile's edge from `(0, 0)` to `(1, 0)`, metres.
    pub axis_u: [f32; 3],
    pub _pad1: f32,
    /// The tile's edge from `(0, 0)` to `(0, 1)`, metres.
    pub axis_v: [f32; 3],
    pub _pad2: f32,
    /// Unit up of the tile: the direction its blades grow and its heights are measured in.
    pub up: [f32; 3],
    pub _pad3: f32,
}

const _: () = assert!(std::mem::size_of::<GpuTileFrame>() == 64);

/// A tile's frame in world coordinates ([`GroundGeometry::tile_frame`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileFrame {
    pub origin: DVec3,
    pub axis_u: DVec3,
    pub axis_v: DVec3,
    pub up: DVec3,
}

impl TileFrame {
    /// The world XZ tile of a flat ground with +Y up (the tiling every heightfield uses)
    /// at base tile `coord`, `span` base tiles across.
    pub fn flat(coord: [i32; 2], span: u32) -> Self {
        let base = f64::from(crate::FOLIAGE_TILE_SIZE_METERS);
        let size = base * f64::from(span.max(1));
        Self {
            origin: DVec3::new(f64::from(coord[0]) * base, 0.0, f64::from(coord[1]) * base),
            axis_u: DVec3::new(size, 0.0, 0.0),
            axis_v: DVec3::new(0.0, 0.0, size),
            up: DVec3::Y,
        }
    }

    /// The part `[lo, hi)` (in `0..1` of each axis) of this frame: a block of its tile.
    pub fn part(&self, lo: [f64; 2], hi: [f64; 2]) -> Self {
        Self {
            origin: self.origin + self.axis_u * lo[0] + self.axis_v * lo[1],
            axis_u: self.axis_u * (hi[0] - lo[0]),
            axis_v: self.axis_v * (hi[1] - lo[1]),
            up: self.up,
        }
    }

    /// The frame in render space, relative to the renderer's world origin.
    pub fn gpu(&self, world_origin: DVec3) -> GpuTileFrame {
        let f = |v: DVec3| v.as_vec3().to_array();
        GpuTileFrame {
            origin: f(self.origin - world_origin),
            axis_u: f(self.axis_u),
            axis_v: f(self.axis_v),
            up: f(self.up.normalize_or(DVec3::Y)),
            ..GpuTileFrame::default()
        }
    }
}

impl GpuTileFrame {
    /// Render-space position of `(u, v)` at height `h` (the CPU mirror of
    /// `foliage_frame_point` in the shaders).
    pub fn point(&self, u: f32, v: f32, h: f32) -> Vec3 {
        Vec3::from(self.origin)
            + Vec3::from(self.axis_u) * u
            + Vec3::from(self.axis_v) * v
            + Vec3::from(self.up) * h
    }
}

/// One candidate's question to the ground: the segment from `start` down `-up` for
/// `length` metres. Render space.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuGroundQuery {
    pub start: [f32; 3],
    pub length: f32,
    pub up: [f32; 3],
    pub _pad: u32,
}

/// The ground's answer to a [`GpuGroundQuery`].
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuGroundHit {
    /// Metres from the query's `start` down to the surface, as `f32` bits; negative or
    /// past the query's length: no surface there.
    pub distance: u32,
    /// The surface's unit normal, octahedral, two snorm16 ([`pack_octahedral`]).
    pub normal: u32,
    /// The ground id there; [`GROUND_NONE`]: none foliage grows on.
    pub ground: u32,
    /// The ground's linear colour there, RGBA8 unorm (red in the low byte); `0`: the
    /// ground has none to give (grass keeps its type's colour).
    pub colour: u32,
}

impl GpuGroundHit {
    pub const MISS: Self = Self {
        distance: 0xbf80_0000,
        normal: 0,
        ground: GROUND_NONE,
        colour: 0,
    };
}

/// The header of a frame's queries: the answering dispatch's workgroups (an indirect
/// dispatch: `x` workgroups of [`GROUND_WORKGROUP`]), then the query count.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuGroundArgs {
    pub groups: [u32; 3],
    pub count: u32,
}

/// A unit vector in two snorm16 halves (octahedral), as WGSL's `pack2x16snorm` of the
/// folded `xy` (shared by every ground's answer and placement's decode).
pub fn pack_octahedral(n: Vec3) -> u32 {
    let n = n / (n.x.abs() + n.y.abs() + n.z.abs()).max(1e-20);
    let mut p = glam::Vec2::new(n.x, n.y);
    if n.z < 0.0 {
        p = (glam::Vec2::ONE - glam::Vec2::new(p.y.abs(), p.x.abs()))
            * glam::Vec2::new(p.x.signum(), p.y.signum());
    }
    let s = |x: f32| ((x.clamp(-1.0, 1.0) * 32767.0).round() as i32 as u32) & 0xffff;
    s(p.x) | (s(p.y) << 16)
}

/// Inverse of [`pack_octahedral`].
pub fn unpack_octahedral(packed: u32) -> Vec3 {
    let s = |bits: u32| ((bits & 0xffff) as u16 as i16 as f32 / 32767.0).max(-1.0);
    let p = glam::Vec2::new(s(packed), s(packed >> 16));
    let mut n = Vec3::new(p.x, p.y, 1.0 - p.x.abs() - p.y.abs());
    if n.z < 0.0 {
        let q = (glam::Vec2::ONE - glam::Vec2::new(n.y.abs(), n.x.abs()))
            * glam::Vec2::new(n.x.signum(), n.y.signum());
        n = Vec3::new(q.x, q.y, n.z);
    }
    n.normalize_or(Vec3::Y)
}

/// Which tiles cover a ground and where they stand (the CPU half of a ground).
pub trait GroundGeometry: Send + Sync {
    /// The chart of tiles around world point `eye` and the eye's place on it (metres along
    /// the chart's u and v), or `None` while the ground cannot say (not loaded yet). Tiles
    /// of one chart only are resident: a new chart places every tile again.
    fn chart(&self, eye: DVec3) -> Option<GroundChart>;
    /// The tile of `chart` whose corner is base tile `coord` and which spans `span` base
    /// tiles each way (base tile `[i, j]` spans chart metres `[8 i, 8 i + 8)` and
    /// `[8 j, 8 j + 8)`; a far band's tile spans several).
    fn tile_frame(&self, chart: u64, coord: [i32; 2], span: u32) -> Option<TileFrame>;
    /// Where world point `p` lies on `chart` (metres), to find the tiles under a change.
    fn locate(&self, chart: u64, p: DVec3) -> Option<[f64; 2]>;
    /// Whether the ground has any content (a terrain exists in the scene). A ground
    /// without any hands placement to its built-in ground until it has; one that has
    /// content but cannot tile yet (loading) is present and has no [`Self::chart`].
    fn present(&self) -> bool {
        true
    }
}

/// A chart and the eye's place on it ([`GroundGeometry::chart`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundChart {
    pub id: u64,
    pub eye: [f64; 2],
}

impl<T: GroundGeometry> GroundGeometry for std::sync::Arc<T> {
    fn chart(&self, eye: DVec3) -> Option<GroundChart> {
        (**self).chart(eye)
    }
    fn tile_frame(&self, chart: u64, coord: [i32; 2], span: u32) -> Option<TileFrame> {
        (**self).tile_frame(chart, coord, span)
    }
    fn locate(&self, chart: u64, p: DVec3) -> Option<[f64; 2]> {
        (**self).locate(chart, p)
    }
    fn present(&self) -> bool {
        (**self).present()
    }
}

/// Flat ground at world height 0 under +Y, tiled over world XZ: the heightfield's chart.
/// Its answers come from placement's own kernel (the terrain capture where one is
/// published, else the plane).
pub struct PlaneGround;

impl GroundGeometry for PlaneGround {
    fn chart(&self, eye: DVec3) -> Option<GroundChart> {
        Some(GroundChart {
            id: 0,
            eye: [eye.x, eye.z],
        })
    }
    fn tile_frame(&self, _chart: u64, coord: [i32; 2], span: u32) -> Option<TileFrame> {
        Some(TileFrame::flat(coord, span))
    }
    fn locate(&self, _chart: u64, p: DVec3) -> Option<[f64; 2]> {
        Some([p.x, p.z])
    }
}

/// The ground a placement pass grows on and what the two exchange (shared by the
/// placement pass and, for a ground answered elsewhere, the pass that answers).
pub struct GroundLink {
    geometry: Box<dyn GroundGeometry>,
    /// Whether another pass answers the queries ([`Self::answered_elsewhere`]).
    external: bool,
    state: Mutex<LinkState>,
}

/// A tile a batch asks about: its frame (world), the height along its up its queries
/// start at (the same for all of them), and its queries, `[first, first + count)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AskedTile {
    pub frame: TileFrame,
    pub start_height: f64,
    pub first: u32,
    pub count: u32,
}

/// A batch of queries placement published this frame ([`GroundLink::take_question`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub batch: u64,
    pub tiles: Vec<AskedTile>,
}

#[derive(Default)]
struct LinkState {
    changes: Vec<[DVec3; 2]>,
    /// The batch of queries published this frame and not answered yet.
    asked: Option<Question>,
    /// The latest batch the ground answered.
    answered: Option<u64>,
}

impl GroundLink {
    /// A ground whose queries placement answers itself (a heightfield).
    pub fn built_in(geometry: Box<dyn GroundGeometry>) -> Self {
        Self {
            geometry,
            external: false,
            state: Mutex::default(),
        }
    }

    /// A ground whose queries another pass answers ([`Self::take_question`],
    /// [`Self::answer`]).
    pub fn answered_elsewhere(geometry: Box<dyn GroundGeometry>) -> Self {
        Self {
            geometry,
            external: true,
            state: Mutex::default(),
        }
    }

    pub fn geometry(&self) -> &dyn GroundGeometry {
        &*self.geometry
    }

    pub fn is_external(&self) -> bool {
        self.external
    }

    /// The ground changed in these world boxes (axis-aligned, min and max): the tiles
    /// under them are placed again.
    pub fn push_changes(&self, boxes: impl IntoIterator<Item = [DVec3; 2]>) {
        self.state
            .lock()
            .expect("ground link")
            .changes
            .extend(boxes);
    }

    /// The changes pushed since the last call.
    pub fn take_changes(&self) -> Vec<[DVec3; 2]> {
        std::mem::take(&mut self.state.lock().expect("ground link").changes)
    }

    /// Placement published batch `batch` of queries this frame, about `tiles`.
    pub fn ask(&self, batch: u64, tiles: Vec<AskedTile>) {
        self.state.lock().expect("ground link").asked = Some(Question { batch, tiles });
    }

    /// The batch an answering pass should answer this frame (its queries are in the
    /// published resources), taken once.
    pub fn take_question(&self) -> Option<Question> {
        self.state.lock().expect("ground link").asked.take()
    }

    /// The ground answered `batch` (its hits are written by this frame's commands).
    pub fn answer(&self, batch: u64) {
        self.state.lock().expect("ground link").answered = Some(batch);
    }

    /// The latest batch answered.
    pub fn answered(&self) -> Option<u64> {
        self.state.lock().expect("ground link").answered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_their_wgsl_sizes() {
        assert_eq!(std::mem::size_of::<GpuTileFrame>(), 64);
        assert_eq!(std::mem::size_of::<GpuGroundQuery>(), 32);
        assert_eq!(std::mem::size_of::<GpuGroundHit>(), 16);
        assert_eq!(std::mem::size_of::<GpuGroundArgs>(), 16);
    }

    #[test]
    fn octahedral_normals_round_trip() {
        for n in [
            Vec3::Y,
            -Vec3::Y,
            Vec3::X,
            Vec3::new(0.3, 0.9, -0.2).normalize(),
            Vec3::new(-0.6, -0.1, -0.79).normalize(),
        ] {
            let back = unpack_octahedral(pack_octahedral(n));
            assert!((back - n).length() < 1e-3, "{n} -> {back}");
        }
    }

    #[test]
    fn flat_frames_tile_world_xz_under_plus_y() {
        let frame = TileFrame::flat([-3, 5], 1).gpu(DVec3::new(100.0, 0.0, 0.0));
        assert_eq!(frame.point(0.0, 0.0, 2.0), Vec3::new(-124.0, 2.0, 40.0));
        assert_eq!(frame.point(1.0, 1.0, 0.0), Vec3::new(-116.0, 0.0, 48.0));
        // A far band's tile spans several base tiles from its corner.
        let big = TileFrame::flat([-4, 4], 4).gpu(DVec3::ZERO);
        assert_eq!(big.point(1.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 64.0));
        let block = TileFrame::flat([0, 0], 4)
            .part([0.5, 0.25], [0.75, 0.5])
            .gpu(DVec3::ZERO);
        assert_eq!(block.point(0.0, 0.0, 0.0), Vec3::new(16.0, 0.0, 8.0));
        assert_eq!(block.point(1.0, 1.0, 0.0), Vec3::new(24.0, 0.0, 16.0));
        assert_eq!(
            PlaneGround.chart(DVec3::new(1.0, 2.0, 3.0)).unwrap().eye,
            [1.0, 3.0]
        );
    }

    #[test]
    fn a_question_is_answered_once() {
        let link = GroundLink::answered_elsewhere(Box::new(PlaneGround));
        assert_eq!(link.take_question(), None);
        link.ask(7, Vec::new());
        assert_eq!(link.take_question().map(|q| q.batch), Some(7));
        assert_eq!(link.take_question(), None);
        link.answer(7);
        assert_eq!(link.answered(), Some(7));
        link.push_changes([[DVec3::ZERO, DVec3::ONE]]);
        assert_eq!(link.take_changes().len(), 1);
        assert!(link.take_changes().is_empty());
    }
}

/// Grounds the passes that answer them (a terrain) offer foliage placement
/// while a graph is built: build state the graph's nodes share. Placement
/// grows on the first; with none it keeps its built-in ground.
#[derive(Default)]
pub struct OfferedGrounds(pub Vec<std::sync::Arc<GroundLink>>);
