//! Chunk trees in an octree (`docs/chunk-octree.md`):
//! generation of chunk trees from the terrain contract, residency for a
//! moving eye and primary visibility.
//!
//! A node is a chunk: 64^3 cells of one level, stored as a 64-tree; a
//! footprint is the 64x64 columns of one level under a stack of nodes.
//! [`ChunkWorld::update`] asks the [`residency::Planner`] which nodes to
//! draw and generate, and generates and evicts them on the GPU within a
//! budget; [`ChunkWorld::draw`] rasterizes the chunks' boxes to seed each
//! pixel with the nearest box its ray enters, then traces each pixel's ray
//! through the cut in a compute pass (shaders/chunk_draw.wgsl), and reports
//! what rays met so detail goes where it is seen.

mod candidates;
pub mod classify;
mod drawn;
mod sun_pages;
mod volumes;
pub use sun_pages::SunPageStats;
pub mod ground;
pub mod pass;
pub mod residency;
pub mod scene;

use crate::edit_store::{Brick, ColumnKey, EditStore};
use crate::edits::FaceBrush;
use crate::engine::{terrain_bytes, terrain_source, WorldGpu};
use crate::grid::Grid;
use crate::planet::{Edits, Planet};
use bytemuck::{Pod, Zeroable};
use classify::{brush_meets, EditView, Ranges};
pub use classify::{Footprint, Kind, NodeKey};
use glam::{DMat3, DVec3, Mat4, Vec2, Vec3};
use rustc_hash::{FxHashMap, FxHashSet};
pub use volumes::{VolumeId, MAX_VOLUMES};
use wgpu::util::DeviceExt;

/// Cells along a chunk edge (and columns along a tile edge).
pub const CHUNK: i32 = 64;
const TILE_COLUMNS: u64 = 4096;
/// Tiles generated per dispatch (their columns are scratch until their
/// chunks are built).
pub(crate) const TILE_BATCH: usize = 512;

/// Words of a computed footprint's measured layers (`RANGE_WORDS` of
/// chunk_generate.wgsl): four a quarter, by quarter (each a child footprint's).
const RANGE_WORDS: usize = 16;

/// A footprint's measured ranges by quarter (none past a face's edge):
/// what the planner classifies its nodes by (`residency::Event::Measured`).
type Quarters = [Option<Ranges>; 4];
/// Chunks generated per dispatch (column masks are scratch).
const CHUNK_BATCH: usize = 2048;
/// Terrain regions a volume keeps on the GPU at most (`engine::RegionPool`;
/// streams: 896 bytes each, 29 MB; none for a program without regions).
const REGION_CAPACITY: usize = 32768;
/// Chunk slots at most by default (`Capacity::chunks`), within what the
/// device can bind.
pub const DEFAULT_CHUNKS: usize = 131072;
/// Chunk slots a world starts with (fewer if its capacity is less): the slot
/// pool and the leaf pool each double when short (`ChunkWorld::grow_slots`,
/// `ChunkWorld::grow_leaves`), up to the capacity, so a world holds what its
/// views have needed (a settled Earth view some 30k chunks), not the most.
const CHUNKS_START: usize = 8192;
/// Chunk jobs of `DETAIL_LEVELS` a batch (`cell_materials` slots:
/// `CELL_WORDS` words a column, 288 KiB each).
const DETAIL_JOBS: usize = 128;
/// Drawn nodes in view an edit changed, generated in the frame it is made
/// whatever the budget (a small edit's all).
const EDITED_NOW: usize = 48;
/// Levels whose chunks get per-voxel materials, natural and edited: cells
/// of 0.1-0.2 m at 0.1 m voxels, near the eye. Coarser chunks' leaves keep
/// their pairs (an edit's top materials vote in them): a dug wall's detail
/// pages at every level filled a game's pages after some 80 digs 8 m across.
const DETAIL_LEVELS: u32 = 2;
/// Kept chunks (`pack_main`, `restore_main` of chunk_generate.wgsl): words
/// a frame's packs may take, and packs a frame at most (more evicted
/// chunks are not kept: they generate again).
const PACK_WORDS: u64 = 1 << 19;
const MAX_PACKS: usize = 64;
/// Bytes a kept chunk counts besides its pack (its entry).
const KEPT_OVERHEAD: usize = 64;
/// Kept chunks' bytes (`KeptChunks::to_bytes`): magic, and the pack format
/// (bump with `pack_main`'s layout).
const KEPT_MAGIC: [u8; 4] = *b"HVPK";
const KEPT_FORMAT: u16 = 2;

/// A uniform brick's table entry flag (`UNIFORM_BRICK` of
/// chunk_generate.wgsl): its cells' one edit in the low 16 bits.
const UNIFORM_BRICK: u32 = 0x4000_0000;
/// Words a chunk job's column bits take (`FEATURE_JOB_WORDS` of
/// chunk_generate.wgsl).
const FEATURE_JOB_WORDS: u64 = 128;
/// Words a column of a `cell_materials` slot (`CELL_WORDS` of
/// chunk_generate.wgsl): 16 of cell bytes, the column's natural top, its
/// leaves with cells left.
const CELL_WORDS: u64 = 18;
/// Most detail pages by default (`Capacity::detail_pages`): 4 KiB each,
/// one for each leaf page (of `DETAIL_LEVELS`) holding a leaf whose cells
/// its pair would draw wrong. The pool starts at `DETAIL_PAGES_START` and
/// doubles when a build finds it full: what a view needs scales with the
/// level 0-1 surface it shows (at 1600x900: a voxel_craft-like meadow
/// ~1.3k, the Moon's speckled regolith ~18k, inside an Earth wood ~29k, an
/// Earth mountain's snow and rock ~81k, its pages ~80% full of leaves that
/// need them).
pub const DEFAULT_DETAIL_PAGES: usize = 262_144;
/// Detail pages a world starts with (fewer if its capacity is less).
const DETAIL_PAGES_START: u64 = 8192;
/// Words a detail page (`DETAIL_WORDS` of chunk_generate.wgsl).
const DETAIL_WORDS: u64 = 1024;

/// `Cull` of chunk_draw.wgsl: two counts, then 65 bins' counts and cursors.
const CULL_BYTES: u64 = 4 * (2 + 2 * 65);

/// Metres over the highest possible solid cell where rays may end: far
/// above the f32 error of the ceiling test.
const CEILING_MARGIN: f64 = 16.0;
/// Leaves a chunk slot holds on average (8 bytes each), in pages of 64
/// (`chunk_generate.wgsl`), up to `LEAF_PAGES` a chunk.
const LEAVES_PER_SLOT: u64 = 384;
const LEAF_PAGE: u64 = 64;
const LEAF_PAGES: u64 = 64;
/// A brush buffer's content when there are none.
const NO_BRUSH: [FaceBrush; 1] = [FaceBrush {
    flags: 0,
    radius_half: 0,
    k_lo: 0,
    k_hi: 0,
    center: [0; 4],
    ball: [0; 4],
}];

/// Chunk table entries a slot: the drawn nodes (Solid and Buried ones
/// without slots) and their ancestors.
const TABLE_PER_SLOT: usize = 8;
/// Draws whose timings can be pending at once.
pub const TIMING_RING: u64 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TileJob {
    face: u32,
    level: u32,
    cx: i32,
    cy: i32,
    /// Its slot in the footprint pool.
    slot: u32,
    _pad: [u32; 3],
}

/// Footprints a volume's pool holds at most by default: their columns, cave
/// primitives and material slopes (70 KB each), within what the device
/// binds. A pool
/// starts at two batches' and doubles when it would reuse a slot used in
/// the last `FOOTPRINT_RECENT` frames (`Volume::grow_footprints`).
const FOOTPRINT_POOL: usize = 2048;
const FOOTPRINT_RECENT: u64 = 120;
/// A footprint's cave primitives' bytes.
const CAVE_SLOT_BYTES: u64 = crate::volumes::CAVE_SLOT_WORDS as u64 * 4;
/// Material slope words a footprint (`SLOPE_STRIDE` of chunk_generate.wgsl:
/// a level-3 footprint's 34x34 level-4 blocks).
const SLOPE_STRIDE: u64 = 1156;
/// Level-4 tops a footprint's slopes read, at most (`SLOPE_LATTICE`: 38x38).
const SLOPE_LATTICE: u64 = 1444;

/// What a [`ChunkWorld`] holds, each within what the device binds: chunk
/// slots (a chunk's 64-tree and leaves, about 7 KB each), detail pages
/// (4 KiB each), footprint pool slots (75 KB each and the features'
/// placements, `CompiledFeatures::stride` squares of 8 bytes; at least two
/// batches')
/// cached baked bricks (1 KiB each, before the least recently used are
/// reused) and edited chunks kept on the CPU (bytes, least recently used
/// dropped). The default is a game's: 131072 chunks, a world and its device
/// about 3 GB of device memory. `counters` builds the draw with trace
/// counters (`take_counts`, `pixel_trace`, `lost_where`: diagnostics, and
/// a fragment shader with side effects, slower).
#[derive(Clone, Copy, Debug)]
pub struct Capacity {
    pub chunks: usize,
    pub detail_pages: usize,
    pub footprints: usize,
    /// Baked bricks kept on the GPU (1 KiB each; `BrickCache`).
    pub bricks: usize,
    /// Bytes of edited chunks kept on the CPU across eviction (`Kept`; 0:
    /// none kept, every reload generates).
    pub kept_bytes: usize,
    pub counters: bool,
}

impl Default for Capacity {
    fn default() -> Self {
        Self {
            chunks: DEFAULT_CHUNKS,
            detail_pages: DEFAULT_DETAIL_PAGES,
            footprints: FOOTPRINT_POOL,
            bricks: 65536,
            kept_bytes: 256 << 20,
            counters: false,
        }
    }
}

/// GPU slots of data kept across frames by key: footprints' columns (and
/// cave primitives), so a footprint's columns are computed once while it stays
/// in use, whichever of its nodes are generated when (they were 60-75 % of
/// a chunk's generation), and feature candidate blocks
/// (`candidates::CandidateBlocks`). Both depend only on the terrain, never
/// on edits. Least recently used slots are reused (a clock), never one
/// this frame's batch uses.
struct SlotPool<K> {
    slots: FxHashMap<K, u32>,
    keys: Vec<Option<K>>,
    /// The frame each slot was last used in, and whether it was used since
    /// the clock's hand last passed.
    used: Vec<u64>,
    referenced: Vec<bool>,
    hand: usize,
    /// Whether a slot used in the last `FOOTPRINT_RECENT` frames was reused
    /// (or none was free) since the pool last grew.
    pressed: bool,
}

impl<K: Copy + Eq + std::hash::Hash> SlotPool<K> {
    fn new(size: usize) -> Self {
        Self {
            slots: FxHashMap::default(),
            keys: vec![None; size],
            used: vec![u64::MAX; size],
            referenced: vec![false; size],
            hand: 0,
            pressed: false,
        }
    }

    fn len(&self) -> usize {
        self.keys.len()
    }

    /// `size` slots (no fewer than before), the new ones free.
    fn grow(&mut self, size: usize) {
        self.keys.resize(size, None);
        self.used.resize(size, u64::MAX);
        self.referenced.resize(size, false);
        self.pressed = false;
    }

    /// `key`'s slot at `frame`, and whether its data is there already;
    /// none when every slot serves this frame.
    fn take(&mut self, key: K, frame: u64) -> Option<(u32, bool)> {
        if let Some(&slot) = self.slots.get(&key) {
            self.used[slot as usize] = frame;
            self.referenced[slot as usize] = true;
            return Some((slot, true));
        }
        let n = self.keys.len();
        for _ in 0..2 * n {
            let h = self.hand;
            self.hand = (self.hand + 1) % n;
            if self.used[h] == frame {
                continue;
            }
            if std::mem::take(&mut self.referenced[h]) {
                continue;
            }
            if let Some(old) = self.keys[h].take() {
                self.slots.remove(&old);
                self.pressed |= self.used[h].saturating_add(FOOTPRINT_RECENT) > frame;
            }
            self.keys[h] = Some(key);
            self.slots.insert(key, h as u32);
            self.used[h] = frame;
            self.referenced[h] = true;
            return Some((h as u32, false));
        }
        self.pressed = true;
        None
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ChunkJob {
    tile: u32,
    cz: i32,
    slot: u32,
    brushes: u32,
    brush_count: u32,
    /// Refs of the large log (first), then the recent log's.
    large: u32,
    /// Its baked bricks' table in `baked_refs`, or NONE.
    baked: u32,
    /// Its `cell_materials` slot at `DETAIL_LEVELS` (its leaves get
    /// per-voxel materials where their pairs would draw cells wrong), or
    /// NONE.
    detail: u32,
}

/// `ChunkStatic` of chunk_draw.wgsl: a chunk's frame, written when it is
/// generated (its layers when they are read back).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ChunkStatic {
    m: [[f32; 4]; 3],
    inv: [[f32; 4]; 3],
    origin_hi: [f32; 4],
    origin_lo: [f32; 4],
    /// Lowest and one past the highest occupied layer, slot (bits), flags
    /// (bits; 1: mirror x; 8..16: level).
    bounds: [f32; 4],
}

/// A chunk record's size in vec4s (`chunk_records.wgsl`):
/// its slot, its `ChunkStatic` (9), its key; and in bytes.
const RECORD_VEC4S: usize = 2 + std::mem::size_of::<ChunkStatic>() / 16;
const RECORD_BYTES: usize = RECORD_VEC4S * 16;

/// A planet-centred position as a high and a low f32 part.
fn split(v: DVec3) -> ([f32; 4], [f32; 4]) {
    let hi = v.as_vec3();
    let lo = (v - hi.as_dvec3()).as_vec3();
    (hi.extend(0.0).to_array(), lo.extend(0.0).to_array())
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct DrawParams {
    view_proj: [[f32; 4]; 4],
    inverse_view_proj: [[f32; 4]; 4],
    prev_view_proj: [[f32; 4]; 4],
    eye_hi: [f32; 4],
    eye_lo: [f32; 4],
    /// Direction to the sun, near plane.
    sun: [f32; 4],
    /// x: the renderer's debug mode in the low byte (1 UV grid, 32 trace
    /// cost, 34 levels, 35 levels over the left half), the plain colour
    /// flag (bit 8), steady shadows (0x1000); y: a counting build; z, w:
    /// the debug pixel (`set_debug_pixel`).
    debug: [u32; 4],
    /// Chunk table slots - 1, base cells per face edge, boxes holding the
    /// eye, frame.
    table: [u32; 4],
    /// The eye in noise-domain units: integer part, fraction.
    eye_domain: [i32; 4],
    eye_domain_frac: [f32; 4],
    /// Up at the eye (xyz) and the eye's distance from the planet's
    /// centre (w); a plane's up is +y.
    up: [f32; 4],
    /// x: the eye's squared radius less the ceiling's (a plane: its height
    /// over the ceiling), from f64; z: a plane's eye height over its band's
    /// bottom (its world's lowest solid).
    ceiling: [f32; 4],
    /// The planet's radius and voxel (m), its first exact level
    /// ([`exact_level`]), the eye's squared radius less the planet's (from
    /// f64).
    planet: [f32; 4],
    /// x: the octree's coarsest level; y: nodes fading; z: the chunk an
    /// eye under the ground walks from; w: the volume's anchor
    /// (`ChunkWorld::place_anchors`).
    cut: [u32; 4],
    /// The eye in the scene lights' coordinates; w 1 with lights bound.
    light_eye: [f32; 4],
    /// x: the sky rays' reach (m) with GI probes, else 0; y: the sun's
    /// angular radius (radians); z: the cosine of the atmosphere's
    /// (`set_atmosphere_frame`: copied from its frame on the GPU).
    sky: [f32; 4],
    /// An orthographic view's ray direction (xyz; zero: the camera's
    /// perspective) and its near plane's offset along it (w).
    ortho: [f32; 4],
    /// xy: this frame's projection jitter less the last frame's; zw: this
    /// frame's (NDC): the denoiser's history is of pixels, not of their
    /// jittered points, and its output of those points again.
    history: [f32; 4],
    /// The pixels' rays as points a view depth unit beyond the eye's
    /// (`ray_plane`): pixel (0, 0)'s and the eye's view depth, a pixel's
    /// step along x, along y; [1][3] 1 when the camera holds still.
    rays: [[f32; 4]; 3],
    /// The volume's axes in the world (its rotation's columns; w unused):
    /// rays, the sun and lights come into its frame by them, normals go
    /// out. Identity for the camera's own parameters.
    rotation: [[f32; 4]; 3],
    /// x: the volume's index (its chunk keys' high bits); y: its pass in
    /// this view (0: the first traced, which also writes the pixels no
    /// volume hits); z: the volumes traced; w: flags (1: a volume moved
    /// since the last frame: the denoiser reprojects by `motions`; 2: this
    /// volume moved: its hits write their own motion).
    volume: [u32; 4],
    /// The pass's pixels (x0, y0, x1, y1; x0, y0 multiples of
    /// `RECT_ALIGN`): its volume's bounds on the screen, the whole view for
    /// the first pass, which also writes the pixels no volume hits.
    rect: [u32; 4],
}

/// A pass rectangle's origin's alignment (pixels): whole workgroups of
/// every kernel's scale (a 32x32 tile of `feedback`, 4x4 blocks of
/// `sky_sample` in 8x8 groups).
const RECT_ALIGN: u32 = 32;

/// Workgroups over `rect` (`DrawParams::rect`) of a kernel taking `per`
/// pixels a workgroup along each side.
fn rect_groups(rect: [u32; 4], per: u32) -> (u32, u32) {
    (
        (rect[2] - rect[0]).div_ceil(per),
        (rect[3] - rect[1]).div_ceil(per),
    )
}

/// The pixels of view `v` volume `vol`'s bounds may cover: the corners of
/// the box around its bounding ball projected, a pixel more each way, the
/// origin rounded down to `RECT_ALIGN`; the whole view when the box reaches
/// behind the eye's near plane or the volume has no bounds.
fn screen_rect(v: &ViewInput, vol: &volumes::Volume) -> [u32; 4] {
    let whole = [0, 0, v.size[0], v.size[1]];
    let Some(radius) = vol.bounds else {
        return whole;
    };
    let centre = vol.transform.position - v.eye;
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for n in 0..8 {
        let corner = centre
            + DVec3::new(
                if n & 1 == 0 { -radius } else { radius },
                if n & 2 == 0 { -radius } else { radius },
                if n & 4 == 0 { -radius } else { radius },
            );
        let clip = v.view_proj * corner.as_vec3().extend(1.0);
        if clip.w <= v.near.max(1e-6) {
            return whole;
        }
        let ndc = clip.truncate().truncate() / clip.w;
        lo = lo.min(ndc);
        hi = hi.max(ndc);
    }
    let size = Vec2::new(v.size[0] as f32, v.size[1] as f32);
    // NDC y up, pixels y down.
    let x = ((Vec2::new(lo.x, hi.x) * 0.5 + 0.5) * size.x).to_array();
    let y = ((Vec2::new(-hi.y, -lo.y) * 0.5 + 0.5) * size.y).to_array();
    let clamp = |p: f32, most: u32| (p.max(0.0) as u32).min(most);
    let x0 = clamp(x[0] - 1.0, v.size[0]) / RECT_ALIGN * RECT_ALIGN;
    let y0 = clamp(y[0] - 1.0, v.size[1]) / RECT_ALIGN * RECT_ALIGN;
    let x1 = clamp(x[1] + 2.0, v.size[0]).max(x0);
    let y1 = clamp(y[1] + 2.0, v.size[1]).max(y0);
    [x0, y0, x1, y1]
}

/// The pixels volume `vol`'s sun shadow can fall on in view `v`: its
/// bounds' rectangle (`screen_rect`) stretched the way the shadow runs from
/// it, to its vanishing point (where the direction away from the sun
/// projects) when that is ahead of the eye (the infinite cylinder of the
/// shadow projects into the hull of the bounds' disk and that point), else
/// to the view's edges it heads for (it comes towards the eye). The whole
/// view where the bounds reach behind the eye.
fn shadow_rect(v: &ViewInput, vol: &volumes::Volume) -> [u32; 4] {
    let whole = [0, 0, v.size[0], v.size[1]];
    let rect = screen_rect(v, vol);
    if vol.bounds.is_none() || rect == whole {
        return whole;
    }
    let size = Vec2::new(v.size[0] as f32, v.size[1] as f32);
    let (mut lo, mut hi) = (
        Vec2::new(rect[0] as f32, rect[1] as f32),
        Vec2::new(rect[2] as f32, rect[3] as f32),
    );
    let away = v.view_proj * (-v.sun).extend(0.0);
    if away.w > 1e-6 {
        // NDC y up, pixels y down.
        let ndc = away.truncate().truncate() / away.w;
        let p = Vec2::new((ndc.x * 0.5 + 0.5) * size.x, (0.5 - ndc.y * 0.5) * size.y);
        lo = lo.min(p);
        hi = hi.max(p);
    } else {
        // The way its centre's projection moves along the shadow (the sign
        // of d/ds of (c + s a).xy / (c + s a).w).
        let c = v.view_proj * (vol.transform.position - v.eye).as_vec3().extend(1.0);
        let d = away.truncate().truncate() * c.w - c.truncate().truncate() * away.w;
        if d.x > 0.0 {
            hi.x = size.x;
        } else if d.x < 0.0 {
            lo.x = 0.0;
        }
        if d.y > 0.0 {
            lo.y = 0.0;
        } else if d.y < 0.0 {
            hi.y = size.y;
        }
    }
    let clamp = |p: f32, most: u32| (p.max(0.0) as u32).min(most);
    let x0 = clamp(lo.x, v.size[0]) / RECT_ALIGN * RECT_ALIGN;
    let y0 = clamp(lo.y, v.size[1]) / RECT_ALIGN * RECT_ALIGN;
    [
        x0,
        y0,
        clamp(hi.x + 1.0, v.size[0]).max(x0),
        clamp(hi.y + 1.0, v.size[1]).max(y0),
    ]
}

/// Whether volume `caster`'s sun shadow (its bounds' cylinder away from the
/// sun, `sun` towards it) reaches `receiver`'s bounds.
fn shadow_reaches(caster: &volumes::Volume, receiver: &volumes::Volume, sun: DVec3) -> bool {
    let (Some(rc), Some(rr)) = (caster.bounds, receiver.bounds) else {
        return true;
    };
    let d = receiver.transform.position - caster.transform.position;
    let along = -d.dot(sun);
    along > -(rc + rr) && (d + sun * along).length() < rc + rr
}

/// A view of the volumes in the world frame (`ChunkWorld::draw_params`):
/// its frame number, eye, camera-relative view-projection and the last
/// frame's (rebased to this frame's eye), projection jitter (this frame's,
/// the last's), size (pixels), near plane, the sun's direction, the drawn
/// chunks, the scene lights' eye, whether GI probes trace, and an
/// orthographic view's axis and near offset.
struct ViewInput {
    frame: u64,
    eye: DVec3,
    view_proj: Mat4,
    prev_view_proj: Mat4,
    jitter: [Vec2; 2],
    size: [u32; 2],
    near: f32,
    sun: Vec3,
    drawn: u32,
    light_eye: Option<Vec3>,
    gi: bool,
    ortho: Option<(Vec3, f32)>,
}

/// Where a volume's points were at the last frame, as a matrix of their
/// position relative to the eye in its frame (the eye `local_eye` there,
/// `eye` in the world): their world position when it was placed by `prev`,
/// relative to the eye now. Its f64 parts meet before f32: a still volume's
/// translation is zero.
fn moved_from(prev: &crate::volume::VolumeTransform, local_eye: DVec3, eye: DVec3) -> Mat4 {
    let shift = (prev.position - eye + prev.rotation * local_eye).as_vec3();
    Mat4::from_translation(shift) * Mat4::from_mat3(prev.matrix().as_mat3())
}

/// [`moved_from`] of points given relative to the eye in the world (the
/// volume placed by `now`).
fn world_moved_from(
    now: &crate::volume::VolumeTransform,
    prev: &crate::volume::VolumeTransform,
    eye: DVec3,
) -> Mat4 {
    moved_from(prev, now.to_local(eye), eye) * Mat4::from_mat3(now.matrix().transpose().as_mat3())
}

/// The sun's angular radius as seen from Earth (radians).
pub const EARTH_SUN_ANGULAR_RADIUS: f32 = 0.004675;

/// Where helio-pass-sky's atmosphere frame keeps the cosine of the sun
/// disk's angular radius (`AtmosphereFrame::sun.w`).
const ATMOSPHERE_SUN_DISK: u64 = 28;

/// `DrawParams::rays` for `view_proj` (eye-relative) at `size` pixels: the
/// pixels' rays as `pixel_ray` makes them, scaled to a view depth unit.
fn ray_plane(view_proj: Mat4, size: [u32; 2]) -> [[f32; 4]; 3] {
    let inverse = view_proj.inverse();
    let ray = |x: f32, y: f32| {
        let ndc = glam::Vec4::new(
            (x + 0.5) / size[0] as f32 * 2.0 - 1.0,
            1.0 - (y + 0.5) / size[1] as f32 * 2.0,
            0.5,
            1.0,
        );
        let far = inverse * ndc;
        let r = (far.truncate() / far.w).normalize();
        r / (view_proj * r.extend(0.0)).w
    };
    let (origin, x, y) = (ray(0.0, 0.0), ray(1.0, 0.0), ray(0.0, 1.0));
    [
        origin.extend((view_proj * glam::Vec4::W).w).to_array(),
        (x - origin).extend(0.0).to_array(),
        (y - origin).extend(0.0).to_array(),
    ]
}

/// Frames the sun's occlusion pages may take to converge after the
/// residency settles before `ChunkWorld::settled` stops waiting for them.
const PAGES_SETTLE_FRAMES: u64 = 180;

/// A sun occlusion trace view's side in texels (`sun_pages::VIEW_PAGES`
/// pages).
const OCCLUSION_VIEW: u32 = sun_pages::VIEW_PAGES as u32 * helio_light_api::SUN_OCCLUSION_PAGE;
/// Bytes of the pages a frame's shadow pass asks for: a bit a window page
/// of every level.
const SUN_REQUEST_BYTES: u64 = (sun_pages::LEVELS as u64
    * helio_light_api::SUN_OCCLUSION_WINDOW as u64
    * helio_light_api::SUN_OCCLUSION_WINDOW as u64)
    / 8;
/// Bytes of a trace view's parameters in the ring (aligned for copies).
const OCCLUSION_PARAMS_STRIDE: u64 =
    (std::mem::size_of::<DrawParams>() as u64).next_multiple_of(256);

/// A frame's page requests on their way back, and what they were asked
/// against.
struct RequestSlot {
    slot: ReachSlot,
    asking: Option<sun_pages::Asking>,
}

/// The sun's occlusion by the terrain (`ChunkWorld::draw_sun_occlusion`,
/// `helio_light_api::sun_occlusion`): the trace views' own buffers
/// (their volumes' parameters a pass each in a ring, copied in order into
/// each volume's sun view; the ring's room in passes) and targets,
/// the atlas of physical pages, the page tables and near planes, the frame,
/// the pages the shadow pass asks for and their copies back, and the pages
/// planned on the CPU.
struct SunOcclusion {
    params_ring: wgpu::Buffer,
    ring_passes: u64,
    cull_state: wgpu::Buffer,
    cull_bins: wgpu::Buffer,
    draw_list: wgpu::Buffer,
    seed_args: wgpu::Buffer,
    buried: wgpu::Buffer,
    counters: wgpu::Buffer,
    targets: OrthoTargets,
    atlas: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    pages_buffer: wgpu::Buffer,
    frame: wgpu::Buffer,
    requests: wgpu::Buffer,
    request_slots: Vec<RequestSlot>,
    /// What this frame's shadow pass asks against (`draw_sun_occlusion`).
    asking: Option<sun_pages::Asking>,
    /// A page of zeros: pages with no terrain under them.
    empty: wgpu::Buffer,
    /// The pages' planner, on its own thread a frame ahead.
    pages: sun_pages::SunPlanner,
    /// Each volume's placement when last seen (a move changes the
    /// occluders where it was and is).
    placed: FxHashMap<u16, crate::volume::VolumeTransform>,
    /// The last frame drawn, and the frame since which the residency has
    /// been settled (`ChunkWorld::settled`).
    drawn_frame: u64,
    settled_since: Option<u64>,
}

impl SunOcclusion {
    fn new(device: &wgpu::Device, voxel: f64) -> Self {
        use helio_light_api as so;
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let s = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let max_chunks = (device.limits().max_storage_buffer_binding_size / 4).min(1 << 20) as u64;
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("chunk sun occlusion atlas"),
            size: wgpu::Extent3d {
                width: so::SUN_OCCLUSION_ATLAS_SIDE,
                height: so::SUN_OCCLUSION_ATLAS_SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: so::SUN_OCCLUSION_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            params_ring: params_ring(device, sun_pages::MAX_VIEWS as u64),
            ring_passes: sun_pages::MAX_VIEWS as u64,
            cull_state: buffer("chunk sun cull", CULL_BYTES, s),
            cull_bins: buffer("chunk sun cull bins", max_chunks * 4, s),
            draw_list: buffer("chunk sun draw list", max_chunks * 4, s),
            seed_args: buffer("chunk sun seed draws", 32, s | wgpu::BufferUsages::INDIRECT),
            buried: buffer("chunk sun buried hits", BURIED_BYTES, s),
            counters: buffer("chunk sun counters", 1024, s),
            targets: OrthoTargets::new(device, [OCCLUSION_VIEW; 2]),
            atlas_view: view(&atlas),
            atlas,
            pages_buffer: buffer(
                "chunk sun occlusion pages",
                so::SUN_OCCLUSION_PAGES_BYTES,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            frame: buffer(
                "chunk sun occlusion frame",
                so::SUN_OCCLUSION_FRAME_BYTES,
                wgpu::BufferUsages::UNIFORM
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            requests: buffer(
                "chunk sun occlusion requests",
                SUN_REQUEST_BYTES,
                s | wgpu::BufferUsages::COPY_SRC,
            ),
            request_slots: (0..3)
                .map(|_| RequestSlot {
                    slot: ReachSlot {
                        buffer: buffer(
                            "chunk sun occlusion requests read",
                            SUN_REQUEST_BYTES,
                            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                        ),
                        state: ReachState::Free,
                        bytes: SUN_REQUEST_BYTES,
                    },
                    asking: None,
                })
                .collect(),
            asking: None,
            empty: buffer(
                "chunk sun occlusion empty page",
                u64::from(so::SUN_OCCLUSION_PAGE * so::SUN_OCCLUSION_PAGE * 4),
                wgpu::BufferUsages::COPY_SRC,
            ),
            pages: sun_pages::SunPlanner::spawn(voxel),
            placed: FxHashMap::default(),
            drawn_frame: 0,
            settled_since: None,
        }
    }
}

/// A ring of trace views' parameters for `passes` passes.
fn params_ring(device: &wgpu::Device, passes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chunk sun params ring"),
        size: OCCLUSION_PARAMS_STRIDE * passes,
        usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The targets of an orthographic trace view (`trace_view`): seed, its
/// depth, visibility, colour, distance and exits.
struct OrthoTargets {
    seed: wgpu::Texture,
    depth: wgpu::Texture,
    visibility: wgpu::Texture,
    colour: wgpu::Texture,
    distance: wgpu::Texture,
    exits: wgpu::Texture,
}

impl OrthoTargets {
    fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let texture = |label: &str, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: usage | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let a = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let s = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        Self {
            seed: texture("chunk sun seed", SEED_FORMAT, a),
            depth: texture("chunk sun depth", DEPTH_FORMAT, a),
            visibility: texture("chunk sun visibility", wgpu::TextureFormat::Rg32Uint, s),
            colour: texture("chunk sun colour", wgpu::TextureFormat::Rgba8Unorm, s),
            distance: texture("chunk sun distance", wgpu::TextureFormat::R32Float, s),
            exits: texture(
                "chunk sun exits",
                wgpu::TextureFormat::R32Float,
                s | wgpu::TextureUsages::RENDER_ATTACHMENT,
            ),
        }
    }

    fn trace(&self) -> TraceTextures<'_> {
        TraceTextures {
            seed: &self.seed,
            depth: &self.depth,
            visibility: &self.visibility,
            colour: &self.colour,
            distance: &self.distance,
            exits: &self.exits,
        }
    }
}

/// The textures a view's trace draws into (`trace_view`): the camera's
/// [`Targets`], or an [`OrthoTargets`].
#[derive(Clone, Copy)]
struct TraceTextures<'a> {
    seed: &'a wgpu::Texture,
    depth: &'a wgpu::Texture,
    visibility: &'a wgpu::Texture,
    colour: &'a wgpu::Texture,
    distance: &'a wgpu::Texture,
    exits: &'a wgpu::Texture,
}

/// What one view's trace binds of its own (`ChunkWorld::trace_view`): its
/// parameters, cull state, bins and draw list, seed draws, the Buried nodes
/// its rays met and its counters.
#[derive(Clone, Copy)]
struct ViewBuffers<'a> {
    params: &'a wgpu::Buffer,
    cull_state: &'a wgpu::Buffer,
    cull_bins: &'a wgpu::Buffer,
    draw_list: &'a wgpu::Buffer,
    seed_args: &'a wgpu::Buffer,
    buried: &'a wgpu::Buffer,
    counters: &'a wgpu::Buffer,
}

/// A frame's scene lights, for the terrain's shadows of them
/// (`local_visibility`): Helio's lights (`GpuLight`, 128 bytes each), its
/// tile light lists and counts (16 px tiles, 64 lights a tile), and the
/// eye in the lights' coordinates.
pub struct SceneLights<'a> {
    pub lights: &'a wgpu::Buffer,
    pub tile_lists: &'a wgpu::Buffer,
    pub tile_counts: &'a wgpu::Buffer,
    pub eye: Vec3,
}

/// The GI probes' rays the terrain traces (`helio_pass_gi`: `gi_frame`,
/// `gi_rays`, `gi_hits`, `gi_sun`), from the probes' places
/// ([`ChunkWorld::follow_gi`]).
pub struct GiRays<'a> {
    pub frame: &'a wgpu::Buffer,
    pub rays: &'a wgpu::Buffer,
    pub hits: &'a wgpu::Buffer,
    pub sun: &'a wgpu::Buffer,
}

/// `VoxelMaterials` of chunk_draw.wgsl: each voxel material id's material
/// row and the voxel's variation of it (`engine::material_table`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ChunkMaterials {
    rows: [helio_mats::MaterialRow; crate::engine::MATERIALS],
    variation: [crate::engine::VariationGpu; crate::engine::MATERIALS],
    detail: [f32; 4],
    shape: [f32; 4],
}

/// A chunk's affine frame: world = origin + m * cell coordinates.
#[derive(Clone, Copy, Debug)]
struct Frame {
    origin: DVec3,
    m: DMat3,
    inv: DMat3,
}

/// The affine frame of chunk `cz` over `tile`: the grid mapping's central
/// differences across the chunk, about its middle column at height `z`
/// (cells in the chunk: its columns' mean top, its ground). Across a chunk
/// the sphere curves `512 x cell / radius` of a cell from it (0.5 at the
/// coarsest level on Earth, where tiles span 600 km), and columns, which
/// meet at the planet's centre, lean `32 x cell / radius` a cell from it
/// per cell of height: about the ground neighbours agree to a twentieth of
/// a cell, about a 64-cell chunk's middle they parted by cells where the
/// ground lay far below it (docs/chunk-octree.md).
fn chunk_frame(grid: &Grid, tile: Footprint, cz: i32, z: f64) -> Frame {
    let s = f64::from(1u32 << tile.level);
    let half = f64::from(CHUNK / 2) * s;
    let centre = [
        f64::from(tile.cx * CHUNK) * s + half,
        f64::from(tile.cy * CHUNK) * s + half,
        (f64::from(cz * CHUNK) + z) * s,
    ];
    let at = |d: [f64; 3]| {
        grid.position(
            tile.face,
            [centre[0] + d[0], centre[1] + d[1], centre[2] + d[2]],
        )
    };
    let n = f64::from(CHUNK);
    let u = (at([half, 0.0, 0.0]) - at([-half, 0.0, 0.0])) / n;
    let v = (at([0.0, half, 0.0]) - at([0.0, -half, 0.0])) / n;
    let w = (at([0.0, 0.0, half]) - at([0.0, 0.0, -half])) / n;
    let m = DMat3::from_cols(u, v, w);
    Frame {
        origin: at([0.0; 3]) - m * DVec3::new(n / 2.0, n / 2.0, z),
        m,
        inv: m.inverse(),
    }
}

/// The first level drawing walks in the grid's own cells (chunk_draw.wgsl
/// `exact_level`), not in chunks' affine frames: where a frame strays from
/// the sphere by a twentieth of a cell across a chunk (512 x cell /
/// radius: a height and a slope a cell's haze and light show), and
/// planet-centred f32 positions err by under a fiftieth of one. Level 13
/// on an Earth at 0.1 m voxels (820 m cells), level 2 on a 3 km world.
/// None on a plane.
fn exact_level(grid: &Grid) -> u32 {
    if grid.is_plane() {
        return u32::MAX;
    }
    (0..grid.levels())
        .find(|&level| {
            let cell = grid.level_size(level);
            512.0 * cell / grid.radius() >= 0.05
                && cell >= 50.0 * grid.radius() * f64::from(f32::EPSILON)
        })
        .unwrap_or(grid.levels())
}

/// Cells an exact chunk's own cells reach past its frame's box (its raster
/// box grows by them): the grid's points over its occupied layers in its
/// frame, beyond the box, and half a cell.
fn reach_past(grid: &Grid, tile: Footprint, cz: i32, frame: &Frame, z: (f32, f32)) -> f32 {
    let s = f64::from(1u32 << tile.level);
    let mut excess = 0.0f64;
    for a in 0..=8 {
        for b in 0..=8 {
            for k in [z.0, z.1] {
                let local = DVec3::new(f64::from(a * 8), f64::from(b * 8), f64::from(k));
                let at = grid.position(
                    tile.face,
                    [
                        (f64::from(tile.cx * CHUNK) + local.x) * s,
                        (f64::from(tile.cy * CHUNK) + local.y) * s,
                        (f64::from(cz * CHUNK) + local.z) * s,
                    ],
                );
                let f = frame.inv * (at - frame.origin);
                excess = excess
                    .max(-f.x)
                    .max(f.x - 64.0)
                    .max(-f.y)
                    .max(f.y - 64.0)
                    .max(f64::from(z.0) - f.z)
                    .max(f.z - f64::from(z.1));
            }
        }
    }
    (excess + 0.5) as f32
}

/// Chunk `cz` of `tile` in `slot` as drawing reads it, with occupied layers
/// `z` and its frame made about height `about` (cells in the chunk); at an
/// exact level (from `exact`) how far its cells reach past the frame's box
/// (m.x_axis.w).
fn chunk_static(
    grid: &Grid,
    tile: Footprint,
    cz: i32,
    slot: u32,
    z: (f32, f32),
    about: f32,
    exact: u32,
) -> ChunkStatic {
    let frame = chunk_frame(grid, tile, cz, f64::from(about));
    let (m, inv) = (frame.m.as_mat3(), frame.inv.as_mat3());
    let (origin_hi, origin_lo) = split(frame.origin);
    let flags = u32::from(frame.m.determinant() < 0.0) | (tile.level << 8);
    let grow = if tile.level >= exact {
        reach_past(grid, tile, cz, &frame, z)
    } else {
        0.0
    };
    ChunkStatic {
        m: [
            m.x_axis.extend(grow).to_array(),
            m.y_axis.extend(0.0).to_array(),
            m.z_axis.extend(0.0).to_array(),
        ],
        inv: [
            inv.row(0).extend(0.0).to_array(),
            inv.row(1).extend(0.0).to_array(),
            inv.row(2).extend(0.0).to_array(),
        ],
        origin_hi,
        origin_lo,
        bounds: [z.0, z.1, f32::from_bits(slot), f32::from_bits(flags)],
    }
}

/// An eye and, on each face over whose half-space it is, its continuous
/// index coordinates and the grid's axes there (metres per index step):
/// planning measures tiles near it in that linear frame (the inverse
/// mapping costs two arctangents a call; LOD needs no more than the frame's
/// accuracy), farther ones from their centres.
pub struct Eye {
    pub position: DVec3,
    faces: [Option<([f64; 3], [DVec3; 3])>; 6],
    /// Metres across the face the linear frame holds (a sphere curves
    /// d^2 / 2R from it: 1 % at R / 64; seen from orbit a face spans
    /// thousands of kilometres).
    reach: f64,
}

impl Eye {
    pub fn new(grid: &Grid, position: DVec3) -> Self {
        let mut faces = [None; 6];
        let cells = f64::from(grid.cells());
        for &face in grid.faces() {
            if let Some(c) = grid.face_coords(face, position) {
                let at = [c[0].clamp(0.0, cells), c[1].clamp(0.0, cells), c[2]];
                let p0 = grid.position(face, at);
                let axis = |a: usize| {
                    let mut q = at;
                    q[a] += 1.0;
                    grid.position(face, q) - p0
                };
                faces[face as usize] = Some((c, [axis(0), axis(1), axis(2)]));
            }
        }
        Self {
            position,
            faces,
            reach: if grid.is_plane() {
                f64::INFINITY
            } else {
                grid.radius() / 64.0
            },
        }
    }

    /// From the eye to the nearest point of the tile's columns between
    /// layers `range` (level cells), in the eye's linear frame of the
    /// tile's face; `None` when the eye is not over that face or the tile
    /// lies beyond the frame's reach.
    fn offset(&self, tile: Footprint, range: (i32, i32)) -> Option<DVec3> {
        let (e, axes) = self.faces[tile.face as usize]?;
        let s = f64::from(1u32 << tile.level);
        let (i0, j0) = (
            f64::from(tile.cx * CHUNK) * s,
            f64::from(tile.cy * CHUNK) * s,
        );
        let w = f64::from(CHUNK) * s;
        let d = [
            e[0].clamp(i0, i0 + w) - e[0],
            e[1].clamp(j0, j0 + w) - e[1],
            e[2].clamp(f64::from(range.0) * s, f64::from(range.1) * s) - e[2],
        ];
        let across = axes[0] * d[0] + axes[1] * d[1];
        (across.length() <= self.reach).then(|| across + axes[2] * d[2])
    }

    /// How far the eye may move before measuring the tile switches between
    /// [`Self::offset`]'s linear frame and its centre (its nearest point
    /// across the face meets the frame's reach): infinite where it cannot
    /// (planes; faces the eye is not over, whose tiles near it lie past the
    /// reach). Layers do not matter: the reach is across the face.
    fn reach_margin(&self, tile: Footprint, _range: (i32, i32)) -> f64 {
        let Some((e, axes)) = self.faces[tile.face as usize].filter(|_| self.reach.is_finite())
        else {
            return f64::INFINITY;
        };
        let s = f64::from(1u32 << tile.level);
        let (i0, j0) = (
            f64::from(tile.cx * CHUNK) * s,
            f64::from(tile.cy * CHUNK) * s,
        );
        let w = f64::from(CHUNK) * s;
        let across =
            axes[0] * (e[0].clamp(i0, i0 + w) - e[0]) + axes[1] * (e[1].clamp(j0, j0 + w) - e[1]);
        (across.length() - self.reach).abs()
    }

    /// From the eye towards the tile's columns between layers `range`:
    /// [`Self::offset`] near it, else to their centre.
    fn toward(&self, grid: &Grid, tile: Footprint, range: (i32, i32)) -> DVec3 {
        self.offset(tile, range).unwrap_or_else(|| {
            let s = f64::from(1u32 << tile.level);
            let middle = (f64::from(tile.cx * CHUNK) + f64::from(CHUNK) / 2.0) * s;
            let across = (f64::from(tile.cy * CHUNK) + f64::from(CHUNK) / 2.0) * s;
            grid.position(
                tile.face,
                [middle, across, f64::from(range.0 + range.1) * s / 2.0],
            ) - self.position
        })
    }
}

/// Distance from `eye` to the tile's columns between layers `range` (level
/// cells): to the nearest point when the eye is over the tile's face and
/// near it ([`Eye::offset`]), else to its centre less its half-diagonal.
fn tile_distance(grid: &Grid, tile: Footprint, range: (i32, i32), eye: &Eye) -> f64 {
    if let Some(offset) = eye.offset(tile, range) {
        return offset.length();
    }
    let s = f64::from(1u32 << tile.level);
    let (i0, j0) = (
        f64::from(tile.cx * CHUNK) * s,
        f64::from(tile.cy * CHUNK) * s,
    );
    let w = f64::from(CHUNK) * s;
    let (k0, k1) = (f64::from(range.0) * s, f64::from(range.1) * s);
    let c = grid.position(tile.face, [i0 + w / 2.0, j0 + w / 2.0, (k0 + k1) / 2.0]);
    let corner = grid.position(tile.face, [i0, j0, k0]);
    ((c - eye.position).length() - (corner - c).length()).max(0.0)
}

fn loaded(view: &wgpu::TextureView) -> Option<wgpu::RenderPassColorAttachment<'_>> {
    Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        },
    })
}

/// The footprint of volume `volume` a baked column lies in (a chunk is
/// eight bricks across).
fn column_footprint(volume: u16, column: ColumnKey) -> Footprint {
    Footprint {
        volume,
        face: column.face,
        level: u32::from(column.level),
        cx: column.ci >> 3,
        cy: column.cj >> 3,
    }
}

/// The analytic brushes of `edits` resolved per face: the large log's,
/// then the recent log's; the count of large ones, and each recent brush's
/// face count.
/// Which edits a world shows: brushes applied, the history's hash, the
/// baked state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EditState {
    len: usize,
    hash: u64,
    sealed: crate::planet::BakedState,
}

impl EditState {
    fn of(edits: &Edits) -> Self {
        Self {
            len: edits.len(),
            hash: edits.hash(),
            sealed: edits.baked_state(),
        }
    }
}

/// A frame's generation jobs and what they upload.
#[derive(Default)]
struct GenBatch {
    /// Footprints whose columns the batch computes, then those it reads from
    /// the pool; a chunk job's `tile` is an index in the first, or with
    /// `CACHED` in the second (resolved at upload).
    computed: Vec<TileJob>,
    cached: Vec<TileJob>,
    chunk_jobs: Vec<ChunkJob>,
    refs: Vec<u32>,
    /// Baked brick tables (512 a job: `BrickCache` slots).
    baked_refs: Vec<u32>,
    /// `cell_materials` slots taken.
    details: u32,
    /// Restore jobs the batch carries too (their results follow its jobs').
    restored: usize,
}

/// Foliage tiles one ground probe answers at most (`ChunkWorld::probe_ground`): a frame's
/// placement budget, with room.
const PROBE_TILES: usize = 1024;

/// Metres over a tile's highest terrain top its ground probes start their walks.
const PROBE_MARGIN_M: f64 = 4.0;

/// Ground changes kept for foliage between takes (`ChunkWorld::take_ground_changes`).
const GROUND_CHANGES: usize = 16_384;

/// Node `key`'s box in world (planet) coordinates: its eight corners' bounds.
fn node_box(grid: &Grid, key: NodeKey) -> [DVec3; 2] {
    let side = f64::from(CHUNK) * f64::from(1u32 << key.level.min(31));
    let lo = [
        f64::from(key.cx) * side,
        f64::from(key.cy) * side,
        f64::from(key.cz) * side,
    ];
    let mut b = [DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)];
    for corner in 0..8 {
        let p = grid.position(
            key.face,
            [
                lo[0] + side * f64::from(corner & 1),
                lo[1] + side * f64::from((corner >> 1) & 1),
                lo[2] + side * f64::from((corner >> 2) & 1),
            ],
        );
        b = [b[0].min(p), b[1].max(p)];
    }
    b
}

/// The world box (axis-aligned) holding box `b` of a volume placed by
/// `transform`.
fn world_box(b: [DVec3; 2], transform: &crate::volume::VolumeTransform) -> [DVec3; 2] {
    if transform.is_identity() {
        return b;
    }
    let mut out = [DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)];
    for corner in 0..8 {
        let p = DVec3::new(
            if corner & 1 == 0 { b[0].x } else { b[1].x },
            if corner & 2 == 0 { b[0].y } else { b[1].y },
            if corner & 4 == 0 { b[0].z } else { b[1].z },
        );
        let w = transform.to_world(p);
        out = [out[0].min(w), out[1].max(w)];
    }
    out
}

/// `ChunkJob::tile` of a footprint the batch reads from the pool.
const CACHED: u32 = 1 << 31;

impl GenBatch {
    fn footprints(&self) -> usize {
        self.computed.len() + self.cached.len()
    }

    /// The table of `store`'s bricks under chunk `cz` of `tile` (their
    /// `cache` slots for `frame`), or NONE.
    fn baked_table(
        &mut self,
        cache: &mut BrickCache,
        frame: u64,
        store: &EditStore,
        tile: Footprint,
        cz: i32,
    ) -> u32 {
        let mut table = [u32::MAX; 512];
        let mut any = false;
        for by in 0..8 {
            for bx in 0..8 {
                let key = ColumnKey {
                    face: tile.face,
                    level: tile.level as u8,
                    ci: tile.cx * 8 + bx,
                    cj: tile.cy * 8 + by,
                };
                let Some(column) = store.column(&key) else {
                    continue;
                };
                for (bk, brick) in column.iter() {
                    let bz = bk - cz * 8;
                    if !(0..8).contains(&bz) {
                        continue;
                    }
                    table[((by * 8 + bx) * 8 + bz) as usize] = cache.entry(brick, frame);
                    any = true;
                }
            }
        }
        if !any {
            return u32::MAX;
        }
        let n = self.baked_refs.len() as u32;
        self.baked_refs.extend_from_slice(&table);
        n
    }
}

/// Baked bricks on the GPU across batches (`baked_cells`), 256 words a
/// slot: a brick is uploaded once while it stays in use, so regenerating
/// chunks under heavily edited ground costs their tables, not their bricks
/// again (a seal's regenerated chunks sent 5.5 MB of bricks a frame). Keyed
/// by the brick's address; the cache holds the brick, so the address names
/// it while cached. A brick of one edit (a large dig's inside) is its table
/// entry (`UNIFORM_BRICK`), no slot. Slots not used this frame are reused
/// least recently used first once `limit` are taken (the buffer grows to
/// them, and past them when one frame wants more).
struct BrickCache {
    entries: FxHashMap<usize, (u32, std::sync::Arc<Brick>, u64)>,
    free: Vec<u32>,
    /// Slots handed out; the buffer holds `buffer_slots`.
    slots: u32,
    limit: u32,
    buffer: wgpu::Buffer,
    buffer_slots: u32,
    /// Bricks given slots since the last upload.
    pending: Vec<(u32, std::sync::Arc<Brick>)>,
}

/// Bytes a cached brick takes.
const BRICK_BYTES: u64 = 1024;

impl BrickCache {
    fn new(device: &wgpu::Device, limit: u32) -> Self {
        let buffer_slots = 4096.min(limit.max(1));
        Self {
            entries: FxHashMap::default(),
            free: Vec::new(),
            slots: 0,
            limit,
            buffer: Self::buffer(device, buffer_slots),
            buffer_slots,
            pending: Vec::new(),
        }
    }

    fn buffer(device: &wgpu::Device, slots: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk baked bricks"),
            size: u64::from(slots) * BRICK_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    /// `brick`'s table entry for `frame`: its slot (uploaded at the next
    /// [`Self::upload`] when new), or its uniform edit.
    fn entry(&mut self, brick: &std::sync::Arc<Brick>, frame: u64) -> u32 {
        let key = std::sync::Arc::as_ptr(brick) as usize;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.2 = frame;
            return entry.0;
        }
        let entry = if brick.cells.iter().all(|c| *c == brick.cells[0]) {
            UNIFORM_BRICK | u32::from(brick.cells[0].0)
        } else {
            let slot = self
                .free
                .pop()
                .or_else(|| {
                    (self.slots >= self.limit)
                        .then(|| self.evict(frame))
                        .flatten()
                })
                .unwrap_or_else(|| {
                    self.slots += 1;
                    self.slots - 1
                });
            self.pending.push((slot, brick.clone()));
            slot
        };
        self.entries.insert(key, (entry, brick.clone(), frame));
        entry
    }

    /// The least recently used quarter of the entries not used this frame
    /// dropped; one of the slots they free.
    fn evict(&mut self, frame: u64) -> Option<u32> {
        let mut old: Vec<(u64, usize)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.2 < frame)
            .map(|(k, e)| (e.2, *k))
            .collect();
        if old.is_empty() {
            return None;
        }
        old.sort_unstable();
        for &(_, key) in &old[..(old.len() / 4).max(1)] {
            if let Some((entry, ..)) = self.entries.remove(&key) {
                if entry & UNIFORM_BRICK == 0 {
                    self.free.push(entry);
                }
            }
        }
        self.free.pop()
    }

    /// The new bricks written, after growing the buffer to every slot
    /// handed out (its old contents copied first, then the new bricks
    /// copied over them in the same order).
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
    ) {
        let words = |brick: &Brick| {
            brick
                .cells
                .chunks(2)
                .map(|p| u32::from(p[0].0) | (u32::from(p[1].0) << 16))
                .collect::<Vec<u32>>()
        };
        if self.slots <= self.buffer_slots {
            // Runs of consecutive slots in one write each.
            self.pending.sort_unstable_by_key(|(slot, _)| *slot);
            let mut at = 0;
            while at < self.pending.len() {
                let first = self.pending[at].0;
                let run = self.pending[at..]
                    .iter()
                    .enumerate()
                    .take_while(|(n, (slot, _))| *slot == first + *n as u32)
                    .count();
                let data: Vec<u32> = self.pending[at..at + run]
                    .iter()
                    .flat_map(|(_, brick)| words(brick))
                    .collect();
                queue.write_buffer(
                    &self.buffer,
                    u64::from(first) * BRICK_BYTES,
                    bytemuck::cast_slice(&data),
                );
                at += run;
            }
            self.pending.clear();
            return;
        }
        let slots = self.slots.next_power_of_two();
        let buffer = Self::buffer(device, slots);
        encoder.copy_buffer_to_buffer(
            &self.buffer,
            0,
            &buffer,
            0,
            u64::from(self.buffer_slots) * BRICK_BYTES,
        );
        if !self.pending.is_empty() {
            let data: Vec<u32> = self
                .pending
                .iter()
                .flat_map(|(_, brick)| words(brick))
                .collect();
            let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk baked bricks upload"),
                contents: bytemuck::cast_slice(&data),
                usage: wgpu::BufferUsages::COPY_SRC,
            });
            for (n, (slot, _)) in self.pending.drain(..).enumerate() {
                encoder.copy_buffer_to_buffer(
                    &staging,
                    n as u64 * BRICK_BYTES,
                    &buffer,
                    u64::from(slot) * BRICK_BYTES,
                    BRICK_BYTES,
                );
            }
        }
        self.buffer = buffer;
        self.buffer_slots = slots;
    }
}

/// The edits of `planet` as classification and generation read them;
/// `bricks`, the index of its baked store when that is unchanged.
/// The analytic brushes `planet` shows and where they are (`EditView`):
/// the large layer's faces, then the recent brushes'. From an earlier view
/// (`from`), its large faces (shared runs) and their index are extended
/// while the large layer only extends them (seals append; an unseal or
/// another history starts again), so an edit costs the recent brushes, and
/// a seal its own large faces. Whether it extended `from`.
fn edit_view(
    planet: &Planet,
    version: u64,
    from: Option<&EditView>,
    bricks: std::sync::Arc<classify::BrickIndex>,
) -> (EditView, bool) {
    let edits = planet.edits();
    let large = &edits.large;
    let n = large.len();
    let prefix = |count: usize| {
        if count == 0 {
            0
        } else {
            large.prefix_hash(count as u32 - 1)
        }
    };
    let faces_of = |ids: std::ops::Range<usize>| {
        ids.flat_map(|id| large.resolved(id as u32).faces.iter().copied())
            .collect::<Vec<FaceBrush>>()
    };
    let from = from.filter(|v| v.large_brushes <= n && prefix(v.large_brushes) == v.large_prefix);
    let (mut faces, mut index, large_reach, extended) = match from {
        Some(v) => (v.faces.clone(), v.index.clone(), v.large_reach, true),
        None => Default::default(),
    };
    let new_large = faces_of(from.map_or(0, |v| v.large_brushes)..n);
    let large_reach = classify::faces_reach(large_reach, &new_large);
    let indexed = faces.large_len();
    faces.extend_large(new_large);
    index.extend(&faces, indexed);
    let large_faces = faces.large_len();
    let mut recent = Vec::new();
    let mut recent_counts = Vec::new();
    for id in 0..edits.recent.len() as u32 {
        let resolved = &edits.recent.resolved(id).faces;
        recent.extend(resolved.iter().copied());
        recent_counts.push(resolved.len());
    }
    let recent_index = classify::BrushIndex::new(&recent, large_faces as u32, false);
    let reach = classify::faces_reach(large_reach, &recent);
    faces.set_recent(recent);
    let view = EditView {
        grid: *planet.grid(),
        faces,
        index,
        recent_index,
        reach,
        large_reach,
        large_faces,
        large_brushes: n,
        large_prefix: prefix(n),
        recent_counts,
        baked: edits.baked.clone(),
        bricks,
        version,
        volume_depth: (i64::from(planet.volume_bounds().0) / i64::from(planet.grid().layer_mm())
            + 1) as i32,
    };
    (view, extended)
}

/// A brush buffer for `faces` with room for twice as many (at least 64):
/// edits write their tail into it (`ChunkWorld::write_brushes`).
fn brush_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    faces: &classify::Faces,
) -> (wgpu::Buffer, usize) {
    let capacity = (faces.len() * 2).max(64);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chunk brushes"),
        size: (capacity * std::mem::size_of::<FaceBrush>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if faces.is_empty() {
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&NO_BRUSH[..]));
    }
    write_faces(queue, &buffer, faces, 0);
    (buffer, capacity)
}

/// `faces` from `from` on written into the brush buffer.
fn write_faces(queue: &wgpu::Queue, buffer: &wgpu::Buffer, faces: &classify::Faces, from: usize) {
    for (at, run) in faces.slices_from(from) {
        queue.write_buffer(
            buffer,
            (at * std::mem::size_of::<FaceBrush>()) as u64,
            bytemuck::cast_slice(run),
        );
    }
}

/// A batch's restore jobs (slot, pack offset, result index, flags), their
/// packs, and each restored node with its slot and serial.
#[derive(Default)]
struct Restores {
    jobs: Vec<[u32; 4]>,
    data: Vec<u32>,
    nodes: Vec<(NodeKey, u32, u64)>,
}

/// A generated node's chunk.
struct Resident {
    slot: u32,
    /// Generation it belongs to (matches its readback).
    serial: u64,
    /// Occupied layers, once read back.
    z: Option<(i32, i32)>,
    /// Whether it was ever drawn (diagnostics: nodes evicted unseen were
    /// generated for nothing).
    shown: bool,
    /// Whether edits are over it (its chunk is kept when evicted), and its
    /// last job result's flags.
    edited: bool,
    flags: i32,
    /// Its footprint's measurement, once read back: kept with its chunk, so
    /// a restore classifies its footprint as the generation did.
    measured: Option<Quarters>,
}

/// An edited chunk kept across eviction: its packed tree (`pack_main`; none:
/// it is empty), its job result's flags, its footprint's measurement, the
/// frame it was last restored or kept.
struct Kept {
    pack: std::sync::Arc<[u32]>,
    flags: i32,
    measured: Option<Quarters>,
    used: u64,
}

/// Kept chunks out of a world ([`ChunkWorld::kept_chunks`]) for the edits
/// that made them: what a save stores ([`Self::to_bytes`]) and a new world
/// for those edits restores ([`ChunkWorld::load_kept`]).
#[derive(Clone, Debug)]
pub struct KeptChunks {
    /// Of the generation shaders, terrain and world (`ChunkWorld`'s).
    fingerprint: u64,
    /// The history's length and hash.
    history: (u64, u64),
    /// Each node, its result flags, its footprint's measurement and its
    /// pack, in node order.
    chunks: Vec<(NodeKey, i32, Option<Quarters>, std::sync::Arc<[u32]>)>,
}

impl KeptChunks {
    /// Chunks held.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// As bytes: a header (magic, format, fingerprint, the history's length
    /// and hash, the chunk count), then each chunk (node, result flags, its
    /// pack's length; its footprint's measured quarters: a byte of those
    /// present, bit 4 for any, then 16 words; its pack's words) deflated,
    /// then a checksum of it all.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&KEPT_MAGIC);
        out.extend_from_slice(&KEPT_FORMAT.to_le_bytes());
        for v in [
            self.fingerprint,
            self.history.0,
            self.history.1,
            self.chunks.len() as u64,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let mut body = Vec::new();
        for (key, flags, measured, pack) in &self.chunks {
            body.push(key.face);
            body.push(key.level as u8);
            for v in [key.cx, key.cy, key.cz, *flags, pack.len() as i32] {
                body.extend_from_slice(&v.to_le_bytes());
            }
            let quarters = measured.unwrap_or_default();
            body.push(
                quarters
                    .iter()
                    .enumerate()
                    .fold(u8::from(measured.is_some()) << 4, |m, (q, r)| {
                        m | (u8::from(r.is_some()) << q)
                    }),
            );
            for r in quarters {
                let r = r.unwrap_or(Ranges {
                    volume: (0, 0),
                    surface: (0, 0),
                });
                for v in [r.volume.0, r.volume.1, r.surface.0, r.surface.1] {
                    body.extend_from_slice(&v.to_le_bytes());
                }
            }
            body.extend_from_slice(bytemuck::cast_slice(pack));
        }
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(&body, 6));
        let sum = crate::record::fnv64(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// From [`Self::to_bytes`]; refused when they are not kept chunks of
    /// this pack format, or damaged.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 46 || bytes[..4] != KEPT_MAGIC {
            return Err("not kept chunks".into());
        }
        let (body, sum) = bytes.split_at(bytes.len() - 8);
        if crate::record::fnv64(body) != u64::from_le_bytes(sum.try_into().unwrap()) {
            return Err("kept chunks: checksum".into());
        }
        let format = u16::from_le_bytes(body[4..6].try_into().unwrap());
        if format != KEPT_FORMAT {
            return Err(format!("kept chunks of pack format {format}"));
        }
        let u64_at = |n: usize| u64::from_le_bytes(body[6 + 8 * n..14 + 8 * n].try_into().unwrap());
        let (fingerprint, history, count) = (u64_at(0), (u64_at(1), u64_at(2)), u64_at(3));
        let body = miniz_oxide::inflate::decompress_to_vec(&body[38..])
            .map_err(|e| format!("kept chunks: {e:?}"))?;
        let mut at = 0;
        let mut take = |n: usize| -> Result<&[u8], String> {
            let out = body.get(at..at + n).ok_or("kept chunks: cut short")?;
            at += n;
            Ok(out)
        };
        let mut chunks = Vec::with_capacity(count.min(1 << 20) as usize);
        for _ in 0..count {
            let head = take(22)?;
            let i32_at =
                |n: usize| i32::from_le_bytes(head[2 + 4 * n..6 + 4 * n].try_into().unwrap());
            let key = NodeKey {
                volume: 0,
                face: head[0],
                level: u32::from(head[1]),
                cx: i32_at(0),
                cy: i32_at(1),
                cz: i32_at(2),
            };
            let (flags, len) = (i32_at(3), i32_at(4).max(0) as usize);
            let present = take(1)?[0];
            let words = take(64)?;
            let word = |n: usize| i32::from_le_bytes(words[4 * n..4 * n + 4].try_into().unwrap());
            let quarters: Quarters = std::array::from_fn(|q| {
                (present >> q & 1 != 0).then(|| Ranges {
                    volume: (word(4 * q), word(4 * q + 1)),
                    surface: (word(4 * q + 2), word(4 * q + 3)),
                })
            });
            let measured = (present & 16 != 0).then_some(quarters);
            let pack: std::sync::Arc<[u32]> = take(len * 4)?
                .chunks(4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .collect();
            chunks.push((key, flags, measured, pack));
        }
        if at != body.len() {
            return Err("kept chunks: trailing data".into());
        }
        Ok(Self {
            fingerprint,
            history,
            chunks,
        })
    }
}

/// A frame's packs on their way back: each node's slot in the directory
/// and its result flags.
struct PackReadback {
    buffer: helio_core::gpu_cache::PooledBuffer,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Each node, its result flags, its footprint's measurement, and its
    /// serial when packed resident.
    nodes: Vec<(NodeKey, i32, Option<Quarters>, Option<u64>)>,
}

/// What the traces did (counting builds).
#[derive(Clone, Copy, Debug)]
pub struct TraceCounts {
    /// Pixels traced, tree steps, hits, boxes crossed.
    pub seeded: u32,
    pub steps: u32,
    pub hits: u32,
    pub boxes: u32,
    /// Rays that ended without a hit, the boxes they crossed, and those
    /// that left the drawn cut.
    pub missed: u32,
    pub missed_boxes: u32,
    pub lost: u32,
    /// A lost ray's pixel, the last one (none: 0, 0 never written).
    pub lost_pixel: (u32, u32),
    /// Places found lost (any walk, shadows' too), by `lost_at`'s reason.
    pub lost_sites: [u32; 7],
    /// Walks that ended on the box of a Solid node and of a Buried one
    /// (rock where no chunk was generated), the last of each's pixel and
    /// node (table key: face | level << 8, cx, cy, cz).
    pub rock_ends: [u32; 2],
    pub rock_pixels: [(u32, u32); 2],
    pub rock_nodes: [[i32; 4]; 2],
}

/// A frame's GPU time (milliseconds).
#[derive(Clone, Copy, Debug)]
pub struct FrameTimings {
    pub frame: u64,
    /// The draw in all (seeds, traces), its first seed; then sun shadows.
    pub draw: f64,
    pub seed: f64,
    pub shadow: f64,
    /// The shadows' parts: sun samples, sky samples, every hit's sun (and
    /// the fallbacks' walks), local lights, the GI probes' rays.
    pub shadow_stages: [f64; 5],
    /// Of every hit's sun: the fallbacks' walks.
    pub fallbacks: f64,
    /// The sun's occlusion clipmap's level drawn this frame.
    pub occlusion: f64,
    /// The shadows' denoiser (from the shadows' end: a pass's start
    /// timestamp may come before the work ahead of it is done).
    pub denoise: f64,
    /// The update's eviction and generation, and the tiles and chunks it
    /// generated.
    pub generate: f64,
    pub tiles: usize,
    pub chunks: usize,
    /// Generation by kernel: eviction and tile columns, chunk columns,
    /// chunk builds.
    pub stages: [f64; 3],
    /// The first stage's parts: eviction and the tiles' nodes, their
    /// columns, their volumes (caves, extents), their features' placements.
    pub tile_stages: [f64; 4],
}

/// Timestamps a frame writes: generation start, tile volumes end, seed start
/// and end, trace end, chunk columns end, chunk builds end, shadows end,
/// tile nodes end, tile columns end, tile features end, the shadows'
/// denoiser end; the shadows' parts: sun samples end, sky samples end,
/// every hit's sun end, local lights end (the GI rays end at shadows end);
/// the sun's occlusion start and end; every hit's own sun end (before the
/// fallbacks' walks).
const QUERIES: u32 = 19;

/// `Buried`'s size: a count and 4095 keys.
const BURIED_BYTES: u64 = 16 * 4096;

/// Ray reports (`feedback`): a pixel of each 2x2 a frame, every pixel over
/// `REPORT_SWEEP` frames; a workgroup's 32x32 pixels' distinct wanted
/// nodes, up to one a block of 8x8 pixels in all. The reports' buffer: a
/// count, then the keys (up to 4096x4096 pixels).
const REPORT_SWEEP: u64 = 4;
const FEEDBACK_BLOCK: u32 = 8;
const FEEDBACK_BYTES: u64 =
    16 + 16 * (4096 / FEEDBACK_BLOCK as u64) * (4096 / FEEDBACK_BLOCK as u64);
/// Frames of ray reports a plan must have seen since residency last
/// changed (and fades ended) before it counts as settled: those in flight
/// when it changed (a readback slot each), then a sweep of the new cut.
const REPORT_FRAMES: u64 = TIMING_RING + REPORT_SWEEP;

/// Picks a frame answers (more wait a frame).
const MAX_PICKS: usize = 8;

/// A frame's pick hits on their way back: per pick 256 bytes, its
/// visibility (8) then distance (at 16).
struct PickSlot {
    buffer: wgpu::Buffer,
    /// Request ids, the inbox they are answered into, and the state as
    /// for `Buried`.
    requests: Vec<u64>,
    sink: Option<std::sync::Arc<crate::engine::VolumeAnswers>>,
    state: ReachState,
}

/// A copy of a frame's ray reports (`Buried`, or the feedback's) on its
/// way back: its bytes.
struct ReachSlot {
    buffer: wgpu::Buffer,
    state: ReachState,
    bytes: u64,
}

/// Map the report copies just submitted; read (`read`) and free those
/// mapped.
fn take_reports(slots: &mut [ReachSlot], mut read: impl FnMut(&[u8])) {
    for slot in slots {
        match &slot.state {
            ReachState::Copied => {
                let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let flag = ready.clone();
                slot.buffer
                    .slice(..slot.bytes)
                    .map_async(wgpu::MapMode::Read, move |r| {
                        if r.is_ok() {
                            flag.store(true, std::sync::atomic::Ordering::Release);
                        }
                    });
                slot.state = ReachState::Mapping(ready);
            }
            ReachState::Mapping(ready) if ready.load(std::sync::atomic::Ordering::Acquire) => {
                read(&slot.buffer.slice(..slot.bytes).get_mapped_range().unwrap());
                slot.buffer.unmap();
                slot.state = ReachState::Free;
            }
            _ => {}
        }
    }
}

enum ReachState {
    Free,
    /// Copied in a frame not yet submitted.
    Copied,
    Mapping(std::sync::Arc<std::sync::atomic::AtomicBool>),
}

/// A frame's timestamps on their way back.
struct TimingSlot {
    buffer: wgpu::Buffer,
    /// Frame, tiles and chunks written, once resolved; mapping when
    /// `ready` is set.
    frame: Option<(u64, usize, usize)>,
    ready: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// One frame's generation results on their way back from the GPU.
struct Readback {
    buffer: helio_core::gpu_cache::PooledBuffer,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    footprints: Vec<Footprint>,
    /// Chunk jobs in order: node, slot, serial.
    chunks: Vec<(NodeKey, u32, u64)>,
    /// Leaf pool growths when recorded.
    leaf_growths: u32,
}

/// What [`ChunkWorld::update`] did.
#[derive(Clone, Copy, Debug, Default)]
pub struct UpdateStats {
    /// Nodes waiting to be generated.
    pub requests: usize,
    /// Footprints whose columns were computed, footprints read from the
    /// pool, and chunks generated.
    pub generated_footprints: usize,
    pub cached_footprints: usize,
    pub generated_chunks: usize,
    pub evicted_chunks: usize,
    /// Evicted chunks that were never drawn, and generated chunks drawn for
    /// the first time (diagnostics: what generation was for).
    pub evicted_unseen: usize,
    pub shown_chunks: usize,
    /// Generated chunks that held nothing to draw (what classification
    /// could not rule out).
    pub emptied_chunks: usize,
    /// Edited chunks the plan evicted left resident past the frame's packs
    /// (evicted, kept, by a later plan).
    pub evictions_deferred: usize,
    /// Drawn nodes (Solid and Buried ones too), those fading in or out of
    /// a switch between levels, and drawn chunks.
    pub drawn_nodes: usize,
    pub fading_nodes: usize,
    pub drawn_chunks: usize,
    pub resident_chunks: usize,
    /// Ray reports since the last update (each a node a 32x32 tile's rays
    /// met: the planner decodes them); the distinct nodes among them, in a
    /// counting build (`Capacity::counters`, else 0: counting them costs a
    /// hash a report on the render thread); Buried nodes rays met
    /// (generated next).
    pub reports: usize,
    pub seen: usize,
    pub reached: usize,
    /// Drawn Mixed nodes in view coarser than wanted, as a share of them.
    pub coarse_share: f64,
    /// Frames since the eye the plan in use was made for, and the worker's
    /// milliseconds on it (and of those its events, plans and evictions:
    /// `residency::Plans::parts`) and the nodes it visited; whether it
    /// arrived this frame.
    pub plan_age: u64,
    pub plan_ms: f64,
    pub plan_parts: [f64; 3],
    pub plan_visits: usize,
    pub planned: bool,
    /// [`residency::Plan::worst_px`] of the latest plan, and its node;
    /// [`residency::Plan::worst_wanted`].
    pub worst_px: f64,
    pub worst_wanted: f64,
    pub worst: Option<residency::Worst>,
    /// Level switches in view a plan that arrived this frame made, and
    /// [`residency::Plan::switch_px`].
    pub switches: usize,
    pub switch_px: f64,
    /// Chunks this frame could generate.
    pub budget: usize,
    /// CPU milliseconds: results, plan, eviction, generation, drawn set,
    /// table.
    pub cpu_ms: [f64; 6],
    pub out_of_slots: bool,
    pub out_of_leaves: bool,
    /// A chunk kept its leaves' material pairs for want of detail pages.
    pub out_of_details: bool,
    /// Detail pages in use when last read back, of the capacity.
    pub detail_pages: (u64, u64),
    /// The pools' sizes now (they grow on demand): chunk slots, and leaf
    /// pages in use when last read back of the leaf pool's.
    pub chunk_slots: usize,
    pub leaf_pages: (u64, u64),
    /// CPU milliseconds the pools' growth took this frame (buffers made
    /// and their copies submitted).
    pub grow_ms: f64,
    /// Edited chunks restored by copy this frame, those generated (from
    /// the field and the edits over them), and those kept.
    pub restored_chunks: usize,
    pub generated_edited: usize,
    pub kept_chunks: usize,
    pub kept_bytes: usize,
}

/// How [`ChunkWorld::update`] plans and how much it generates a frame.
#[derive(Clone, Copy, Debug)]
pub struct ResidencyConfig {
    pub plan: residency::PlanConfig,
    /// Footprints generated per frame at most.
    pub footprints_per_frame: usize,
    /// GPU milliseconds of generation a frame at 60 frames a second
    /// (against the measured cost of tiles and chunks); `ChunkPass` gives a
    /// slower frame proportionally more, up to twice.
    pub generate_ms: Option<f64>,
    /// Milliseconds a frame may take: `ChunkPass` lets generation grow into
    /// what the last measured frame left of them on the GPU and the CPU, by
    /// half a millisecond a frame at most, up to three times `generate_ms`
    /// (nodes wait only while the frame is full).
    pub frame_ms: Option<f64>,
}

impl Default for ResidencyConfig {
    /// Cells of a pixel or less where rays met them in the last 90 frames
    /// (two in view where none did, two out of view past a 20 degree guard;
    /// up to three where the eye's motion sweeps the image 32 pixels a
    /// frame; a 45 degree view 1080 pixels tall), 400 km out, half a second
    /// of prefetch, buried nodes generated where they hold the eye, chunks
    /// unvisited for 600 frames released; up to 256 footprints and 3 ms of
    /// generation a frame.
    fn default() -> Self {
        let release_frames = 600;
        Self {
            plan: residency::PlanConfig {
                pixel_angle: 2.0 * std::f64::consts::FRAC_PI_8.tan() / 1080.0,
                lod_pixels: 1.0,
                offscreen_pixels: 2.0,
                view_guard: 0.35,
                unseen_pixels: 2.0,
                seen_frames: 90,
                coarsen: 0.8,
                max_distance: 400_000.0,
                prefetch_seconds: 0.5,
                keep_frames: 120,
                release_frames,
                motion_flow: 16.0,
                motion_max: 3.0,
                near_cells: 2.0,
            },
            footprints_per_frame: 256,
            generate_ms: Some(3.0),
            frame_ms: Some(16.0),
        }
    }
}

impl ResidencyConfig {
    /// A host's [`crate::settings::VOXEL_RESIDENCY`] values (the pixel
    /// angle is the pass's: it follows the view).
    pub fn from_settings(settings: &helio_settings::Resolved) -> Self {
        debug_assert!(std::ptr::eq(
            settings.group(),
            &crate::settings::VOXEL_RESIDENCY
        ));
        let n = |key| settings.number(key);
        let defaults = Self::default();
        Self {
            plan: residency::PlanConfig {
                pixel_angle: defaults.plan.pixel_angle,
                lod_pixels: n("lod_pixels"),
                offscreen_pixels: n("offscreen_pixels"),
                view_guard: n("view_guard"),
                unseen_pixels: n("unseen_pixels"),
                seen_frames: n("seen_frames") as _,
                coarsen: n("coarsen"),
                max_distance: n("view_distance_km") * 1000.0,
                prefetch_seconds: n("prefetch_seconds"),
                keep_frames: n("keep_frames") as _,
                release_frames: n("release_frames") as _,
                motion_flow: n("motion_flow"),
                motion_max: n("motion_max"),
                near_cells: n("near_cells"),
            },
            footprints_per_frame: n("footprints_per_frame") as usize,
            generate_ms: settings.optional("generate_ms"),
            frame_ms: settings.optional("frame_ms"),
        }
    }
}

#[cfg(test)]
#[test]
fn default_settings_are_the_default_residency() {
    let configured = ResidencyConfig::from_settings(&crate::settings::VOXEL_RESIDENCY.defaults());
    assert_eq!(
        format!("{configured:?}"),
        format!("{:?}", ResidencyConfig::default())
    );
}

/// Publishing with the host's scene materials (`scene`; each form's
/// pipeline is its `DrawPipelines::publish_scene`).
struct SceneState {
    layout: wgpu::BindGroupLayout,
    shore_params_layout: wgpu::BindGroupLayout,
    /// Entries of the texture table the pipeline was built for.
    capacity: u32,
    map: wgpu::Buffer,
    /// Bound when the frame has no water fields. `state.x` disables swash.
    _swash_fallback_texture: wgpu::Texture,
    swash_fallback_view: wgpu::TextureView,
    swash_fallback_params: wgpu::Buffer,
    /// The map the buffer holds.
    map_written: Option<scene::VoxelMaterialMap>,
    /// The bind group of the inputs' version.
    group: std::sync::Mutex<Option<((u64, u64, bool), wgpu::BindGroup, wgpu::BindGroup)>>,
}

/// What [`ChunkWorld::sync_edits`] did with a volume's edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditSync {
    /// Followed from the edits it had (or unchanged).
    Followed,
    /// Another history: every resident node generated again in place.
    Resynced,
}

/// Chunk trees of a scene's voxel volumes (`volumes`): one chunk pool, one
/// planner and one tracer for every placed world, each volume's chunks
/// keyed by it, generated by its own terrain program and drawn in its own
/// frame.
pub struct ChunkWorld {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// What the world holds (volumes' footprint pools are sized from it).
    capacity: Capacity,
    /// The device has 64-bit integers (generation's wide products).
    wide64: bool,
    /// Generation's layouts, shared by every volume's pipelines.
    gen_layout: wgpu::BindGroupLayout,
    keep_layout: wgpu::BindGroupLayout,
    gen_pipeline_layout: wgpu::PipelineLayout,
    keep_pipeline_layout: wgpu::PipelineLayout,
    /// Generation pipelines by their source's fingerprint: volumes of one
    /// terrain program and form share them.
    gen_cache: FxHashMap<u64, std::sync::Weak<volumes::GenPipelines>>,
    layouts: volumes::DrawLayouts,
    /// Drawing's pipelines for spheres and planes, built when a volume of
    /// that form comes.
    pipelines: [Option<volumes::DrawPipelines>; 2],
    /// The UV reference image of debug mode 1 (trace and publish).
    uv_grid_view: wgpu::TextureView,
    uv_grid_sampler: wgpu::Sampler,
    /// The scene depth of a draw given none: no meshes (1x1, cleared).
    no_scene_depth: wgpu::TextureView,
    /// Bound when a draw has no scene lights (no light, empty lists).
    no_lights: wgpu::Buffer,
    /// Foliage ground probes (`chunks::ground`): the probe frame and the
    /// asked tiles' places.
    probe_frame: wgpu::Buffer,
    probe_tiles: wgpu::Buffer,
    /// World boxes whose drawn ground changed since the last
    /// [`ChunkWorld::take_ground_changes`], and nodes whose ground changes again
    /// when their switch's fade ends (at that frame).
    ground_changes: Vec<[DVec3; 2]>,
    /// World boxes of drawn chunks generated or dug in place, and of chunks
    /// drawn finer, since the last [`ChunkWorld::draw_sun_occlusion`]: the
    /// sun's occlusion pages whose rays cross them are traced again (at
    /// once; in the background).
    occluder_changes: Vec<[DVec3; 2]>,
    occluder_refined: Vec<[DVec3; 2]>,
    /// CPU time of the last update's occluder boxes (ms; `SunPageStats`).
    occluder_ms: f64,
    ground_later: Vec<(u64, NodeKey)>,
    /// The GI probes' rays (`gi_trace`), from each probe's base cell
    /// (`gi_cells`, and its CPU copy) in the clipmap they were located for,
    /// in the primary volume.
    gi_cells: wgpu::Buffer,
    gi_cell_data: Vec<[i32; 4]>,
    gi_clipmap: Option<helio_gi_api::Clipmap>,
    /// The publish with the host's scene materials (devices with texture
    /// binding arrays).
    scene: Option<SceneState>,
    /// Whether every hit walks its own sun ray (`set_steady_shadows`).
    steady_shadows: bool,
    /// The sun's angular radius (radians), and the host's atmosphere frame
    /// (helio-pass-sky's `atmosphere_frame`) whose sun disk wins when given.
    sun_angular_radius: f32,
    atmosphere_frame: Option<wgpu::Buffer>,
    /// The last draw's bind groups: each traced volume's, nearest first.
    last_groups: Option<DrawGroups>,
    /// The sun's occlusion by the terrain, once drawn (`draw_sun_occlusion`).
    sun_occlusion: Option<SunOcclusion>,
    targets: Option<Targets>,
    /// The primary trace's repeats a draw (`set_trace_rounds`: timing it).
    rounds: u32,
    /// The pixel whose boxes counting builds record (`set_debug_pixel`).
    debug_pixel: Option<[u32; 2]>,
    /// A batch's jobs and scratch, shared by the volumes (one generates a
    /// frame).
    tile_jobs: wgpu::Buffer,
    /// The batch's two-rate nodes (`tile_nodes` of the generation shaders),
    /// for the largest program of the volumes.
    tile_node_data: wgpu::Buffer,
    slope_lattice: wgpu::Buffer,
    // The batch's chunk jobs' columns features changed (a bit each).
    job_features: wgpu::Buffer,
    tile_ranges: wgpu::Buffer,
    chunk_jobs: wgpu::Buffer,
    column_masks: wgpu::Buffer,
    chunk_meta: wgpu::Buffer,
    mids: wgpu::Buffer,
    mid_leaves: wgpu::Buffer,
    leaves: wgpu::Buffer,
    leaf_alloc: wgpu::Buffer,
    column_materials: wgpu::Buffer,
    leaf_materials: wgpu::Buffer,
    // Each stored leaf's ground normal (two bytes, as `leaf_materials`).
    leaf_normals: wgpu::Buffer,
    mid_materials: wgpu::Buffer,
    /// The camera's parameters in the world frame (the denoiser's).
    params: wgpu::Buffer,
    /// Chunk keys by slot, and the chunk table of the drawn chunks.
    chunk_keys: wgpu::Buffer,
    chunk_table: wgpu::Buffer,
    statics: wgpu::Buffer,
    draw_list: wgpu::Buffer,
    /// The drawn chunks' slots (uploaded when they change), the cull's
    /// counts (`Cull`), each drawn chunk's bin, and the seed's indirect draws.
    drawn_slots: wgpu::Buffer,
    cull_state: wgpu::Buffer,
    cull_bins: wgpu::Buffer,
    seed_args: wgpu::Buffer,
    /// Each volume's motion since the last draw, as the last frame's
    /// projection of its points (`motions` of chunk_draw.wgsl: the
    /// denoiser's reprojection), by volume index.
    motions: wgpu::Buffer,
    /// GPU timestamps of the frames, read back without waiting.
    queries: Option<(wgpu::QuerySet, wgpu::Buffer)>,
    timing_slots: Vec<TimingSlot>,
    /// This frame's slot, when one is free.
    timing_now: Option<usize>,
    timings: Vec<FrameTimings>,
    /// Fragment, step and hit counts (`HELIO_VOXEL_CHUNKS_COUNT`).
    counters: wgpu::Buffer,
    /// Buried nodes rays met (`Buried`) and its readback slots.
    buried: wgpu::Buffer,
    buried_slots: Vec<ReachSlot>,
    /// Ray reports (`feedback`): the nodes sampled hits want, its readback
    /// slots.
    feedback: wgpu::Buffer,
    feedback_slots: Vec<ReachSlot>,
    /// Whether draws write the plain colour target (a world drawn alone:
    /// tests, harnesses); a host's gbuffer takes the published surface.
    plain_colour: bool,
    /// The renderer-wide debug mode, for the voxel debug shading.
    debug_mode: u32,
    /// Built with trace counters (`Capacity::counters`).
    counting: bool,
    pick_slots: Vec<PickSlot>,
    /// Each slot's chunk key (`chunk_keys`, mirrored).
    slot_keys: Vec<[i32; 4]>,
    /// Ray reports since the last update: each frame's table keys of the
    /// nodes met, as read back (the planner decodes them,
    /// `residency::Event::Reports`); Buried nodes met.
    seen: Vec<std::sync::Arc<[[i32; 4]]>>,
    reached: Vec<NodeKey>,
    /// Frames of ray reports read and not yet sent to the planner, and the
    /// planner's count of them when residency last changed or a fade last
    /// ran: settled takes plans that saw `REPORT_FRAMES` of them since.
    reports_unsent: u64,
    quiet_from: u64,
    /// Chunk table changes scattered (`chunk_table.wgsl`), uploaded into
    /// their own buffer (a count, then the changes).
    table_layout: wgpu::BindGroupLayout,
    table_pipeline: wgpu::ComputePipeline,
    table_changes: wgpu::Buffer,
    /// The frame's small writes as one upload (`FrameUploads`: a copy each
    /// where it is written, one queue write a call that records).
    uploads: helio_core::upload::FrameUploads,
    /// Chunks' frames and keys written this frame (generated, restored,
    /// read back), by slot (the last write of a slot wins): uploaded at once
    /// into `records_buffer` and scattered into `statics` and `chunk_keys`
    /// (`chunk_records.wgsl`).
    records: Vec<[[i32; 4]; RECORD_VEC4S]>,
    record_at: FxHashMap<u32, usize>,
    records_layout: wgpu::BindGroupLayout,
    records_pipeline: wgpu::ComputePipeline,
    records_buffer: wgpu::Buffer,
    pack_data: wgpu::Buffer,
    pack_alloc: wgpu::Buffer,
    pack_readbacks: std::collections::VecDeque<PackReadback>,
    pack_pending: Option<PackReadback>,
    kept: FxHashMap<NodeKey, Kept>,
    /// Nodes packed this frame or the last, not edited since (their packs
    /// are kept when they come back).
    packed_since: FxHashSet<NodeKey>,
    kept_bytes: usize,
    kept_limit: usize,
    /// Resident edited chunks to pack without evicting them
    /// (`keep_resident`).
    to_pack: Vec<NodeKey>,
    /// The free leaf pages (a stack, `leaf_alloc` its count) and each
    /// slot's pages.
    free_pages: wgpu::Buffer,
    leaf_pages: wgpu::Buffer,
    job_results: wgpu::Buffer,
    /// Chunk slots and leaves the pools hold now, and at most
    /// (`Capacity::chunks`); leaf pool growths so far (free pages read back
    /// from before the latest are stale).
    slot_capacity: usize,
    max_chunks: usize,
    leaf_capacity: u64,
    leaf_max: u64,
    leaf_growths: u32,
    /// The leaf pool's free count raised when it grows (`chunk_pool.wgsl`).
    pool_layout: wgpu::BindGroupLayout,
    pool_pipeline: wgpu::ComputePipeline,
    /// Per-voxel materials (`chunk_generate.wgsl`): a batch's edited cell
    /// materials, each leaf page's detail page, the pages, the stack of
    /// those given back, and the allocator (the stack's count, the
    /// overflow flag, pages ever taken, the capacity).
    cell_materials: wgpu::Buffer,
    detail_pages: wgpu::Buffer,
    detail_cells: wgpu::Buffer,
    detail_alloc: wgpu::Buffer,
    detail_free: wgpu::Buffer,
    /// Per slot: whether any of its leaf pages has a detail page.
    slot_details: wgpu::Buffer,
    /// Detail pages the pool holds now, and at most (`Capacity`).
    detail_capacity: u64,
    detail_max: u64,
    /// Pages in use when last read back, and whether a build found the
    /// pool (of that capacity) full since.
    detail_used_seen: u64,
    detail_overflow_seen: Option<u64>,
    /// Chunks built without some detail pages (their leaves draw their
    /// pairs): generated again once the pool grows.
    detail_starved: FxHashSet<NodeKey>,
    /// Free leaf pages when last read back.
    free_pages_seen: u64,
    /// The volumes, by index (`VolumeId`); the primary one (foliage's
    /// ground, GI probes); those its planner does not know yet; nodes of
    /// removed ones still drawn.
    volumes: Vec<Option<volumes::Volume>>,
    primary: Option<u16>,
    /// Each volume's anchor this draw (`place_anchors`), by index.
    anchors: Vec<u32>,
    /// Where each volume but the primary was at the last update (foliage's
    /// ground changes where one near the eye moves).
    ground_placed: FxHashMap<u16, crate::volume::VolumeTransform>,
    unplanned: Vec<u16>,
    removed_drawn: Vec<NodeKey>,
    /// Every volume's planner, on its thread.
    worker: residency::Worker,
    /// The volume generating next: volumes with work take turns.
    gen_turn: usize,
    /// What the latest plans were made for (structural events and frames of
    /// ray reports they saw), the frame they were made for and the worker's
    /// milliseconds on them.
    planned: Option<(u64, u64)>,
    plan_made: (u64, f64, [f64; 3], usize),
    /// Footprints whose columns are on the GPU, and this update's frame.
    frame_now: u64,
    /// Generated nodes with a chunk, and their layers by footprint.
    nodes: FxHashMap<NodeKey, Resident>,
    footprint_nodes: FxHashMap<Footprint, Vec<i32>>,
    /// Resident nodes by volume index.
    held: FxHashMap<u16, usize>,
    /// Slots of chunks dropped between updates (edits), freed by the next.
    dropped: Vec<u32>,
    free_slots: Vec<u32>,
    serial: u64,
    readbacks: std::collections::VecDeque<Readback>,
    /// The results' and packs' `MAP_READ` buffers, reused.
    readback_pool: helio_core::gpu_cache::ReadbackPool,
    /// The bind groups, views and placeholder buffers the world's passes
    /// bind, kept across its frames (`submitted` ends one).
    gpu_cache: helio_core::gpu_cache::GpuCache,
    /// This frame's drawn nodes.
    drawn_set: drawn::DrawnSet,
    /// Nodes whose chunks changed since the drawn set last saw them.
    changed_nodes: FxHashSet<NodeKey>,
    /// The drawn set's version whose slots `drawn_slots` holds.
    slots_version: Option<u64>,
    /// Results recorded by the last update, mapped once submitted.
    pending: Option<Readback>,
    /// GPU cost of generation: measured (moving averages) from
    /// [`GenCost::PRIOR`].
    cost: GenCost,
    /// Footprints and chunks the last update generated.
    last_generated: (usize, usize),
    /// Bumped whenever a volume moves or is added or removed (the sun's
    /// occlusion maps go stale), and the planner configuration volumes are
    /// planned with (the last update's).
    motion_version: u64,
}

/// A draw's bind groups (`ChunkWorld::last_groups`): each traced volume's,
/// nearest first, with its index; the volumes foliage's probes walk past
/// the primary (`probe_ground`), their groups for it.
#[derive(Clone)]
struct DrawGroups {
    volumes: Vec<(u16, wgpu::BindGroup, [u32; 4])>,
    ground: Vec<(u16, wgpu::BindGroup)>,
}

/// A volume that lights the other volumes' points in a draw
/// (`ChunkWorld::cross_volumes`): its bind group (the camera's view, over
/// `shadow`'s rectangle), the rectangle its sun shadow can fall on another
/// volume's hits in (none: it falls on none), whether the GI probes' rays
/// reach it and foliage's probes may meet it.
struct CrossVolume {
    index: u16,
    group: wgpu::BindGroup,
    shadow: Option<[u32; 4]>,
    gi: bool,
    ground: bool,
}

/// How far from the eye (m) foliage's probes may meet a volume besides the
/// primary: its bands' reach (`helio_pass_foliage_place`, 320 m) and a
/// largest tile.
const FOLIAGE_REACH: f64 = 512.0;

/// What generation costs on the GPU: milliseconds of a tile's columns and
/// of a chunk's columns and tree on average, chunks a tile, and the lines
/// through each frame's batch (a batch costs a fixed part, then each tile
/// and chunk: the averages of the small batches a flight makes hold mostly
/// that fixed part, and would budget a few times too few tiles).
#[derive(Clone, Copy, Debug)]
struct GenCost {
    tile: f64,
    chunk: f64,
    chunks_per_tile: f64,
    tiles: CostLine,
    chunks: CostLine,
}

impl GenCost {
    /// Before the first measured frame: an RTX 3060's (a 300 m/s cruise and
    /// a 50 m blast).
    const PRIOR: Self = Self {
        tile: 0.035,
        chunk: 0.007,
        chunks_per_tile: 1.7,
        tiles: CostLine::EMPTY,
        chunks: CostLine::EMPTY,
    };

    /// A batch's fixed milliseconds, then a tile's and a chunk's: the lines'
    /// once their batches spread enough, else the averages.
    fn model(&self) -> (f64, f64, f64) {
        let (fixed_t, tile) = self.tiles.fit().unwrap_or((0.0, self.tile));
        let (fixed_c, chunk) = self.chunks.fit().unwrap_or((0.0, self.chunk));
        (fixed_t + fixed_c, tile, chunk)
    }
}

/// A least-squares line through (items, milliseconds) samples, recent ones
/// weighing more: what a batch costs, fixed and per item.
#[derive(Clone, Copy, Debug)]
struct CostLine {
    w: f64,
    x: f64,
    y: f64,
    xx: f64,
    xy: f64,
}

impl CostLine {
    const EMPTY: Self = Self {
        w: 0.0,
        x: 0.0,
        y: 0.0,
        xx: 0.0,
        xy: 0.0,
    };

    fn add(&mut self, x: f64, y: f64) {
        const KEEP: f64 = 0.98;
        *self = Self {
            w: self.w * KEEP + 1.0,
            x: self.x * KEEP + x,
            y: self.y * KEEP + y,
            xx: self.xx * KEEP + x * x,
            xy: self.xy * KEEP + x * y,
        };
    }

    /// Fixed and per-item milliseconds, once enough batches of spread
    /// sizes were seen (a line through a size alone is anything).
    fn fit(&self) -> Option<(f64, f64)> {
        if self.w < 8.0 {
            return None;
        }
        let (mx, my) = (self.x / self.w, self.y / self.w);
        let var = self.xx / self.w - mx * mx;
        if var < (0.25 * mx).powi(2).max(4.0) {
            return None;
        }
        let per = (self.xy / self.w - mx * my) / var;
        let fixed = my - per * mx;
        // A fixed part under zero: the plain average, per item.
        if per <= 0.0 || fixed < 0.0 {
            return (mx > 0.0).then(|| (0.0, my / mx));
        }
        Some((fixed, per))
    }
}

/// The draw's targets: the seed (each pixel's nearest box) and the trace's
/// outputs.
pub struct Targets {
    pub size: [u32; 2],
    seed: wgpu::Texture,
    /// The seed's box entries (Depth32Float, reversed: `seed_box`).
    pub depth: wgpu::Texture,
    /// Chunk slot (u32::MAX: none) and cell, face (Rg32Uint).
    pub visibility: wgpu::Texture,
    /// Plain colour (Rgba8Unorm).
    pub colour: wgpu::Texture,
    /// Distance from the eye to the hit, 0 for none (R32Float).
    pub distance: wgpu::Texture,
    /// Where each ray left its last box without a hit (R32Float).
    exits: wgpu::Texture,
    /// The sun's visibility at each hit, denoised, and the sun's direction
    /// (Rgba16Float, Helio's `directional_visibility`), and its view.
    pub sun: wgpu::Texture,
    sun_view: wgpu::TextureView,
    /// The share of each pixel's light its sun samples give stochastically
    /// (helio-pass-tsr's `tsr_stochastic`; r, the rest unused).
    stochastic: wgpu::Texture,
    stochastic_view: wgpu::TextureView,
    /// Each pixel's sun as the walks found it, 0 or 1; whether it is its
    /// own walk's (1) or a neighbour's (0); whether a terrain hit is shown
    /// (bytes of a word a pixel, rows of the frame).
    pub sun_walked: wgpu::Buffer,
    /// The sky's visibility at each hit, denoised, for the gbuffer
    /// (R32Float).
    pub sky: wgpu::Texture,
    /// The local lights each pixel's walks see past the terrain: two words
    /// a pixel, row by row: bit i the tile list's light i; 1 where they are
    /// its own walks', 0 a neighbour's.
    local_seen: wgpu::Buffer,
    /// The denoised local lights (`LocalTargets`), made with the first
    /// draw that has lights.
    local: Option<LocalTargets>,
    /// A local lights' sample a 2x2 block: word, distance.
    local_samples: wgpu::Buffer,
    /// The normal each hit is lit with and its sky sample (Rgba16Float;
    /// zero where no terrain hit is shown).
    pub normal: wgpu::Texture,
    /// The denoiser's history, this frame's and the last in turn
    /// (`denoised`'s parity), two words a pixel (rows of the frame): the
    /// sun's mean, the sky's mean and second moment, the sky's frames
    /// accumulated; the short histories, the sun's samples' weight
    /// accumulated (halves); the hits they are for: view depth, packed
    /// normal.
    history: [wgpu::Buffer; 2],
    history_short: [wgpu::Buffer; 2],
    geometry: [wgpu::Buffer; 2],
    right_history: Option<[wgpu::Buffer; 2]>,
    right_history_short: Option<[wgpu::Buffer; 2]>,
    right_geometry: Option<[wgpu::Buffer; 2]>,
    /// The filters' sun and sky and their variance (halves), in turn: the
    /// history's, the first pass's, the second's.
    filtered: [wgpu::Buffer; 2],
    /// Frames denoised into these targets.
    denoised: [u64; 2],
    /// A sun sample a 2x2 block: flags, distance, slope (Rgba32Uint).
    sun_samples: wgpu::Texture,
    /// A sky visibility sample a 4x4 block: visibility, distance
    /// (Rg32Float).
    sky_samples: wgpu::Texture,
    /// Hits no sample serves: count, pixels; their indirect workgroups.
    fallbacks: wgpu::Buffer,
    fallback_groups: wgpu::Buffer,
}

/// The denoised local lights: Helio's `local_visibility` (three words a
/// pixel: the tracked lights' tile slots, their visibility, a byte each;
/// the other slots' bits) and the tracked lights' history, this frame's
/// and the last in turn (four words a pixel).
pub struct LocalTargets {
    pub visibility: wgpu::Buffer,
    history: [wgpu::Buffer; 2],
    right_history: Option<[wgpu::Buffer; 2]>,
    /// The local lights' passes' indirect dispatch, and whether the scene
    /// had local lights this frame and the last (`local_gate`).
    dispatch: wgpu::Buffer,
}

impl LocalTargets {
    fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let pixels = u64::from(size[0]) * u64::from(size[1]);
        let buffer = |label: &str, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: pixels * bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let history = [
            buffer("chunk local light history", 16),
            buffer("chunk local light history", 16),
        ];
        let dispatch = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk local light dispatch"),
            size: 20,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
            mapped_at_creation: false,
        });
        Self {
            visibility: buffer("chunk local lights", 12),
            history,
            right_history: None,
            dispatch,
        }
    }

    fn ensure_right_history(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.right_history.is_some() {
            return;
        }
        let pixels = u64::from(size[0]) * u64::from(size[1]);
        let buffer = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: pixels * 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        self.right_history = Some([
            buffer("chunk local light history right"),
            buffer("chunk local light history right"),
        ]);
    }
}

const SEED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

impl Targets {
    fn trace(&self) -> TraceTextures<'_> {
        TraceTextures {
            seed: &self.seed,
            depth: &self.depth,
            visibility: &self.visibility,
            colour: &self.colour,
            distance: &self.distance,
            exits: &self.exits,
        }
    }

    fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let texture = |label: &str, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: usage | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let a = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let s = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        let sun = texture("chunk sun", wgpu::TextureFormat::Rgba16Float, s);
        let stochastic = texture("chunk stochastic sun", wgpu::TextureFormat::Rgba8Unorm, s);
        let pixels = u64::from(size[0]) * u64::from(size[1]);
        let buffer = |label: &str, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: pixels * bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let pair = |label: &str| [buffer(label, 8), buffer(label, 8)];
        Self {
            size,
            seed: texture("chunk seed", SEED_FORMAT, a),
            depth: texture("chunk depth", DEPTH_FORMAT, a),
            visibility: texture("chunk visibility", wgpu::TextureFormat::Rg32Uint, s),
            colour: texture("chunk colour", wgpu::TextureFormat::Rgba8Unorm, s),
            distance: texture("chunk distance", wgpu::TextureFormat::R32Float, s),
            exits: texture(
                "chunk exits",
                wgpu::TextureFormat::R32Float,
                s | wgpu::TextureUsages::RENDER_ATTACHMENT,
            ),
            sun_view: view(&sun),
            sun,
            stochastic_view: view(&stochastic),
            stochastic,
            sun_walked: buffer("chunk walked sun", 4),
            sky: texture("chunk sky", wgpu::TextureFormat::R32Float, s),
            local_seen: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chunk local lights walked"),
                size: u64::from(size[0]) * u64::from(size[1]) * 8,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            local: None,
            history: pair("chunk shadow history"),
            history_short: pair("chunk shadow short history"),
            geometry: pair("chunk shadow history hits"),
            right_history: None,
            right_history_short: None,
            right_geometry: None,
            filtered: pair("chunk shadow filtered"),
            denoised: [0; 2],
            local_samples: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chunk local light samples"),
                size: u64::from(size[0].div_ceil(2)) * u64::from(size[1].div_ceil(2)) * 8,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            normal: texture("chunk normal", wgpu::TextureFormat::Rgba16Float, s),
            sun_samples: device.create_texture(&wgpu::TextureDescriptor {
                label: Some("chunk sun samples"),
                size: wgpu::Extent3d {
                    width: size[0].div_ceil(2),
                    height: size[1].div_ceil(2),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Uint,
                usage: s,
                view_formats: &[],
            }),
            sky_samples: device.create_texture(&wgpu::TextureDescriptor {
                label: Some("chunk sky samples"),
                size: wgpu::Extent3d {
                    width: size[0].div_ceil(4),
                    height: size[1].div_ceil(4),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rg32Float,
                usage: s,
                view_formats: &[],
            }),
            fallbacks: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chunk sun fallbacks"),
                size: 4 + 4 * u64::from(size[0]) * u64::from(size[1]),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            fallback_groups: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chunk sun fallback workgroups"),
                size: 12,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        }
    }

    fn ensure_right_history(&mut self, device: &wgpu::Device) {
        if self.right_history.is_some() {
            return;
        }
        let pixels = u64::from(self.size[0]) * u64::from(self.size[1]);
        let pair = |label: &str| {
            std::array::from_fn(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: pixels * 8,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })
            })
        };
        self.right_history = Some(pair("chunk shadow history right"));
        self.right_history_short = Some(pair("chunk shadow short history right"));
        self.right_geometry = Some(pair("chunk shadow history hits right"));
    }
}

fn view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&Default::default())
}

/// The entries of a chunk table for `slots` chunk slots: a power of two,
/// within what the device binds (`binding` bytes).
fn table_entries(slots: usize, binding: u64) -> usize {
    (slots * TABLE_PER_SLOT)
        .next_power_of_two()
        .min((binding / 32).next_power_of_two() as usize / 2)
}

/// `buffer` in a new one of `size` bytes (its usage, `label`), its contents
/// copied by `encoder`.
fn regrown(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    buffer: &wgpu::Buffer,
    label: &str,
    size: u64,
) -> wgpu::Buffer {
    let next = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: buffer.usage(),
        mapped_at_creation: false,
    });
    encoder.copy_buffer_to_buffer(buffer, 0, &next, 0, buffer.size());
    next
}

fn storage(
    binding: u32,
    read_only: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn entry(binding: u32, b: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: b.as_entire_binding(),
    }
}

fn view_entry(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(v),
    }
}

fn texture_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// The sun's occlusion atlas (`helio_light_api::sun_occlusion`).
fn sun_occlusion_maps_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// The UV reference image (debug mode 1: faces shaded by their UVs), a
/// repeating, filtered texture.
fn create_uv_grid(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::TextureView, wgpu::Sampler) {
    const UV_GRID_BYTES: &[u8] = include_bytes!("../../assets/UV_Grid_Sm.jpg");
    let decoded = image::load_from_memory(UV_GRID_BYTES)
        .expect("embedded UV reference image is valid")
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("voxel UV reference image"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        decoded.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("voxel UV reference sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    (texture.create_view(&Default::default()), sampler)
}

fn storage_texture(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// The chunk shaders as compiled for a world form and terrain program:
/// generation, then drawing (with diagnostic counters when `counting`).
/// The draw shader's walk templates (`// WALK {` .. `// } WALK`) twice:
/// with `walk` (`hit_walk`, `hit_lights`, `sky_visibility`), and with `walk_fine`
/// (`..._fine`: the kernels for hits finer than the exact levels).
fn walk_templates(source: &str) -> String {
    // Past a marker's line (checkouts may end lines with \r\n).
    let past_line =
        |text: &str, at: usize| text[at..].find('\n').map_or(text.len(), |n| at + n + 1);
    let mut out = String::with_capacity(source.len() * 2);
    let mut rest = source;
    while let Some(start) = rest.find("// WALK {") {
        let end = rest[start..]
            .find("// } WALK")
            .expect("closed walk template")
            + start;
        let body = &rest[past_line(rest, start)..end];
        out.push_str(&rest[..start]);
        out.push_str(body);
        out.push_str(
            &body
                .replace("walk(Walk(", "walk_fine(Walk(")
                .replace("hit_walk(", "hit_walk_fine(")
                .replace("fn hit_lights(", "fn hit_lights_fine(")
                .replace("fn sky_visibility(", "fn sky_visibility_fine("),
        );
        rest = &rest[past_line(rest, end)..];
    }
    out.push_str(rest);
    out
}

pub fn shader_sources(
    plane: bool,
    program: &crate::terrain::TerrainProgram,
    wide64: bool,
    counting: bool,
) -> [(&'static str, String); 2] {
    [
        (
            "chunk generation",
            generation_source(plane, program, wide64),
        ),
        ("chunk draw", draw_source(plane, counting)),
    ]
}

/// The generation shader of a world form and terrain program.
fn generation_source(
    plane: bool,
    program: &crate::terrain::TerrainProgram,
    wide64: bool,
) -> String {
    terrain_source(plane, program, wide64, true)
        + &format!(
            "const HALO_GROUPS: u32 = {}u;
",
            crate::engine::HALO_GROUPS
        )
        + &candidates::wgsl_constants()
        + include_str!("../../shaders/chunk_generate.wgsl")
}

/// The draw shader of a world form (with diagnostic counters when
/// `counting`): it reads no terrain program, so every volume of the form
/// shares it.
fn draw_source(plane: bool, counting: bool) -> String {
    // Counters only in a counting build: side effects in a fragment shader
    // can turn early depth tests off.
    // Primary rays count in the trace (`COUNT`), in their walks when
    // `counted` (`WCOUNT`; shadow walks do not).
    let count = |n: u32| {
        if counting {
            format!("atomicAdd(&counters[{n}], 1u);")
        } else {
            String::new()
        }
    };
    // The primary steps' heat (debug mode 32) counts in every build, in the
    // pipeline of that view (`STEP_HEAT`).
    let wcount = |n: u32| {
        let heat = format!("if STEP_HEAT && counted {{ heat += u32({n} == 1); }}");
        if counting {
            format!("if counted {{ atomicAdd(&counters[{n}], 1u); }} {heat}")
        } else {
            heat
        }
    };
    // The frame's prelude (its G-buffer tags), the noise library (materials'
    // patches), then the draw (from disk with `HELIO_SHADER_FROM_DISK`).
    let source = |rel: &str, embedded: &'static str| {
        helio_core::shader::hot::source_text(env!("CARGO_MANIFEST_DIR"), rel, embedded)
    };
    (helio_core::shader::PRELUDE.to_owned() + &format!("const DOMAIN_UNIT: f32 = {};\n", crate::grid::DOMAIN_UNIT) + &helio_core::shader::hot::source_text(helio_voxel_world::wgsl::DIR, helio_voxel_world::wgsl::NOISE_PATH, helio_voxel_world::wgsl::NOISE) + helio_gi_api::GI_COMMON_SNIPPET.source + helio_light_api::SUN_OCCLUSION_SNIPPET.source + helio_light_api::LIGHT_LIST_SNIPPET.source + helio_mats::MATERIAL_ROW_SNIPPET.source + helio_mats::PBR_EVAL_SNIPPET.source + helio_foliage_api::GROUND_SNIPPET.source + &walk_templates(&source("shaders/chunk_draw.wgsl", include_str!("../../shaders/chunk_draw.wgsl"))))
        .replace("COUNTERS", if counting { "@group(0) @binding(9) var<storage, read_write> counters: array<atomic<u32>, 256>;" } else { "" })
        .replace("WCOUNT(1)", &wcount(1))
        .replace("WCOUNT(3)", &wcount(3))
        .replace("WCOUNT(8)", &wcount(8))
        .replace("WCOUNT(10)", &wcount(10))
        // Lost places by reason (`lost_at`: counters 5, 6, 7, 11, 12, 13, 15).
        .replace("LOST_SITE", if counting { "atomicAdd(&counters[array<u32, 7>(5u, 6u, 7u, 11u, 12u, 13u, 15u)[site]], 1u);" } else { "" })
        // Where the last search past the top began: face | level << 8,
        // cell (counters[200..204]).
        .replace("LOST_WHERE", if counting { "atomicStore(&counters[200], u32(face | (level << 8u))); atomicStore(&counters[201], bitcast<u32>(cell.x)); atomicStore(&counters[202], bitcast<u32>(cell.y)); atomicStore(&counters[203], bitcast<u32>(cell.z));" } else { "" })
        // The last lost ray's pixel (counters[14]).
        .replace("LOST_PIXEL", if counting { "if counted { atomicStore(&counters[14], pixel.x | (pixel.y << 16u)); }" } else { "" })
        // Walks ending on a Solid (rock) or Buried node's box, the last
        // one's pixel and node (counters[240..250]).
        .replace("ROCK_END", if counting { "if counted { let b = u32(next.level == BURIED); atomicAdd(&counters[240u + b], 1u); atomicStore(&counters[242u + b], pixel.x | (pixel.y << 16u)); atomicStore(&counters[244u + 3u * b], u32(next.hi.x)); atomicStore(&counters[245u + 3u * b], bitcast<u32>(next.lo.x)); atomicStore(&counters[246u + 3u * b], bitcast<u32>(next.lo.y)); atomicStore(&counters[250u + b], bitcast<u32>(next.lo.z)); }" } else { "" })
        .replace("WCOUNT_BOXES", if counting { "if counted { atomicAdd(&counters[9], chunk); }" } else { "" })
        .replace("COUNT(0)", &count(0))
        .replace("COUNT(2)", &count(2))
        // Diagnostics (counting builds): the chunks one pixel's ray visits
        // (`HELIO_VOXEL_CHUNKS_DEBUG_PIXEL=x,y`), from counters[16].
        .replace(
            "TRACE_PIXEL(chunk, slot, cube_lo, real)",
            if counting {
                "if counted && all(pixel == params.debug.zw) && chunk < 24u { atomicStore(&counters[16u + chunk * 5u], slot); atomicStore(&counters[17u + chunk * 5u], bitcast<u32>(cube_lo.x)); atomicStore(&counters[18u + chunk * 5u], bitcast<u32>(cube_lo.y)); atomicStore(&counters[19u + chunk * 5u], bitcast<u32>(cube_lo.z)); atomicStore(&counters[20u + chunk * 5u], u32(real) + 2u * u32(key_level(key.x))); atomicStore(&counters[4], chunk + 1u); }"
            } else {
                ""
            },
        )
        .replace("FADE_FRAMES", &drawn::FADE_FRAMES.to_string())
        .replace("PLANE_WORLD", if plane { "true" } else { "false" })
        .replace("SCENE_MATERIALS", scene::STUB)
}

impl ChunkWorld {
    /// A world of the default [`Capacity`], without volumes yet
    /// ([`Self::add_volume`]).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self::with_capacity(device, queue, Capacity::default())
    }

    /// A world of one volume: `planet` in its own frame (the world's).
    pub fn of_planet(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        planet: &Planet,
        capacity: Capacity,
    ) -> Self {
        let mut world = Self::with_capacity(device, queue, capacity);
        world.add_volume(0, planet, crate::volume::VolumeTransform::IDENTITY);
        world
    }

    pub fn with_capacity(device: &wgpu::Device, queue: &wgpu::Queue, capacity: Capacity) -> Self {
        let wide64 = device.features().contains(wgpu::Features::SHADER_INT64);
        let c = wgpu::ShaderStages::COMPUTE;
        let gen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk generation"),
            entries: &[
                uniform(1, c),
                uniform(16, c),
                uniform(18, c),
                storage(2, true, c),
                storage(3, false, c),
                storage(4, false, c),
                storage(5, true, c),
                storage(6, false, c),
                storage(7, true, c),
                storage(8, true, c),
                storage(9, false, c),
                storage(10, false, c),
                storage(11, false, c),
                storage(12, false, c),
                storage(13, false, c),
                storage(14, false, c),
                storage(15, false, c),
                storage(17, false, c),
                storage(19, false, c),
                storage(20, false, c),
                storage(21, true, c),
                storage(22, true, c),
                storage(23, true, c),
                storage(24, false, c),
                storage(25, false, c),
                storage(26, false, c),
                storage(27, false, c),
                storage(28, false, c),
                storage(29, false, c),
                storage(30, false, c),
                storage(31, false, c),
                storage(32, false, c),
                storage(33, false, c),
                storage(34, false, c),
                storage(35, false, c),
                storage(36, true, c),
                storage(37, false, c),
                storage(38, false, c),
                storage(39, false, c),
                storage(40, true, c),
                storage(41, true, c),
            ],
        });
        let gen_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&gen_layout)],
            immediate_size: 0,
        });
        let keep_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk keep"),
            entries: &[
                storage(0, true, c),
                storage(1, false, c),
                storage(2, false, c),
                storage(3, true, c),
                storage(4, true, c),
            ],
        });
        let keep_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chunk keep"),
            bind_group_layouts: &[Some(&gen_layout), Some(&keep_layout)],
            immediate_size: 0,
        });
        let table_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunk table"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/chunk_table.wgsl").into()),
        });
        let table_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk table"),
            entries: &[storage(0, true, c), storage(1, false, c)],
        });
        let table_pipeline = helio_core::shader::create_compute_pipeline(
            device,
            wgpu::ComputePipelineDescriptor {
                label: Some("chunk table"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: None,
                        bind_group_layouts: &[Some(&table_layout)],
                        immediate_size: 0,
                    }),
                ),
                module: &table_module,
                entry_point: Some("table_update"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let records_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunk records"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/chunk_records.wgsl").into(),
            ),
        });
        let records_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk records"),
            entries: &[
                storage(0, true, c),
                storage(1, false, c),
                storage(2, false, c),
            ],
        });
        let records_pipeline = helio_core::shader::create_compute_pipeline(
            device,
            wgpu::ComputePipelineDescriptor {
                label: Some("chunk records"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: None,
                        bind_group_layouts: &[Some(&records_layout)],
                        immediate_size: 0,
                    }),
                ),
                module: &records_module,
                entry_point: Some("records_update"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let layouts = volumes::DrawLayouts::new(device);
        let (uv_grid_view, uv_grid_sampler) = create_uv_grid(device, queue);
        // Zeroed: the reversed depth's far, no mesh anywhere.
        let no_scene_depth = view(&device.create_texture(&wgpu::TextureDescriptor {
            label: Some("chunk no scene depth"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }));
        let probe_frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk probe frame"),
            size: std::mem::size_of::<ground::ProbeFrame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let probe_tiles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk probe tiles"),
            size: (PROBE_TILES * std::mem::size_of::<ground::ProbeTile>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // The publish textured by the host's scene materials, where the
        // device has texture binding arrays (its pipelines are a form's).
        let scene = scene::texture_capacity(device).map(|capacity| {
            let fallback_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Chunk Disabled Water Swash Fields"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 2,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let fallback_view = fallback_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Chunk Disabled Water Swash Fields View"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                base_array_layer: 0,
                array_layer_count: Some(2),
                ..Default::default()
            });
            let fallback_params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk Disabled Water Swash Parameters"),
                contents: bytemuck::cast_slice(&[0.0f32; 8]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            SceneState {
                layout: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("chunk scene materials"),
                    entries: &scene::layout_entries(capacity),
                }),
                shore_params_layout: scene::shore_params_layout(device),
                capacity,
                map: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("chunk scene material map"),
                    size: std::mem::size_of::<scene::VoxelMaterialMap>() as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                _swash_fallback_texture: fallback_texture,
                swash_fallback_view: fallback_view,
                swash_fallback_params: fallback_params,
                map_written: None,
                group: std::sync::Mutex::new(None),
            }
        });

        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let s = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        // The capacity within what the device binds: the largest per-slot
        // buffer is its mids (1 KiB), and the leaves are 8 bytes each.
        let limits = device.limits();
        let binding = u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        let max_chunks = capacity
            .chunks
            .min((binding / 1024) as usize)
            .min((binding / (LEAVES_PER_SLOT * 8)) as usize)
            .max(1024);
        let slots = max_chunks.min(CHUNKS_START);
        let leaf_capacity = slots as u64 * LEAVES_PER_SLOT;
        let detail_max = capacity
            .detail_pages
            .min((binding / (DETAIL_WORDS * 4)) as usize)
            .max(64) as u64;
        let detail_capacity = detail_max.min(DETAIL_PAGES_START);
        let table_slots = table_entries(slots, binding);
        let queries = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| {
                (
                    device.create_query_set(&wgpu::QuerySetDescriptor {
                        label: Some("chunk draw"),
                        ty: wgpu::QueryType::Timestamp,
                        count: QUERIES * TIMING_RING as u32,
                    }),
                    buffer(
                        "chunk timestamps",
                        u64::from(wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT) * TIMING_RING,
                        wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    ),
                )
            });
        let timing_slots = (0..if queries.is_some() { TIMING_RING } else { 0 })
            .map(|_| TimingSlot {
                buffer: buffer(
                    "chunk timestamps read",
                    u64::from(QUERIES) * 8,
                    wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                ),
                frame: None,
                ready: None,
            })
            .collect();
        let drawn_set = drawn::DrawnSet::new(slots, table_slots);
        let chunk_table = buffer("chunk table", (table_slots * 32) as u64, s);
        let pool_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunk pool"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/chunk_pool.wgsl").into()),
        });
        let pool_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk pool"),
            entries: &[storage(0, false, c), storage(1, true, c)],
        });
        let pool_pipeline = helio_core::shader::create_compute_pipeline(
            device,
            wgpu::ComputePipelineDescriptor {
                label: Some("chunk pool"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: None,
                        bind_group_layouts: &[Some(&pool_layout)],
                        immediate_size: 0,
                    }),
                ),
                module: &pool_module,
                entry_point: Some("pool_grow"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        // The pools' buffers are copied into larger ones when they grow.
        let g = s | wgpu::BufferUsages::COPY_SRC;
        queue.write_buffer(&chunk_table, 0, bytemuck::cast_slice(drawn_set.table()));
        Self {
            device: device.clone(),
            queue: queue.clone(),
            capacity,
            wide64,
            gen_layout,
            keep_layout,
            gen_pipeline_layout,
            keep_pipeline_layout,
            gen_cache: FxHashMap::default(),
            layouts,
            pipelines: [None, None],
            uv_grid_view,
            uv_grid_sampler,
            no_scene_depth,
            no_lights: buffer("chunk no lights", 128, wgpu::BufferUsages::STORAGE),
            probe_frame,
            probe_tiles,
            ground_changes: Vec::new(),
            occluder_changes: Vec::new(),
            occluder_refined: Vec::new(),
            occluder_ms: 0.0,
            ground_later: Vec::new(),
            gi_cells: buffer(
                "chunk GI probe cells",
                u64::from(helio_gi_api::PROBES) * 16,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            gi_cell_data: vec![[0; 4]; helio_gi_api::PROBES as usize],
            gi_clipmap: None,
            scene,
            steady_shadows: false,
            sun_angular_radius: EARTH_SUN_ANGULAR_RADIUS,
            atmosphere_frame: None,
            last_groups: None,
            sun_occlusion: None,
            targets: None,
            rounds: 1,
            debug_pixel: None,
            tile_jobs: buffer(
                "chunk tile jobs",
                (TILE_BATCH * std::mem::size_of::<TileJob>()) as u64,
                s,
            ),
            tile_node_data: buffer("chunk tile nodes", 16, wgpu::BufferUsages::STORAGE),
            slope_lattice: buffer(
                "chunk slope lattices",
                TILE_BATCH as u64 * SLOPE_LATTICE * 4,
                wgpu::BufferUsages::STORAGE,
            ),
            job_features: buffer(
                "chunk job features",
                CHUNK_BATCH as u64 * FEATURE_JOB_WORDS * 4,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            // Four words a tile, then the batch's tile count.
            tile_ranges: buffer(
                "chunk tile ranges",
                (TILE_BATCH * RANGE_WORDS * 4 + 4) as u64,
                s | wgpu::BufferUsages::COPY_SRC,
            ),
            chunk_jobs: buffer("chunk jobs", (CHUNK_BATCH * 32) as u64, s),
            column_masks: buffer(
                "chunk column masks",
                CHUNK_BATCH as u64 * TILE_COLUMNS * 8,
                wgpu::BufferUsages::STORAGE,
            ),
            chunk_meta: buffer("chunk meta", (slots * 48) as u64, g),
            mids: buffer("chunk mids", (slots * 64 * 16) as u64, g),
            mid_leaves: buffer("chunk mid leaves", (slots * 64 * 4) as u64, g),
            leaves: buffer("chunk leaves", leaf_capacity * 8, g),
            // Every page free: the stack's count, no overflow.
            leaf_alloc: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk leaf allocator"),
                contents: bytemuck::cast_slice(&[(leaf_capacity / LEAF_PAGE) as i32, 0]),
                usage: s | wgpu::BufferUsages::COPY_SRC,
            }),
            column_materials: buffer(
                "chunk column materials",
                CHUNK_BATCH as u64 * TILE_COLUMNS * 4,
                wgpu::BufferUsages::STORAGE,
            ),
            leaf_materials: buffer("chunk leaf materials", leaf_capacity * 2, g),
            leaf_normals: buffer("chunk leaf normals", leaf_capacity * 2, g),
            mid_materials: buffer("chunk mid materials", (slots * 64 * 4) as u64, g),
            params: buffer(
                "chunk draw params",
                std::mem::size_of::<DrawParams>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            chunk_keys: buffer("chunk keys", (slots * 16) as u64, g),
            chunk_table,
            statics: buffer(
                "chunk statics",
                (slots * std::mem::size_of::<ChunkStatic>()) as u64,
                g,
            ),
            draw_list: buffer("chunk draw list", (slots * 4) as u64, g),
            drawn_slots: buffer("chunk drawn slots", (slots * 4) as u64, g),
            cull_state: buffer("chunk cull", CULL_BYTES, s | wgpu::BufferUsages::COPY_SRC),
            cull_bins: buffer("chunk cull bins", (slots * 4) as u64, g),
            seed_args: buffer("chunk seed draws", 32, s | wgpu::BufferUsages::INDIRECT),
            motions: buffer(
                "chunk volume motions",
                (volumes::MAX_VOLUMES * 64) as u64,
                s,
            ),
            queries,
            timing_slots,
            timing_now: None,
            timings: Vec::new(),
            counters: buffer("chunk counters", 1024, s | wgpu::BufferUsages::COPY_SRC),
            buried: buffer(
                "chunk buried hits",
                BURIED_BYTES,
                s | wgpu::BufferUsages::COPY_SRC,
            ),
            plain_colour: true,
            debug_mode: 0,
            counting: capacity.counters,
            buried_slots: (0..TIMING_RING)
                .map(|_| ReachSlot {
                    buffer: buffer(
                        "chunk buried hits read",
                        BURIED_BYTES,
                        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    ),
                    state: ReachState::Free,
                    bytes: BURIED_BYTES,
                })
                .collect(),
            feedback: buffer(
                "chunk feedback",
                FEEDBACK_BYTES,
                s | wgpu::BufferUsages::COPY_SRC,
            ),
            feedback_slots: (0..TIMING_RING)
                .map(|_| ReachSlot {
                    buffer: buffer(
                        "chunk feedback read",
                        FEEDBACK_BYTES,
                        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    ),
                    state: ReachState::Free,
                    bytes: 16,
                })
                .collect(),
            seen: Vec::new(),
            reports_unsent: 0,
            quiet_from: 0,
            reached: Vec::new(),
            pick_slots: (0..3)
                .map(|_| PickSlot {
                    buffer: buffer(
                        "chunk picks",
                        256 * MAX_PICKS as u64,
                        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    ),
                    requests: Vec::new(),
                    sink: None,
                    state: ReachState::Free,
                })
                .collect(),
            slot_keys: vec![[-1, 0, 0, 0]; slots],
            table_layout,
            table_pipeline,
            table_changes: buffer("chunk table changes", 16 + 1024 * 48, s),
            uploads: helio_core::upload::FrameUploads::new(device, queue, "chunk uploads"),
            records: Vec::new(),
            record_at: FxHashMap::default(),
            records_layout,
            records_pipeline,
            records_buffer: buffer("chunk records", (16 + 256 * RECORD_BYTES) as u64, s),
            pack_data: buffer(
                "chunk packs",
                PACK_WORDS * 4,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            pack_alloc: buffer(
                "chunk pack directory",
                (1 + 2 * MAX_PACKS as u64) * 4,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            pack_readbacks: Default::default(),
            pack_pending: None,
            kept: FxHashMap::default(),
            packed_since: FxHashSet::default(),
            kept_bytes: 0,
            kept_limit: capacity.kept_bytes,
            to_pack: Vec::new(),
            free_pages: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk free pages"),
                contents: bytemuck::cast_slice(
                    &(0..(leaf_capacity / LEAF_PAGE) as u32).collect::<Vec<u32>>(),
                ),
                usage: g,
            }),
            leaf_pages: buffer("chunk leaf pages", slots as u64 * LEAF_PAGES * 4, g),
            job_results: buffer(
                "chunk job results",
                (CHUNK_BATCH * 16) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            slot_capacity: slots,
            max_chunks,
            leaf_capacity,
            leaf_max: max_chunks as u64 * LEAVES_PER_SLOT,
            leaf_growths: 0,
            pool_layout,
            pool_pipeline,
            cell_materials: buffer(
                "chunk cell materials",
                DETAIL_JOBS as u64 * TILE_COLUMNS * CELL_WORDS * 4,
                wgpu::BufferUsages::STORAGE,
            ),
            // No leaf page has a detail page yet.
            detail_pages: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk detail pages"),
                contents: bytemuck::cast_slice(&vec![
                    u32::MAX;
                    (leaf_capacity / LEAF_PAGE) as usize
                ]),
                usage: g,
            }),
            detail_cells: buffer(
                "chunk detail cells",
                detail_capacity * DETAIL_WORDS * 4,
                s | wgpu::BufferUsages::COPY_SRC,
            ),
            detail_alloc: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk detail allocator"),
                contents: bytemuck::cast_slice(&[0, 0, 0, detail_capacity as i32]),
                usage: s | wgpu::BufferUsages::COPY_SRC,
            }),
            detail_free: buffer(
                "chunk detail free pages",
                detail_capacity * 4,
                s | wgpu::BufferUsages::COPY_SRC,
            ),
            slot_details: buffer("chunk slot details", slots as u64 * 4, g),
            detail_capacity,
            detail_max,
            detail_used_seen: 0,
            detail_overflow_seen: None,
            detail_starved: FxHashSet::default(),
            free_pages_seen: leaf_capacity / LEAF_PAGE,
            volumes: Vec::new(),
            primary: None,
            anchors: Vec::new(),
            ground_placed: FxHashMap::default(),
            unplanned: Vec::new(),
            removed_drawn: Vec::new(),
            worker: residency::Worker::spawn(),
            gen_turn: 0,
            planned: None,
            plan_made: (0, 0.0, [0.0; 3], 0),
            frame_now: 0,
            nodes: Default::default(),
            footprint_nodes: Default::default(),
            held: Default::default(),
            dropped: Vec::new(),
            free_slots: (0..slots as u32).rev().collect(),
            serial: 0,
            readbacks: std::collections::VecDeque::new(),
            readback_pool: helio_core::gpu_cache::ReadbackPool::new("chunk results"),
            gpu_cache: helio_core::gpu_cache::GpuCache::new(),
            drawn_set,
            changed_nodes: Default::default(),
            slots_version: None,
            pending: None,
            cost: GenCost::PRIOR,
            last_generated: (0, 0),
            motion_version: 0,
        }
    }

    fn read<T: Pod + Default>(&self, source: &wgpu::Buffer, count: usize) -> Vec<T> {
        let size = (count * std::mem::size_of::<T>()) as u64;
        if size == 0 {
            return Vec::new();
        }
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(source, 0, &staging, 0, size);
        self.queue.submit([encoder.finish()]);
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let out = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
        out
    }

    /// The slot pool doubled (up to its most): every per-slot buffer copied
    /// into one twice the size, the new slots free, the chunk table larger
    /// with it (rebuilt from its mirror). The copies are submitted at once,
    /// before the update records anything: writes queued since land in the
    /// new buffers, after them.
    fn grow_slots(&mut self) {
        // The pools are replaced: nothing bound over the old ones stays.
        self.gpu_cache.clear();
        let old = self.slot_capacity;
        let slots = (old * 2).min(self.max_chunks);
        let device = self.device.clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        let per = |bytes: usize| (slots * bytes) as u64;
        let e = &mut encoder;
        self.chunk_meta = regrown(&device, e, &self.chunk_meta, "chunk meta", per(48));
        self.mids = regrown(&device, e, &self.mids, "chunk mids", per(64 * 16));
        self.mid_leaves = regrown(
            &device,
            e,
            &self.mid_leaves,
            "chunk mid leaves",
            per(64 * 4),
        );
        self.mid_materials = regrown(
            &device,
            e,
            &self.mid_materials,
            "chunk mid materials",
            per(64 * 4),
        );
        self.chunk_keys = regrown(&device, e, &self.chunk_keys, "chunk keys", per(16));
        self.statics = regrown(
            &device,
            e,
            &self.statics,
            "chunk statics",
            per(std::mem::size_of::<ChunkStatic>()),
        );
        self.draw_list = regrown(&device, e, &self.draw_list, "chunk draw list", per(4));
        self.drawn_slots = regrown(&device, e, &self.drawn_slots, "chunk drawn slots", per(4));
        self.cull_bins = regrown(&device, e, &self.cull_bins, "chunk cull bins", per(4));
        self.leaf_pages = regrown(
            &device,
            e,
            &self.leaf_pages,
            "chunk leaf pages",
            per(LEAF_PAGES as usize * 4),
        );
        self.slot_details = regrown(&device, e, &self.slot_details, "chunk slot details", per(4));
        self.queue.submit([encoder.finish()]);
        let limits = device.limits();
        let binding = u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        if self.drawn_set.grow(slots, table_entries(slots, binding)) {
            self.chunk_table = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chunk table"),
                size: (self.drawn_set.table().len() * 16) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(
                &self.chunk_table,
                0,
                bytemuck::cast_slice(self.drawn_set.table()),
            );
        }
        // The old free slots taken first.
        let mut free: Vec<u32> = (old as u32..slots as u32).rev().collect();
        free.append(&mut self.free_slots);
        self.free_slots = free;
        self.slot_keys.resize(slots, [-1, 0, 0, 0]);
        self.slots_version = None;
        self.slot_capacity = slots;
    }

    /// The leaf pool doubled (up to its most): leaves, their materials and
    /// normals and each page's detail page copied into buffers twice the
    /// size; the new pages go on the free stack under the old ones, and its
    /// count rises by them on the GPU (`chunk_pool.wgsl`: the GPU's count is
    /// ahead of any read back). Submitted at once, as `grow_slots`.
    fn grow_leaves(&mut self) {
        // The pools are replaced: nothing bound over the old ones stays.
        self.gpu_cache.clear();
        let old_pages = self.leaf_capacity / LEAF_PAGE;
        let leaves = (self.leaf_capacity * 2).min(self.leaf_max);
        let pages = leaves / LEAF_PAGE;
        let added = pages - old_pages;
        let device = self.device.clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        let e = &mut encoder;
        self.leaves = regrown(&device, e, &self.leaves, "chunk leaves", leaves * 8);
        self.leaf_materials = regrown(
            &device,
            e,
            &self.leaf_materials,
            "chunk leaf materials",
            leaves * 2,
        );
        self.leaf_normals = regrown(
            &device,
            e,
            &self.leaf_normals,
            "chunk leaf normals",
            leaves * 2,
        );
        self.detail_pages = regrown(
            &device,
            e,
            &self.detail_pages,
            "chunk detail pages",
            pages * 4,
        );
        self.queue.write_buffer(
            &self.detail_pages,
            old_pages * 4,
            bytemuck::cast_slice(&vec![u32::MAX; added as usize]),
        );
        // The stack: the new pages, then the old stack (its count's worth
        // valid) over them.
        let free = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk free pages"),
            size: pages * 4,
            usage: self.free_pages.usage(),
            mapped_at_creation: false,
        });
        self.queue.write_buffer(
            &free,
            0,
            bytemuck::cast_slice(&(old_pages as u32..pages as u32).collect::<Vec<u32>>()),
        );
        encoder.copy_buffer_to_buffer(&self.free_pages, 0, &free, added * 4, old_pages * 4);
        let added_words = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk pool growth"),
            contents: bytemuck::bytes_of(&(added as i32)),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let group = self.gpu_cache.bind_group(
            &device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk pool"),
                layout: &self.pool_layout,
                entries: &[entry(0, &self.leaf_alloc), entry(1, &added_words)],
            },
        );
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pool_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        self.queue.submit([encoder.finish()]);
        self.free_pages = free;
        self.free_pages_seen += added;
        self.leaf_capacity = leaves;
        self.leaf_growths += 1;
    }

    /// The detail pool doubled (up to its most): its pages and the stack
    /// of those given back copied into buffers twice the size, the
    /// capacity raised and the overflow flag cleared before the frame's
    /// builds (`take_detail` in chunk_generate.wgsl).
    fn grow_details(&mut self, encoder: &mut helio_core::CommandRecorder<'_>) {
        // The pools are replaced: nothing bound over the old ones stays.
        self.gpu_cache.clear();
        let capacity = (self.detail_capacity * 2).min(self.detail_max);
        let usage = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC;
        let buffer = |label: &str, size: u64| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let cells = buffer("chunk detail cells", capacity * DETAIL_WORDS * 4);
        let free = buffer("chunk detail free pages", capacity * 4);
        encoder.copy_buffer_to_buffer(
            &self.detail_cells,
            0,
            &cells,
            0,
            self.detail_capacity * DETAIL_WORDS * 4,
        );
        encoder.copy_buffer_to_buffer(&self.detail_free, 0, &free, 0, self.detail_capacity * 4);
        self.queue
            .write_buffer(&self.detail_alloc, 4, bytemuck::bytes_of(&0i32));
        self.queue.write_buffer(
            &self.detail_alloc,
            12,
            bytemuck::bytes_of(&(capacity as i32)),
        );
        self.detail_cells = cells;
        self.detail_free = free;
        self.detail_capacity = capacity;
    }

    fn gen_group(
        &self,
        vol: &volumes::Volume,
        brush_refs: &wgpu::Buffer,
        evictions: &wgpu::Buffer,
        baked_refs: &wgpu::Buffer,
        baked_cells: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk generation"),
                layout: &self.gen_layout,
                entries: &[
                    entry(1, &vol.world),
                    entry(16, &vol.terrain),
                    entry(18, &vol.volumes),
                    entry(40, &vol.terrain_tables),
                    entry(41, vol.regions.buffer()),
                    entry(2, &self.tile_jobs),
                    entry(3, &vol.tile_data),
                    entry(25, &self.tile_node_data),
                    entry(26, &vol.tile_cave_data),
                    entry(34, &vol.tile_slope_data),
                    entry(35, &self.slope_lattice),
                    entry(36, &vol.feature_set),
                    entry(37, &vol.tile_placements),
                    entry(38, &self.job_features),
                    entry(39, &vol.feature_blocks),
                    entry(4, &self.tile_ranges),
                    entry(5, &self.chunk_jobs),
                    entry(6, &self.column_masks),
                    entry(7, &vol.brushes),
                    entry(8, brush_refs),
                    entry(9, &self.chunk_meta),
                    entry(10, &self.mids),
                    entry(11, &self.mid_leaves),
                    entry(12, &self.leaves),
                    entry(13, &self.leaf_alloc),
                    entry(14, &self.column_materials),
                    entry(15, &self.leaf_materials),
                    entry(27, &self.leaf_normals),
                    entry(17, &self.mid_materials),
                    entry(19, &self.job_results),
                    entry(20, &self.free_pages),
                    entry(21, evictions),
                    entry(24, &self.leaf_pages),
                    entry(22, baked_refs),
                    entry(23, baked_cells),
                    entry(28, &self.cell_materials),
                    entry(29, &self.detail_pages),
                    entry(30, &self.detail_cells),
                    entry(31, &self.detail_alloc),
                    entry(32, &self.detail_free),
                    entry(33, &self.slot_details),
                ],
            },
        )
    }

    /// A generated node's chunk is resident in `slot`.
    /// `kept` held for `key`, the least recently used dropped past the
    /// limit.
    fn keep(&mut self, key: NodeKey, kept: Kept) {
        self.kept_bytes += KEPT_OVERHEAD + kept.pack.len() * 4;
        if let Some(old) = self.kept.insert(key, kept) {
            self.kept_bytes -= KEPT_OVERHEAD + old.pack.len() * 4;
        }
        if self.kept_bytes > self.kept_limit {
            let mut by_use: Vec<(u64, NodeKey)> =
                self.kept.iter().map(|(k, v)| (v.used, *k)).collect();
            by_use.sort_unstable_by_key(|(used, _)| *used);
            for (_, key) in by_use {
                if self.kept_bytes <= self.kept_limit * 3 / 4 {
                    break;
                }
                self.drop_kept(key);
            }
        }
    }

    /// What was kept of `key` (and a pack of it on its way back) dropped:
    /// an edit is over it, or it was generated again.
    fn drop_kept(&mut self, key: NodeKey) {
        self.packed_since.remove(&key);
        if let Some(old) = self.kept.remove(&key) {
            self.kept_bytes -= KEPT_OVERHEAD + old.pack.len() * 4;
        }
    }

    /// `key` restored from what was kept of it into a new slot (a restore
    /// job of `restores`, its result after the batch's `generated` jobs):
    /// whether it was.
    /// `fp`'s measurement, while the footprint pool holds it.
    fn footprint_measured(vol: &volumes::Volume, fp: Footprint) -> Option<Quarters> {
        vol.pool
            .slots
            .get(&fp)
            .and_then(|&slot| vol.pool_measured[slot as usize])
    }

    fn restore(
        &mut self,
        vol: &mut volumes::Volume,
        key: NodeKey,
        restores: &mut Restores,
        generated: usize,
        evictions: &mut Vec<u32>,
        stats: &mut UpdateStats,
    ) -> bool {
        if generated + restores.jobs.len() >= CHUNK_BATCH || self.free_slots.is_empty() {
            return false;
        }
        let frame = self.frame_now;
        let Some(kept) = self.kept.get_mut(&key) else {
            return false;
        };
        kept.used = frame;
        // Its footprint as generation measured it: a footprint whose nodes
        // are all restored is never measured again, and the estimate from
        // its parent may classify it otherwise.
        let measured = kept.measured;
        if let Some(quarters) = measured {
            vol.events
                .push(residency::Event::Measured(key.footprint(), quarters));
        }
        if kept.pack.is_empty() {
            // Empty: the plan learns it again, nothing is generated.
            vol.events.push(residency::Event::Emptied(key));
            stats.restored_chunks += 1;
            return true;
        }
        let flags = kept.flags;
        let slot = self.free_slots.pop().unwrap();
        restores
            .jobs
            .push([slot, restores.data.len() as u32, 0, flags as u32]);
        restores.data.extend_from_slice(&kept.pack);
        self.serial += 1;
        restores.nodes.push((key, slot, self.serial));
        let record = chunk_static(
            &vol.grid,
            key.footprint(),
            key.cz,
            slot,
            (0.0, 64.0),
            32.0,
            vol.exact_level,
        );
        self.write_record(slot, &record, key.table_key());
        self.slot_keys[slot as usize] = key.table_key();
        let shown = self.drawn_set.holds(key);
        if let Some(old) = self.insert_resident(
            key,
            Resident {
                slot,
                serial: self.serial,
                z: None,
                shown,
                edited: true,
                flags,
                measured,
            },
        ) {
            evictions.push(old.slot);
        }
        stats.shown_chunks += usize::from(shown);
        stats.restored_chunks += 1;
        self.changed_nodes.insert(key);
        vol.events.push(residency::Event::Generated(key));
        true
    }

    /// The bind group of pack jobs `packs`, or of restore jobs `restores`
    /// and their packs `data` (`keep_layout`).
    fn keep_group(
        &self,
        packs: &[[u32; 4]],
        restores: &[[u32; 4]],
        data: &[u32],
    ) -> wgpu::BindGroup {
        let init = |label: &str, contents: &[u8]| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE,
                })
        };
        let packs = init("chunk pack jobs", bytemuck::cast_slice(packs));
        let restores_buffer = init("chunk restore jobs", bytemuck::cast_slice(restores));
        let data = init(
            "chunk restore packs",
            bytemuck::cast_slice(if data.is_empty() { &[0u32][..] } else { data }),
        );
        self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk keep"),
                layout: &self.keep_layout,
                entries: &[
                    entry(0, &packs),
                    entry(1, &self.pack_data),
                    entry(2, &self.pack_alloc),
                    entry(3, &restores_buffer),
                    entry(4, &data),
                ],
            },
        )
    }

    fn insert_resident(&mut self, key: NodeKey, resident: Resident) -> Option<Resident> {
        let old = self.nodes.insert(key, resident);
        if old.is_none() {
            *self.held.entry(key.volume).or_default() += 1;
            self.footprint_nodes
                .entry(key.footprint())
                .or_default()
                .push(key.cz);
        }
        old
    }

    fn remove_resident(&mut self, key: NodeKey) -> Option<Resident> {
        let old = self.nodes.remove(&key)?;
        if let Some(n) = self.held.get_mut(&key.volume) {
            *n -= 1;
        }
        if let Some(list) = self.footprint_nodes.get_mut(&key.footprint()) {
            list.retain(|&cz| cz != key.cz);
            if list.is_empty() {
                self.footprint_nodes.remove(&key.footprint());
            }
        }
        Some(old)
    }

    /// Jobs generating nodes `czs` of `fp` into `batch` (one footprint job,
    /// a chunk job each); their old chunks to `evictions`. How many fit.
    #[allow(clippy::too_many_arguments)]
    fn queue_footprint(
        &mut self,
        vol: &mut volumes::Volume,
        fp: Footprint,
        czs: &[i32],
        batch: &mut GenBatch,
        readback: &mut Readback,
        evictions: &mut Vec<u32>,
        stats: &mut UpdateStats,
    ) -> usize {
        if batch.footprints() >= TILE_BATCH || czs.is_empty() {
            return 0;
        }
        let room = CHUNK_BATCH
            .saturating_sub(batch.chunk_jobs.len() + batch.restored)
            .min(czs.len());
        if self.free_slots.len() < room {
            stats.out_of_slots = true;
        }
        let n = room.min(self.free_slots.len());
        if n == 0 {
            return 0;
        }
        let grid = vol.grid;
        let (i, j) = fp.span_half();
        let shift = fp.level + 1;
        let span = |c: i32| {
            (
                i64::from(c * CHUNK) << shift,
                (i64::from(c * CHUNK + CHUNK) << shift) - 1,
            )
        };
        let edits = vol.edits.clone();
        let baked = edits.bricks.holds(&fp);
        // Each chunk's brushes (from those over the footprint:
        // `BrushIndex`); at `DETAIL_LEVELS` each takes a `cell_materials`
        // slot of the batch's (its leaves get per-voxel materials): the
        // chunks up to the first the batch has no slot for.
        let near = edits.brushes_near(fp.face, i, j, fp.level);
        let mut details = batch.details as usize;
        let mut chunks = Vec::with_capacity(n);
        for &cz in &czs[..n] {
            // Large brushes apply below their bake level (baked cells hold
            // them from it up: `planet::bake_level`).
            let refs: Vec<u32> = near
                .iter()
                .copied()
                .filter(|&b| {
                    let fb = &edits.faces[b as usize];
                    brush_meets(fb, fp.face, fp.level, i, j, span(cz))
                        && ((b as usize) >= edits.large_faces
                            || fp.level < crate::planet::bake_level(fb))
                })
                .collect();
            let detailed = fp.level < DETAIL_LEVELS;
            if detailed && details >= DETAIL_JOBS {
                break;
            }
            details += usize::from(detailed);
            chunks.push((cz, refs, detailed));
        }
        if chunks.is_empty() {
            return 0;
        }
        // The terrain's regions over the footprint (its streams), resident
        // while its tiles and chunks generate (made off the frame: the
        // footprint waits for them).
        let keys = vol.region_keys(fp);
        if !vol.regions.reserve(&keys, self.frame_now) {
            return 0;
        }
        // A footprint to compute keeps its candidate blocks this frame.
        if vol.has_features
            && !vol.pool.slots.contains_key(&fp)
            && !vol.blocks.reserve(&vol.features, fp, self.frame_now)
        {
            return 0;
        }
        let Some((pool_slot, cached)) = vol.pool.take(fp, self.frame_now) else {
            return 0;
        };
        if !cached {
            vol.pool_measured[pool_slot as usize] = None;
        }
        let job = TileJob {
            face: u32::from(fp.face),
            level: fp.level,
            cx: fp.cx,
            cy: fp.cy,
            slot: pool_slot,
            _pad: [0; 3],
        };
        // A footprint already in this batch is in the pool by now (its
        // columns are computed before any chunk reads them).
        let index = match batch
            .computed
            .iter()
            .position(|t| t.slot == pool_slot)
            .map(|n| n as u32)
            .or_else(|| {
                batch
                    .cached
                    .iter()
                    .position(|t| t.slot == pool_slot)
                    .map(|n| n as u32 | CACHED)
            }) {
            Some(index) => index,
            None if cached => {
                batch.cached.push(job);
                stats.cached_footprints += 1;
                (batch.cached.len() as u32 - 1) | CACHED
            }
            None => {
                batch.computed.push(job);
                readback.footprints.push(fp);
                stats.generated_footprints += 1;
                batch.computed.len() as u32 - 1
            }
        };
        let queued = chunks.len();
        for (cz, refs, detailed) in chunks {
            let detail = if detailed {
                batch.details += 1;
                batch.details - 1
            } else {
                u32::MAX
            };
            let key = fp.node(cz);
            let slot = self.free_slots.pop().unwrap();
            self.serial += 1;
            let first = batch.refs.len() as u32;
            let large = refs
                .iter()
                .filter(|&&b| (b as usize) < edits.large_faces)
                .count() as u32;
            batch.refs.extend_from_slice(&refs);
            let baked = if baked {
                batch.baked_table(&mut vol.brick_cache, self.frame_now, &edits.baked, fp, cz)
            } else {
                u32::MAX
            };
            let under_edits = !refs.is_empty() || baked != u32::MAX;
            stats.generated_edited += usize::from(under_edits);
            batch.chunk_jobs.push(ChunkJob {
                tile: index,
                cz,
                slot,
                brushes: first,
                brush_count: batch.refs.len() as u32 - first,
                large,
                baked,
                detail,
            });
            // Layers not read back yet: the whole chunk.
            let record = chunk_static(&grid, fp, cz, slot, (0.0, 64.0), 32.0, vol.exact_level);
            self.write_record(slot, &record, key.table_key());
            self.slot_keys[slot as usize] = key.table_key();
            readback.chunks.push((key, slot, self.serial));
            let shown = self.drawn_set.holds(key);
            // Generated again: what was kept of it no longer holds.
            self.drop_kept(key);
            if let Some(old) = self.insert_resident(
                key,
                Resident {
                    slot,
                    serial: self.serial,
                    z: None,
                    shown,
                    edited: under_edits,
                    flags: 0,
                    measured: None,
                },
            ) {
                evictions.push(old.slot);
            }
            stats.shown_chunks += usize::from(shown);
            self.changed_nodes.insert(key);
            vol.events.push(residency::Event::Generated(key));
        }
        stats.generated_chunks += queued;
        queued
    }

    /// Follow `planet`'s edits in volume `id` (its recipe unchanged):
    /// resident nodes under brushes that came or went are generated again
    /// in the next update; under baked edits that changed (a seal's coarser
    /// levels), within the budget. Edits the change log cannot bridge
    /// (another history, or one past the changes it keeps) regenerate the
    /// volume's resident nodes in place ([`Self::resync_edits`]):
    /// [`EditSync::Resynced`] (a host may load the chunks it kept for that
    /// history, `load_kept`). None when there is no such volume.
    pub fn sync_edits(&mut self, id: VolumeId, planet: &Planet) -> Option<EditSync> {
        let mut vol = self.take_volume(id.0)?;
        let sync = self.sync_volume_edits(&mut vol, planet);
        self.put_volume(vol);
        Some(sync)
    }

    fn sync_volume_edits(&mut self, vol: &mut volumes::Volume, planet: &Planet) -> EditSync {
        let edits = planet.edits();
        let state = EditState::of(edits);
        if state == vol.edit_state {
            return EditSync::Followed;
        }
        let Some(changes) = edits.baked_changes_since(vol.edit_state.sealed) else {
            self.resync_edits(vol, planet, state);
            return EditSync::Resynced;
        };
        let columns: Vec<ColumnKey> = changes
            .restated
            .iter()
            .chain(&changes.world)
            .map(|c| c.key)
            .collect();
        // Where the bricks are: unchanged without seals, extended by a
        // seal's columns (bricks are only added), built again after an
        // unseal.
        let bricks = if state.sealed == vol.edit_state.sealed {
            vol.edits.bricks.clone()
        } else if state.sealed.sealed > vol.edit_state.sealed.sealed {
            std::sync::Arc::new(vol.edits.bricks.extended(
                planet.grid(),
                &edits.baked,
                columns.iter().copied(),
            ))
        } else {
            std::sync::Arc::new(classify::BrickIndex::new(
                planet.grid(),
                &edits.baked,
                vol.index,
            ))
        };
        let (view, extended) = edit_view(planet, vol.edits.version + 1, Some(&vol.edits), bricks);
        // Brushes in one list and not the other: past the lists' common
        // start (the old view's large faces, when the new one extends
        // them; a seal's own may match further).
        let start = if extended { vol.edits.large_faces } else { 0 };
        let same = start
            + vol
                .edits
                .faces
                .iter_from(start)
                .zip(view.faces.iter_from(start))
                .take_while(|(a, b)| a == b)
                .count();
        let mut count: FxHashMap<[u32; 12], i32> = FxHashMap::default();
        for fb in vol.edits.faces.iter_from(same) {
            *count.entry(bytemuck::cast(*fb)).or_default() -= 1;
        }
        for fb in view.faces.iter_from(same) {
            *count.entry(bytemuck::cast(*fb)).or_default() += 1;
        }
        // Recent brushes sealed into the baked store left the list, not the
        // world (its coarser levels follow from the baked changes).
        let sealed = state
            .sealed
            .sealed
            .saturating_sub(vol.edit_state.sealed.sealed);
        let sealed_faces: usize = vol.edits.recent_counts.iter().take(sealed).sum();
        let large = vol.edits.large_faces;
        for fb in vol.edits.faces.iter_from(large).take(sealed_faces) {
            if let Some(n) = count.get_mut(&bytemuck::cast::<FaceBrush, [u32; 12]>(*fb)) {
                if *n < 0 {
                    *n += 1;
                }
            }
        }
        // Classify against the new edits (drawn uniform nodes a brush's
        // surface crosses).
        let old = std::mem::replace(&mut vol.edits, std::sync::Arc::new(view));
        for (words, n) in count {
            if n != 0 {
                self.mark_brush(vol, &bytemuck::cast(words));
            }
        }
        drop(old);
        // Restated bricks (a seal's, an undone seal's: the world is the
        // same) end what was kept under them, and their nodes are generated
        // again when the budget allows.
        let restated: FxHashSet<Footprint> = changes
            .restated
            .iter()
            .map(|c| column_footprint(vol.index, c.key))
            .collect();
        if !restated.is_empty() {
            self.drop_kept_where(|k| restated.contains(&k.footprint()));
        }
        // Changes of the world (a batch sealed at once, features) are marked
        // as brushes are.
        self.mark_columns(vol, &changes.world);
        for column in changes.restated.iter().map(|c| c.key) {
            let fp = column_footprint(vol.index, column);
            for cz in self.footprint_nodes.get(&fp).cloned().unwrap_or_default() {
                let key = fp.node(cz);
                if !vol.urgent.contains(&key) {
                    vol.stale.insert(key);
                }
            }
        }
        self.write_brushes(vol, same);
        vol.edits_unsent = Some(vol.edits.clone());
        vol.edit_state = state;
        vol.ceiling = planet.outer_radius() - planet.grid().radius() + CEILING_MARGIN;
        vol.floor = planet.inner_radius() - planet.grid().radius() - CEILING_MARGIN;
        vol.bounds = crate::volume::bounding_radius(planet);
        self.motion_version += 1;
        EditSync::Followed
    }

    /// Edits unrelated to the ones volume `vol` followed: what changed is
    /// unknown, so its every resident node is generated again in place
    /// (drawn ones keep showing their chunks until their new ones come, the
    /// others go), what was kept of it goes, and its planner classifies
    /// every node again, drawing those edits made Mixed as they were until
    /// generated. The pools, residency and measured footprints stay.
    fn resync_edits(&mut self, vol: &mut volumes::Volume, planet: &Planet, state: EditState) {
        let grid = vol.grid;
        let volume = vol.index;
        let bricks = std::sync::Arc::new(classify::BrickIndex::new(
            &grid,
            &planet.edits().baked,
            volume,
        ));
        vol.edits = std::sync::Arc::new(edit_view(planet, vol.edits.version + 1, None, bricks).0);
        self.drop_kept_where(|k| k.volume == volume);
        let resident: Vec<NodeKey> = self
            .nodes
            .keys()
            .filter(|k| k.volume == volume)
            .copied()
            .collect();
        self.mark_resident(vol, resident);
        // Every node of every level, as an edit marks those under it.
        let everywhere: std::sync::Arc<[residency::NodeBox]> = (0..grid.levels())
            .map(|level| residency::NodeBox {
                level,
                x: [i32::MIN, i32::MAX],
                y: [i32::MIN, i32::MAX],
                z: [i32::MIN, i32::MAX],
            })
            .collect();
        for &face in grid.faces() {
            vol.events
                .push(residency::Event::Edited(face, everywhere.clone()));
        }
        // Drawn uniform nodes Mixed now: their chunks drawn when generated.
        let uniform: Vec<NodeKey> = self
            .drawn_set
            .uniform_nodes()
            .filter(|k| k.volume == volume)
            .collect();
        vol.urgent.extend(uniform);
        self.write_brushes(vol, 0);
        vol.edits_unsent = Some(vol.edits.clone());
        vol.edit_state = state;
        vol.ceiling = planet.outer_radius() - grid.radius() + CEILING_MARGIN;
        vol.floor = planet.inner_radius() - grid.radius() - CEILING_MARGIN;
        vol.bounds = crate::volume::bounding_radius(planet);
        self.motion_version += 1;
    }

    /// The brush buffer for `self.edits`: its faces from `from` on written
    /// in place, or a larger buffer when they outgrow it.
    fn write_brushes(&mut self, vol: &mut volumes::Volume, from: usize) {
        let faces = &vol.edits.faces;
        if faces.len() > vol.brush_capacity {
            (vol.brushes, vol.brush_capacity) = brush_buffer(&self.device, &self.queue, faces);
        } else {
            write_faces(&self.queue, &vol.brushes, faces, from);
        }
    }

    /// Nodes brush `fb` can change, at every level it applies at: resident
    /// ones in the cut are generated again (urgent), resident ones out of
    /// it are dropped (generated again if wanted), and drawn Solid or
    /// Buried ones its surface crosses are generated now, so a dig shows
    /// the frame it is made.
    fn mark_brush(&mut self, vol: &mut volumes::Volume, fb: &FaceBrush) {
        let (ci, cj, w) = (
            i64::from(fb.center[0]),
            i64::from(fb.center[1]),
            i64::from(fb.center[3]),
        );
        // Half base cells to footprints and node layers of a level.
        let to = |half: i64, level: u32| {
            (half.div_euclid(2) >> level).div_euclid(i64::from(CHUNK)) as i32
        };
        let span = |level: u32| {
            (
                to(ci - w, level)..=to(ci + w, level),
                to(cj - w, level)..=to(cj + w, level),
                to(i64::from(fb.k_lo), level)..=to(i64::from(fb.k_hi), level),
            )
        };
        let levels = (0..vol.grid.levels()).take_while(|&l| fb.active(l)).count() as u32;
        let volume = vol.index;
        let inside = |k: &NodeKey| {
            let (x, y, z) = span(k.level);
            k.volume == volume
                && k.face == fb.face()
                && k.level < levels
                && x.contains(&k.cx)
                && y.contains(&k.cy)
                && z.contains(&k.cz)
        };
        let footprints: u64 = (0..levels)
            .map(|l| {
                let (x, y, _) = span(l);
                (x.end() - x.start() + 1) as u64 * (y.end() - y.start() + 1) as u64
            })
            .sum();
        // Nodes generated empty under it are the planner's to generate again.
        let boxes: std::sync::Arc<[residency::NodeBox]> = (0..levels)
            .map(|l| {
                let (x, y, z) = span(l);
                residency::NodeBox {
                    level: l,
                    x: [*x.start(), *x.end()],
                    y: [*y.start(), *y.end()],
                    z: [*z.start(), *z.end()],
                }
            })
            .collect();
        vol.events.push(residency::Event::Edited(fb.face(), boxes));
        // What was kept of nodes under it no longer holds.
        self.drop_kept_where(inside);
        let mut marked: Vec<NodeKey> = Vec::new();
        if footprints > self.footprint_nodes.len() as u64 {
            marked.extend(self.nodes.keys().filter(|k| inside(k)));
        } else {
            for level in 0..levels {
                let (xs, ys, zs) = span(level);
                for cx in xs {
                    for cy in ys.clone() {
                        let fp = Footprint {
                            volume,
                            face: fb.face(),
                            level,
                            cx,
                            cy,
                        };
                        for &cz in self.footprint_nodes.get(&fp).into_iter().flatten() {
                            if zs.contains(&cz) {
                                marked.push(fp.node(cz));
                            }
                        }
                    }
                }
            }
        }
        self.mark_resident(vol, marked);
        // Drawn uniform nodes the brush's surface crosses become Mixed.
        let uniform: Vec<NodeKey> = self
            .drawn_set
            .uniform_nodes()
            .filter(|k| inside(k))
            .collect();
        for key in uniform {
            let bands = vol.edits.surface_bands(fb, key.footprint());
            let (lo, hi) = key.layers();
            if bands.iter().any(|&(a, b)| b > a && a < hi && b > lo) {
                vol.urgent.insert(key);
            }
        }
    }

    /// Nodes a change of the world in the baked store touched (`columns`,
    /// with their bricks' heights), marked as [`Self::mark_brush`] marks a
    /// brush's: what was kept of them goes, resident ones in the cut are
    /// generated again (urgent) and the rest dropped, and drawn uniform ones
    /// over the changed bricks become Mixed.
    fn mark_columns(
        &mut self,
        vol: &mut volumes::Volume,
        columns: &[crate::planet::ChangedColumn],
    ) {
        if columns.is_empty() {
            return;
        }
        // Each footprint's node layers holding changed bricks (a node's
        // layer is eight bricks).
        let mut spans: FxHashMap<Footprint, [i32; 2]> = FxHashMap::default();
        for c in columns {
            let [lo, hi] = [c.bricks[0] >> 3, c.bricks[1] >> 3];
            let span = spans
                .entry(column_footprint(vol.index, c.key))
                .or_insert([lo, hi]);
            *span = [span[0].min(lo), span[1].max(hi)];
        }
        let inside = |k: &NodeKey| {
            spans
                .get(&k.footprint())
                .is_some_and(|&[lo, hi]| (lo..=hi).contains(&k.cz))
        };
        // Nodes generated empty there are the planner's to generate again.
        let mut faces: FxHashMap<u8, Vec<residency::NodeBox>> = FxHashMap::default();
        for (fp, &[lo, hi]) in &spans {
            faces.entry(fp.face).or_default().push(residency::NodeBox {
                level: fp.level,
                x: [fp.cx, fp.cx],
                y: [fp.cy, fp.cy],
                z: [lo, hi],
            });
        }
        for (face, boxes) in faces {
            vol.events
                .push(residency::Event::Edited(face, boxes.into()));
        }
        self.drop_kept_where(inside);
        let marked: Vec<NodeKey> = spans
            .iter()
            .flat_map(|(fp, &[lo, hi])| {
                self.footprint_nodes
                    .get(fp)
                    .into_iter()
                    .flatten()
                    .filter(move |&&cz| (lo..=hi).contains(&cz))
                    .map(move |&cz| fp.node(cz))
            })
            .collect();
        self.mark_resident(vol, marked);
        let uniform: Vec<NodeKey> = self
            .drawn_set
            .uniform_nodes()
            .filter(|k| inside(k))
            .collect();
        vol.urgent.extend(uniform);
    }

    /// Resident nodes something changed under: generated again now when the
    /// cut draws them, else dropped (generated again if wanted).
    fn mark_resident(&mut self, vol: &mut volumes::Volume, nodes: Vec<NodeKey>) {
        for key in nodes {
            vol.stale.remove(&key);
            if self.drawn_set.in_cut(key) {
                vol.urgent.insert(key);
            } else if let Some(old) = self.remove_resident(key) {
                self.dropped.push(old.slot);
                vol.events.push(residency::Event::Stale(key));
            }
        }
    }

    /// What was kept of the nodes `inside` holds no longer.
    fn drop_kept_where(&mut self, inside: impl Fn(&NodeKey) -> bool) {
        let kept: Vec<NodeKey> = self
            .kept
            .keys()
            .chain(&self.packed_since)
            .filter(|k| inside(k))
            .copied()
            .collect();
        for key in kept {
            self.drop_kept(key);
        }
    }

    /// One frame of residency for an eye at world point `eye` moving at
    /// `velocity` (world) and looking along `view`, recorded into `encoder`
    /// (submit it before the next update): take back finished generation
    /// results and ray reports, plan every volume (each in its own frame,
    /// the eye's motion relative to it), evict, and generate a volume's
    /// most wanted nodes within the budget (volumes with work take turns,
    /// edits first). [`Self::draw`] then shows the plans' nodes.
    pub fn update(
        &mut self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        eye: DVec3,
        velocity: DVec3,
        view: Option<residency::View>,
        frame: u64,
        config: &ResidencyConfig,
    ) -> UpdateStats {
        self.claim_timing(frame);
        self.frame_now = frame;
        let mut stats = UpdateStats::default();
        // The primary volume follows the eye (`follow_eye`): in a new one,
        // foliage near the eye and every GI probe are placed again.
        let before = self.primary;
        self.follow_eye(eye);
        if before.is_some() && self.primary != before {
            let near = DVec3::splat(FOLIAGE_REACH);
            self.ground_changes.push([eye - near, eye + near]);
            self.gi_clipmap = None;
        }
        // The other volumes near the eye that moved change foliage's ground
        // where they were and are (`ground_probe_cross`).
        for vol in self.volumes.iter().flatten() {
            if Some(vol.index) == self.primary {
                continue;
            }
            let last = self.ground_placed.insert(vol.index, vol.transform);
            let Some(last) = last.filter(|t| *t != vol.transform) else {
                continue;
            };
            if let Some(r) = vol.bounds {
                if (vol.transform.position - eye).length() - r < FOLIAGE_REACH {
                    for t in [last, vol.transform] {
                        self.ground_changes
                            .push([t.position - DVec3::splat(r), t.position + DVec3::splat(r)]);
                    }
                }
            }
        }
        // Volumes the planner does not know yet, with this update's settings.
        for index in std::mem::take(&mut self.unplanned) {
            if let Some(vol) = self.volumes[usize::from(index)].as_mut() {
                self.worker.add_volume(
                    index,
                    vol.grid,
                    vol.height_layers,
                    config.plan,
                    vol.edits.clone(),
                );
                vol.edits_unsent = None;
            }
        }
        let mut evictions: Vec<u32> = std::mem::take(&mut self.dropped);

        let mut clock = std::time::Instant::now();
        let mut lap = |stats: &mut UpdateStats, i: usize| {
            stats.cpu_ms[i] = clock.elapsed().as_secs_f64() * 1e3;
            clock = std::time::Instant::now();
        };
        // 1. Finished generation results: measured footprints, occupancy.
        let results_scope = helio_core::CpuScope::new("VoxelChunks: results");
        while self
            .readbacks
            .front()
            .is_some_and(|r| r.ready.load(std::sync::atomic::Ordering::Acquire))
        {
            let r = self.readbacks.pop_front().unwrap();
            {
                let data = r.buffer.slice(..).get_mapped_range().unwrap();
                let words: &[i32] = bytemuck::cast_slice(&data);
                for (n, fp) in r.footprints.iter().enumerate() {
                    // By quarter: a quarter past a face's edge measured nothing.
                    let quarters: [Option<Ranges>; 4] = std::array::from_fn(|q| {
                        let w = &words[n * RANGE_WORDS + q * 4..n * RANGE_WORDS + q * 4 + 4];
                        (w[0] <= w[1]).then_some(Ranges {
                            volume: (w[0], w[1]),
                            surface: (w[2], w[1]),
                        })
                    });
                    let Some(vol) = self.volumes[usize::from(fp.volume)].as_mut() else {
                        continue;
                    };
                    if quarters.iter().any(Option::is_some) {
                        vol.events.push(residency::Event::Measured(*fp, quarters));
                        if let Some(&slot) = vol.pool.slots.get(fp) {
                            vol.pool_measured[slot as usize] = Some(quarters);
                        }
                    }
                }
                let base = r.footprints.len() * RANGE_WORDS;
                for (n, &(key, _, serial)) in r.chunks.iter().enumerate() {
                    let w = &words[base + n * 4..base + n * 4 + 4];
                    if self.nodes.get(&key).is_none_or(|c| c.serial != serial) {
                        continue;
                    }
                    let Some(mut vol) = self.take_volume(key.volume) else {
                        continue;
                    };
                    self.changed_nodes.insert(key);
                    if w[3] & 8 != 0 {
                        // Not stored: wanted again once leaves free up.
                        stats.out_of_leaves = true;
                        evictions.push(self.remove_resident(key).unwrap().slot);
                        vol.events.push(residency::Event::Stale(key));
                    } else if w[3] & 2 != 0 {
                        // Nothing to draw. (A chunk solid throughout stays:
                        // its faces are the surface where it lies on a
                        // chunk boundary.)
                        let old = self.remove_resident(key).unwrap();
                        // An edited one is remembered empty (generating it
                        // again would read the edits over it again).
                        if old.edited && self.kept_limit > 0 {
                            let measured = old
                                .measured
                                .or(Self::footprint_measured(&vol, key.footprint()));
                            self.keep(
                                key,
                                Kept {
                                    pack: Vec::new().into(),
                                    flags: w[3],
                                    measured,
                                    used: frame,
                                },
                            );
                        }
                        evictions.push(old.slot);
                        vol.events.push(residency::Event::Emptied(key));
                        stats.emptied_chunks += 1;
                    } else {
                        // Out of detail pages: its leaves' pairs show until
                        // the pool grows and it is built again.
                        if w[3] & 16 != 0 {
                            stats.out_of_details = true;
                            self.detail_starved.insert(key);
                        }
                        let measured = Self::footprint_measured(&vol, key.footprint());
                        let c = self.nodes.get_mut(&key).unwrap();
                        c.z = Some((w[1], w[2]));
                        c.flags = w[3];
                        c.measured = measured.or(c.measured);
                        vol.events.push(residency::Event::Occupied(
                            key,
                            (key.cz * CHUNK + w[1], key.cz * CHUNK + w[2]),
                        ));
                        // Its frame about its columns' mean top (its ground),
                        // else its occupied layers.
                        let surface = (w[3] as u32) >> 16;
                        let about = if surface == 0xffff {
                            (w[1] + w[2]) as f32 * 0.5
                        } else {
                            surface as f32 / 4.0
                        };
                        let record = chunk_static(
                            &vol.grid,
                            key.footprint(),
                            key.cz,
                            c.slot,
                            (w[1] as f32, w[2] as f32),
                            about,
                            vol.exact_level,
                        );
                        let slot = c.slot;
                        self.write_record(slot, &record, self.slot_keys[slot as usize]);
                    }
                    self.put_volume(vol);
                }
                let at = base + r.chunks.len() * 4;
                if let Some(&free) = words
                    .get(at)
                    .filter(|_| r.leaf_growths == self.leaf_growths)
                {
                    self.free_pages_seen = free.max(0) as u64;
                }
                if let [given, overflow, taken, capacity] =
                    words.get(at + 2..at + 6).unwrap_or_default()
                {
                    self.detail_used_seen = (*taken - *given).max(0) as u64;
                    if *overflow != 0 {
                        self.detail_overflow_seen = Some(*capacity as u64);
                    }
                }
            }
            r.buffer.unmap();
        }
        while self
            .pack_readbacks
            .front()
            .is_some_and(|r| r.ready.load(std::sync::atomic::Ordering::Acquire))
        {
            let r = self.pack_readbacks.pop_front().unwrap();
            {
                let data = r.buffer.slice(..).get_mapped_range().unwrap();
                let words: &[u32] = bytemuck::cast_slice(&data);
                let data_at = 1 + 2 * MAX_PACKS;
                for (n, &(key, flags, measured, serial)) in r.nodes.iter().enumerate() {
                    let (offset, len) = (words[1 + 2 * n], words[2 + 2 * n] as usize);
                    // Not packed (out of room), or edited since, or wanted
                    // again (evicted) or generated again (resident) since.
                    let current = match serial {
                        None => !self.nodes.contains_key(&key),
                        Some(serial) => self.nodes.get(&key).is_some_and(|c| c.serial == serial),
                    };
                    if !self.packed_since.remove(&key) || !current {
                        continue;
                    }
                    if offset == u32::MAX || u64::from(offset) + len as u64 > PACK_WORDS {
                        // Out of room: a resident one is packed again later.
                        if serial.is_some() {
                            self.to_pack.push(key);
                        }
                        continue;
                    }
                    let at = data_at + offset as usize;
                    self.keep(
                        key,
                        Kept {
                            pack: words[at..at + len].into(),
                            flags,
                            measured,
                            used: frame,
                        },
                    );
                }
            }
            r.buffer.unmap();
        }
        drop(results_scope);
        // Ray reports: nodes met, and Buried nodes met (generate them).
        let events_scope = helio_core::CpuScope::new("VoxelChunks: report events");
        stats.reports = self.seen.iter().map(|r| r.len()).sum();
        if self.counting {
            let distinct: FxHashSet<[i32; 4]> =
                self.seen.iter().flat_map(|r| r.iter().copied()).collect();
            stats.seen = distinct.len();
        }
        stats.reached = self.reached.len();
        for reports in std::mem::take(&mut self.seen) {
            for vol in self.volumes.iter_mut().flatten() {
                vol.events.push(residency::Event::Reports(reports.clone()));
            }
        }
        for key in std::mem::take(&mut self.reached) {
            if let Some(vol) = self
                .volumes
                .get_mut(usize::from(key.volume))
                .and_then(Option::as_mut)
            {
                vol.events.push(residency::Event::Reached(key));
            }
        }
        drop(events_scope);
        lap(&mut stats, 0);
        // 2. The latest plans (made on the worker from an eye a frame or two
        // old) and their evictions; then the next plans, with what slots
        // need freeing.
        let (mut added, mut removed) = (Vec::new(), Vec::new());
        let mut evicted = Vec::new();
        if let Some(plans) = self.worker.poll() {
            self.planned = Some((plans.events_seen, plans.reports_seen));
            self.plan_made = (plans.frame, plans.ms, plans.parts, plans.visits);
            stats.planned = true;
            for planned in plans.volumes {
                // A volume removed since its plan was made: nothing of it.
                let Some(vol) = self.volumes[usize::from(planned.volume)].as_mut() else {
                    continue;
                };
                stats.switches += planned.plan.switches;
                stats.switch_px = stats.switch_px.max(planned.plan.switch_px);
                vol.plan = planned.plan;
                vol.planned_for = Some(planned.made_for);
                evicted.extend(planned.evicted);
                added.extend(planned.added);
                removed.extend(planned.removed);
            }
        }
        // Removed volumes' nodes leave the cut.
        removed.append(&mut self.removed_drawn);
        stats.plan_age = frame.saturating_sub(self.plan_made.0);
        stats.plan_ms = self.plan_made.1;
        stats.plan_parts = self.plan_made.2;
        stats.plan_visits = self.plan_made.3;
        let mut packs: Vec<(NodeKey, u32, i32, Option<Quarters>, Option<u64>)> = Vec::new();
        for key in evicted {
            let Some(mut vol) = self.take_volume(key.volume) else {
                continue;
            };
            // An edited chunk to keep past this frame's packs stays resident
            // (the planner told so: it is evicted again, packed, by a later
            // plan) rather than lost to generating again from the history.
            // An edited chunk, read back and unedited since, is kept.
            let keep = self.kept_limit > 0
                && self
                    .nodes
                    .get(&key)
                    .is_some_and(|c| c.edited && c.z.is_some())
                && !self.kept.contains_key(&key)
                && !vol.urgent.contains(&key)
                && !vol.stale.contains(&key);
            if keep && packs.len() >= MAX_PACKS {
                vol.events.push(residency::Event::Generated(key));
                stats.evictions_deferred += 1;
                self.put_volume(vol);
                continue;
            }
            if let Some(old) = self.remove_resident(key) {
                if keep {
                    packs.push((key, old.slot, old.flags, old.measured, None));
                    self.packed_since.insert(key);
                }
                evictions.push(old.slot);
                stats.evicted_unseen += usize::from(!old.shown);
                stats.evicted_chunks += 1;
            }
            vol.urgent.remove(&key);
            vol.stale.remove(&key);
            self.changed_nodes.insert(key);
            self.put_volume(vol);
        }
        // Resident edited chunks packed on demand (a save), as room allows.
        while packs.len() < MAX_PACKS {
            let Some(key) = self.to_pack.pop() else { break };
            let Some(c) = self.nodes.get(&key) else {
                continue;
            };
            let Some(vol) = self.volumes[usize::from(key.volume)].as_ref() else {
                continue;
            };
            if c.edited
                && c.z.is_some()
                && !self.kept.contains_key(&key)
                && !self.packed_since.contains(&key)
                && !vol.urgent.contains(&key)
                && !vol.stale.contains(&key)
            {
                packs.push((key, c.slot, c.flags, c.measured, Some(c.serial)));
                self.packed_since.insert(key);
            }
        }
        // Slots and leaf pages kept free for a frame's generation and
        // more: short of either, its pool grows (doubling, up to its most);
        // at its most, the least recently visited nodes go.
        // Pages: an eighth of the pool, and two batches' at their average
        // (no more than half the pool).
        let reserve = |slots: usize| (slots / 8).max(2 * CHUNK_BATCH);
        let page_reserve = |leaves: u64| {
            let pages = leaves / LEAF_PAGE;
            (pages / 8)
                .max(2 * CHUNK_BATCH as u64 * LEAVES_PER_SLOT / LEAF_PAGE)
                .min(pages / 2)
        };
        let grow_clock = std::time::Instant::now();
        if self.free_slots.len() + evictions.len() < reserve(self.slot_capacity)
            && self.slot_capacity < self.max_chunks
        {
            self.grow_slots();
        }
        if self.free_pages_seen < page_reserve(self.leaf_capacity)
            && self.leaf_capacity < self.leaf_max
        {
            self.grow_leaves();
        }
        for vol in self.volumes.iter_mut().flatten() {
            if vol.pool.pressed && vol.pool.len() < vol.pool_max {
                vol.grow_footprints(&self.device, &self.queue);
            }
        }
        stats.grow_ms = grow_clock.elapsed().as_secs_f64() * 1e3;
        let reserve = reserve(self.slot_capacity);
        let mut need = reserve.saturating_sub(self.free_slots.len() + evictions.len());
        let pages = self.leaf_capacity / LEAF_PAGE;
        let page_reserve = page_reserve(self.leaf_capacity);
        if self.free_pages_seen < page_reserve && !self.nodes.is_empty() {
            let per_node =
                ((pages - self.free_pages_seen) as f64 / self.nodes.len() as f64).max(1.0);
            need =
                need.max(((page_reserve - self.free_pages_seen) as f64 / per_node).ceil() as usize);
        }
        // Each volume frees its share, by the chunks it holds.
        let resident = self.nodes.len().max(1);
        // The eye in each volume's frame, its view and its motion relative
        // to it.
        let now = std::time::Instant::now();
        let mut unseen = self
            .volumes
            .iter()
            .flatten()
            .any(|v| v.events.iter().any(residency::Event::structural))
            || self
                .planned
                .is_none_or(|p| p.0 != self.worker.events_sent());
        let mut jobs = Vec::new();
        for vol in self.volumes.iter_mut().flatten() {
            let local_eye = vol.local_eye(eye);
            let local_view = view.map(|v| residency::View {
                forward: vol.transform.dir_to_local(v.forward),
                cos_half: v.cos_half,
            });
            // The eye's own velocity in the volume's frame, and the volume's
            // motion under it since the last update.
            let mut local_velocity = vol.transform.dir_to_local(velocity);
            if let Some((_, last, at)) = vol.last_local_eye {
                let dt = now.duration_since(at).as_secs_f64();
                if last != vol.transform && dt > 1e-4 {
                    local_velocity += (local_eye - last.to_local(eye)) / dt;
                }
            }
            vol.last_local_eye = Some((local_eye, vol.transform, now));
            unseen |= vol
                .planned_for
                .is_none_or(|p| p.eye != local_eye || p.view != local_view);
            if !self.worker.busy() {
                jobs.push(residency::VolumeJob {
                    volume: vol.index,
                    eye: local_eye,
                    velocity: local_velocity,
                    view: local_view,
                    events: std::mem::take(&mut vol.events),
                    need: (need * self.held.get(&vol.index).copied().unwrap_or(0))
                        .div_ceil(resident),
                    edits: vol.edits_unsent.take(),
                });
            }
        }
        if !jobs.is_empty() {
            self.worker.submit(frame, &mut self.reports_unsent, jobs);
        }
        lap(&mut stats, 1);
        for vol in self.volumes.iter().flatten() {
            let plan = &vol.plan;
            stats.requests += plan.waiting;
            stats.coarse_share = stats.coarse_share.max(plan.coarse_share);
            stats.worst_wanted = stats.worst_wanted.max(plan.worst_wanted);
            if plan.worst_px > stats.worst_px {
                stats.worst_px = plan.worst_px;
                stats.worst = plan.worst;
            }
        }
        lap(&mut stats, 2);

        // A build found the detail pool full: at its present size it
        // doubles (up to its most); once it has grown since, the chunks
        // built short of pages are built again; full at its most, they
        // keep their leaves' pairs.
        if let Some(full_at) = self.detail_overflow_seen.take() {
            if full_at == self.detail_capacity && self.detail_capacity < self.detail_max {
                self.grow_details(encoder);
            }
            if full_at < self.detail_capacity {
                let starved: Vec<NodeKey> = self.detail_starved.drain().collect();
                for key in starved {
                    if let Some(vol) = self.volumes[usize::from(key.volume)].as_mut() {
                        if self.nodes.contains_key(&key) && !vol.urgent.contains(&key) {
                            vol.stale.insert(key);
                        }
                    }
                }
            } else {
                self.detail_starved.clear();
            }
        }

        // 3. Generate a volume's nodes: those edits changed, then its plan's
        // most wanted. Volumes with work take turns, those edits changed
        // first; a frame without any still runs the evictions and packs
        // (with any volume's pipelines: they touch the shared pool only).
        let indices = self.volume_indices();
        let turn = self.gen_turn;
        let rotation: Vec<u16> = (0..indices.len())
            .map(|n| indices[(turn + n) % indices.len().max(1)])
            .collect();
        let generating = rotation
            .iter()
            .find(|&&i| {
                self.volumes[usize::from(i)]
                    .as_ref()
                    .is_some_and(|v| !v.urgent.is_empty())
            })
            .or_else(|| {
                rotation.iter().find(|&&i| {
                    self.volumes[usize::from(i)]
                        .as_ref()
                        .is_some_and(volumes::Volume::wants_generation)
                })
            })
            .or(rotation.first())
            .copied();
        let Some(generating) = generating else {
            // No volume: evictions wait for one.
            self.dropped = evictions;
            lap(&mut stats, 3);
            return self.finish_update(encoder, stats, added, removed, frame, unseen);
        };
        self.gen_turn = indices
            .iter()
            .position(|&i| i == generating)
            .map_or(0, |p| p + 1);
        let mut vol = self.take_volume(generating).unwrap();
        let grid = vol.grid;
        let local_eye = Eye::new(&grid, vol.local_eye(eye));
        let mut batch = GenBatch::default();
        let queue_scope = helio_core::CpuScope::new("VoxelChunks: generate: queue");
        let mut readback = Readback {
            buffer: self.readback_pool.get(&self.device, 16),
            ready: Default::default(),
            footprints: Vec::new(),
            chunks: Vec::new(),
            leaf_growths: self.leaf_growths,
        };
        // The frame's generation: `footprints_per_frame` footprints at most
        // and, with a time budget, the measured cost of their columns and
        // chunks within it (eight footprints at least).
        let cpt = self.cost.chunks_per_tile;
        let cost = config.generate_ms.map(|ms| (ms, self.cost.model()));
        let most = config.footprints_per_frame.min(TILE_BATCH);
        stats.budget = cost.map_or(most * 2, |(ms, (fixed, tile, chunk))| {
            (((ms - fixed).max(0.0) / (tile / cpt.max(1.0) + chunk)) as usize).max(8)
        });
        let full = |batch: &GenBatch| {
            let tiles = batch.computed.len();
            tiles >= most
                || batch.footprints() >= TILE_BATCH
                || batch.chunk_jobs.len() >= CHUNK_BATCH
                || cost.is_some_and(|(ms, (fixed, tile, chunk))| {
                    (tiles >= 8 || batch.chunk_jobs.len() >= 8)
                        && fixed + tiles as f64 * tile + batch.chunk_jobs.len() as f64 * chunk >= ms
                })
        };
        // Nodes edits changed: those drawn in view first, nearest first,
        // then the rest drawn, then the cut's ancestors. Each keeps showing
        // its old chunk until its new one is generated (never a coarse
        // ancestor in its place); drawn ones in view are generated this
        // frame past the budget, up to `EDITED_NOW`, so a small edit shows
        // the frame it is made. Then stale nodes.
        let local_view = view.map(|v| (vol.transform.dir_to_local(v.forward), v.cos_half));
        let in_view = |k: &NodeKey| {
            local_view.is_none_or(|(forward, cos_half)| {
                let offset = local_eye.toward(&grid, k.footprint(), k.layers());
                let length = offset.length();
                let size = grid.level_size(k.level) * f64::from(CHUNK) * 1.5;
                length < size || offset.dot(forward) > cos_half * length - size
            })
        };
        let mut edited: Vec<(NodeKey, u8, f64)> = Vec::with_capacity(vol.urgent.len());
        for k in vol.urgent.drain() {
            let distance = *vol
                .urgent_distance
                .entry(k)
                .or_insert_with(|| tile_distance(&grid, k.footprint(), k.layers(), &local_eye));
            let rank = if !self.drawn_set.holds(k) {
                2
            } else if in_view(&k) {
                0
            } else {
                1
            };
            edited.push((k, rank, distance));
        }
        edited.sort_unstable_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)));
        let edited: Vec<(NodeKey, Option<u8>)> = edited
            .into_iter()
            .map(|(k, rank, _)| (k, Some(rank)))
            .chain(vol.stale.iter().map(|k| (*k, None)))
            .collect();
        let mut now = 0;
        for (key, urgent) in edited {
            let forced = urgent == Some(0) && now < EDITED_NOW;
            let hard_full = batch.footprints() >= TILE_BATCH
                || batch.chunk_jobs.len() >= CHUNK_BATCH
                || stats.out_of_slots;
            if hard_full || (!forced && full(&batch)) {
                if urgent.is_some() {
                    vol.urgent.insert(key);
                    continue;
                }
                break;
            }
            vol.stale.remove(&key);
            if self.queue_footprint(
                &mut vol,
                key.footprint(),
                &[key.cz],
                &mut batch,
                &mut readback,
                &mut evictions,
                &mut stats,
            ) == 0
            {
                if urgent.is_some() {
                    vol.urgent.insert(key);
                } else {
                    vol.stale.insert(key);
                }
            } else {
                now += usize::from(forced);
                if urgent.is_some() && self.drawn_set.kind(key).is_some_and(|k| k != Kind::Mixed) {
                    // A drawn uniform node an edit made Mixed: its chunk is
                    // drawn now, before the planner says so.
                    added.push((key, Kind::Mixed));
                    removed.push(key);
                }
            }
        }
        let urgent = &vol.urgent;
        vol.urgent_distance.retain(|k, _| urgent.contains(k));
        // The plan's requests, a footprint's nodes together.
        let mut restores = Restores::default();
        let requests = std::mem::take(&mut vol.plan.requests);
        // The terrain regions of the footprints the next frames may reach,
        // asked for together (made off the frame); a footprint whose regions
        // are not resident yet waits while the next ones go on.
        let mut ahead = Vec::new();
        for k in &requests {
            let fp = k.footprint();
            if ahead.last() != Some(&fp) {
                ahead.push(fp);
                if ahead.len() >= 4 * most {
                    break;
                }
            }
        }
        vol.regions.admit(self.frame_now);
        for fp in ahead {
            let keys = vol.region_keys(fp);
            vol.regions.request(&keys);
        }
        let mut at = 0;
        while at < requests.len() && !full(&batch) {
            let fp = requests[at].footprint();
            let end = requests[at..]
                .iter()
                .position(|k| k.footprint() != fp)
                .map_or(requests.len(), |n| at + n);
            let keys = vol.region_keys(fp);
            if !vol.regions.resident(&keys) {
                at = end;
                continue;
            }
            // Generated since that plan.
            let mut czs: Vec<i32> = requests[at..end]
                .iter()
                .filter(|k| !self.nodes.contains_key(k))
                .map(|k| k.cz)
                .collect();
            // Kept nodes come back by copy.
            czs.retain(|&cz| {
                !self.restore(
                    &mut vol,
                    fp.node(cz),
                    &mut restores,
                    batch.chunk_jobs.len(),
                    &mut evictions,
                    &mut stats,
                )
            });
            batch.restored = restores.jobs.len();
            if !czs.is_empty()
                && self.queue_footprint(
                    &mut vol,
                    fp,
                    &czs,
                    &mut batch,
                    &mut readback,
                    &mut evictions,
                    &mut stats,
                ) < czs.len()
            {
                break;
            }
            at = end;
        }
        vol.plan.requests = requests;
        let GenBatch {
            computed,
            cached,
            mut chunk_jobs,
            refs,
            baked_refs,
            ..
        } = batch;
        // Restored chunks' results follow the generated ones'.
        let results_at = chunk_jobs.len() as u32;
        for (n, job) in restores.jobs.iter_mut().enumerate() {
            job[2] = results_at + n as u32;
        }
        readback.chunks.extend(restores.nodes.iter().copied());
        drop(queue_scope);
        let record_scope = helio_core::CpuScope::new("VoxelChunks: generate: record");
        // The batch's footprints: those it computes first (the kernels run
        // over them), then those it reads from the pool.
        let computed_count = computed.len();
        for job in &mut chunk_jobs {
            if job.tile & CACHED != 0 {
                job.tile = computed_count as u32 + (job.tile & !CACHED);
            }
        }
        let tile_jobs: Vec<TileJob> = computed.into_iter().chain(cached).collect();

        // GPU: blocks of evicted chunks back to their free lists, then the
        // tiles' columns, the chunks' columns and trees.
        let refs_buffer = self.gpu_cache.buffer_init(
            &self.device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("chunk brush refs"),
                contents: bytemuck::cast_slice(if refs.is_empty() { &[0u32][..] } else { &refs }),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );
        let evictions_buffer = self.gpu_cache.buffer_init(
            &self.device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("chunk evictions"),
                contents: bytemuck::cast_slice(if evictions.is_empty() {
                    &[u32::MAX][..]
                } else {
                    &evictions
                }),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );
        let baked_refs_buffer = self.gpu_cache.buffer_init(
            &self.device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("chunk baked refs"),
                contents: bytemuck::cast_slice(if baked_refs.is_empty() {
                    &[u32::MAX][..]
                } else {
                    &baked_refs
                }),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );
        vol.brick_cache.upload(&self.device, &self.queue, encoder);
        let pack_group = (!packs.is_empty()).then(|| {
            let jobs: Vec<[u32; 4]> = packs.iter().map(|&(_, slot, ..)| [slot, 0, 0, 0]).collect();
            encoder.clear_buffer(&self.pack_alloc, 0, Some((1 + 2 * MAX_PACKS as u64) * 4));
            self.keep_group(&jobs, &[[0; 4]], &[0])
        });
        let restore_group = (!restores.jobs.is_empty())
            .then(|| self.keep_group(&[[0; 4]], &restores.jobs, &restores.data));
        vol.regions.flush(&self.queue);
        let group = self.gen_group(
            &vol,
            &refs_buffer,
            &evictions_buffer,
            &baked_refs_buffer,
            &vol.brick_cache.buffer,
        );
        if !tile_jobs.is_empty() {
            self.uploads.write(
                encoder,
                &self.tile_jobs,
                0,
                bytemuck::cast_slice(&tile_jobs),
            );
            let empty: Vec<[i32; 4]> =
                vec![[i32::MAX, i32::MIN + 1, i32::MAX, i32::MIN + 1]; computed_count.max(1) * 4];
            self.uploads
                .write(encoder, &self.tile_ranges, 0, bytemuck::cast_slice(&empty));
            self.uploads.write(
                encoder,
                &self.tile_ranges,
                (TILE_BATCH * RANGE_WORDS * 4) as u64,
                bytemuck::bytes_of(&(computed_count as i32)),
            );
        }
        // The frame's candidate block jobs and its computed footprints'
        // block tables.
        let block_jobs = if vol.has_features && computed_count > 0 {
            let computed = tile_jobs[..computed_count].iter().map(|t| Footprint {
                volume: vol.index,
                face: t.face as u8,
                level: t.level,
                cx: t.cx,
                cy: t.cy,
            });
            let (jobs, tiles, slots) = vol.blocks.take_batch(&vol.features, computed);
            let words = |at: usize| at as u64 * 4;
            if !jobs.is_empty() {
                self.uploads.write(
                    encoder,
                    &vol.feature_blocks,
                    words(candidates::JOBS_AT),
                    bytemuck::cast_slice(&jobs),
                );
            }
            self.uploads.write(
                encoder,
                &vol.feature_blocks,
                words(candidates::TILES_AT),
                bytemuck::cast_slice(&tiles),
            );
            if !slots.is_empty() {
                self.uploads.write(
                    encoder,
                    &vol.feature_blocks,
                    words(candidates::SLOTS_AT),
                    bytemuck::cast_slice(&slots),
                );
            }
            jobs.len() as u32
        } else {
            0
        };
        if !chunk_jobs.is_empty() {
            self.uploads.write(
                encoder,
                &self.chunk_jobs,
                0,
                bytemuck::cast_slice(&chunk_jobs),
            );
            if vol.has_features {
                encoder.clear_buffer(
                    &self.job_features,
                    0,
                    Some(chunk_jobs.len() as u64 * FEATURE_JOB_WORDS * 4),
                );
            }
        }
        // A pass per kernel, each timed (`QUERIES`).
        let stamp = |begin: Option<u32>, end: u32| {
            self.queries
                .as_ref()
                .zip(self.timing_now)
                .map(|((set, _), ring)| {
                    let ring = ring as u32;
                    wgpu::ComputePassTimestampWrites {
                        query_set: set,
                        beginning_of_pass_write_index: begin.map(|b| QUERIES * ring + b),
                        end_of_pass_write_index: Some(QUERIES * ring + end),
                    }
                })
        };
        let gen = vol.gen.clone();
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk tile nodes"),
                timestamp_writes: stamp(Some(0), 8),
            });
            pass.set_bind_group(0, &group, &[]);
            if let Some(keep) = &pack_group {
                pass.set_pipeline(&gen.pack);
                pass.set_bind_group(1, keep, &[]);
                pass.dispatch_workgroups(packs.len() as u32, 1, 1);
            }
            if !evictions.is_empty() {
                pass.set_pipeline(&gen.evict);
                pass.dispatch_workgroups((evictions.len() as u32).div_ceil(64), 1, 1);
            }
            if computed_count > 0 {
                pass.set_pipeline(&gen.tile_nodes);
                pass.dispatch_workgroups(computed_count as u32, 1, 1);
                pass.set_pipeline(&gen.tile_slope_nodes);
                pass.dispatch_workgroups(computed_count as u32, 1, 1);
            }
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk tile columns"),
                timestamp_writes: stamp(None, 9),
            });
            pass.set_bind_group(0, &group, &[]);
            if computed_count > 0 {
                pass.set_pipeline(&gen.tile_columns);
                pass.dispatch_workgroups(64, computed_count as u32, 1);
                pass.set_pipeline(&gen.tile_slope_heights);
                pass.dispatch_workgroups(
                    (SLOPE_LATTICE as u32).div_ceil(64),
                    computed_count as u32,
                    1,
                );
                pass.set_pipeline(&gen.tile_slopes);
                pass.dispatch_workgroups(
                    (SLOPE_STRIDE as u32).div_ceil(64),
                    computed_count as u32,
                    1,
                );
            }
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk tile volumes"),
                timestamp_writes: stamp(None, 1),
            });
            pass.set_bind_group(0, &group, &[]);
            if computed_count > 0 {
                // The footprints' cave primitives, then each column's
                // volume extent (`docs/volumes.md`).
                pass.set_pipeline(&gen.tile_caves);
                pass.dispatch_workgroups(
                    (computed_count as u32).min(crate::engine::HALO_GROUPS),
                    1,
                    1,
                );
                pass.set_pipeline(&gen.tile_volumes);
                pass.dispatch_workgroups(64, computed_count as u32, 1);
            }
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk tile features"),
                timestamp_writes: stamp(None, 10),
            });
            pass.set_bind_group(0, &group, &[]);
            if block_jobs > 0 {
                pass.set_pipeline(&gen.feature_block_quads);
                pass.dispatch_workgroups(block_jobs, 1, 1);
                pass.set_pipeline(&gen.feature_block_nodes);
                pass.dispatch_workgroups(
                    (4 * candidates::BLOCK_READS as u32).div_ceil(64),
                    block_jobs,
                    1,
                );
                pass.set_pipeline(&gen.feature_block_candidates);
                pass.dispatch_workgroups(block_jobs, 1, 1);
            }
            if computed_count > 0 && vol.has_features {
                pass.set_pipeline(&gen.tile_features);
                pass.dispatch_workgroups(
                    (vol.features.stride as u32).div_ceil(64).max(1),
                    computed_count as u32,
                    1,
                );
            }
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk columns"),
                timestamp_writes: stamp(None, 5),
            });
            pass.set_bind_group(0, &group, &[]);
            if !chunk_jobs.is_empty() {
                pass.set_pipeline(&gen.chunk_columns);
                pass.dispatch_workgroups(64, chunk_jobs.len() as u32, 1);
            }
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk build"),
                timestamp_writes: stamp(None, 6),
            });
            pass.set_bind_group(0, &group, &[]);
            // Detail jobs' leaf cells first (`chunk_cells_main`).
            if chunk_jobs.iter().any(|j| j.detail != u32::MAX) {
                pass.set_pipeline(&gen.chunk_cells);
                pass.dispatch_workgroups(64, chunk_jobs.len() as u32, 1);
            }
            if !chunk_jobs.is_empty() {
                pass.set_pipeline(&gen.chunk_build);
                pass.dispatch_workgroups(chunk_jobs.len() as u32, 1, 1);
            }
            if let Some(keep) = &restore_group {
                pass.set_pipeline(&gen.restore);
                pass.set_bind_group(1, keep, &[]);
                pass.dispatch_workgroups(restores.jobs.len() as u32, 1, 1);
            }
        }
        self.put_volume(vol);
        // The packs back (mapped with the results).
        if !packs.is_empty() {
            let directory = (1 + 2 * MAX_PACKS as u64) * 4;
            let buffer = self
                .readback_pool
                .get(&self.device, directory + PACK_WORDS * 4);
            encoder.copy_buffer_to_buffer(&self.pack_alloc, 0, &buffer, 0, directory);
            encoder.copy_buffer_to_buffer(&self.pack_data, 0, &buffer, directory, PACK_WORDS * 4);
            self.pack_pending = Some(PackReadback {
                buffer,
                ready: Default::default(),
                nodes: packs
                    .iter()
                    .map(|&(key, _, flags, measured, serial)| (key, flags, measured, serial))
                    .collect(),
            });
        }
        // Results back: footprints' layers, each chunk's job result, the
        // free leaf pages.
        if !tile_jobs.is_empty() || !restores.jobs.is_empty() {
            let tiles = (computed_count * RANGE_WORDS * 4) as u64;
            let chunks = ((chunk_jobs.len() + restores.jobs.len()) * 16) as u64;
            readback.buffer = self.readback_pool.get(&self.device, tiles + chunks + 24);
            if tiles > 0 {
                encoder.copy_buffer_to_buffer(&self.tile_ranges, 0, &readback.buffer, 0, tiles);
            }
            if chunks > 0 {
                encoder.copy_buffer_to_buffer(
                    &self.job_results,
                    0,
                    &readback.buffer,
                    tiles,
                    chunks,
                );
            }
            encoder.copy_buffer_to_buffer(&self.leaf_alloc, 0, &readback.buffer, tiles + chunks, 8);
            encoder.copy_buffer_to_buffer(
                &self.detail_alloc,
                0,
                &readback.buffer,
                tiles + chunks + 8,
                16,
            );
            self.pending = Some(readback);
        }
        // Slots evicted now are reused from the next frame.
        self.free_slots.extend(evictions);
        drop(record_scope);
        lap(&mut stats, 3);
        self.finish_update(encoder, stats, added, removed, frame, unseen)
    }

    /// The end of [`Self::update`]: kept chunks' counts, this frame's drawn
    /// nodes and the table entries that changed, the ground changes, and
    /// what settling waits for.
    fn finish_update(
        &mut self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        mut stats: UpdateStats,
        added: Vec<(NodeKey, Kind)>,
        removed: Vec<NodeKey>,
        frame: u64,
        unseen: bool,
    ) -> UpdateStats {
        let mut clock = std::time::Instant::now();
        let mut lap = |stats: &mut UpdateStats, i: usize| {
            stats.cpu_ms[i] = clock.elapsed().as_secs_f64() * 1e3;
            clock = std::time::Instant::now();
        };
        stats.kept_chunks = self.kept.len();
        stats.kept_bytes = self.kept_bytes;
        // 4. This frame's drawn nodes and the table entries that changed. The ground
        // under drawn nodes whose chunk changed (generated, dug) changes for foliage now
        // and again once a switch's fade ends; under nodes drawn finer than before, once
        // the fade ends (while it lasts the probe may meet either level). A node drawn
        // coarser in place of finer ones, or no longer drawn, does not change it: the
        // finer ground placed the foliage there, and placing it again against the coarser
        // would only make it flicker as a still view's cut breathes. Foliage is tiled on
        // the primary volume (the others near the eye change its ground where they move:
        // `update`).
        if let Some(primary) = self
            .primary
            .and_then(|p| self.volumes[usize::from(p)].as_ref())
        {
            let (grid, transform, volume) = (primary.grid, primary.transform, primary.index);
            let levels = grid.levels();
            let mut over_removed = FxHashSet::default();
            for &r in removed.iter().filter(|k| k.volume == volume) {
                let mut k = r;
                while k.level + 1 < levels {
                    k = k.parent();
                    if !over_removed.insert(k) {
                        break;
                    }
                }
            }
            let finer: FxHashSet<NodeKey> = added
                .iter()
                .map(|a| a.0)
                .filter(|k| k.volume == volume && !over_removed.contains(k))
                .collect();
            let drawn_set = &self.drawn_set;
            let changed: Vec<NodeKey> = self
                .changed_nodes
                .iter()
                .copied()
                .filter(|k| k.volume == volume && drawn_set.holds(*k) && !finer.contains(k))
                .collect();
            let due: Vec<NodeKey> = self
                .ground_later
                .iter()
                .filter(|(at, k)| *at <= frame && k.volume == volume)
                .map(|(_, k)| *k)
                .collect();
            self.ground_later.retain(|(at, _)| *at > frame);
            for key in changed.iter().chain(&due) {
                self.ground_changes
                    .push(world_box(node_box(&grid, *key), &transform));
            }
            self.ground_later.extend(
                changed
                    .iter()
                    .chain(&finer)
                    .map(|k| (frame + drawn::FADE_FRAMES + 1, *k)),
            );
            // A world nothing asks to draw keeps few: past a bound, the oldest go.
            if self.ground_changes.len() > GROUND_CHANGES {
                self.ground_changes
                    .drain(..self.ground_changes.len() - GROUND_CHANGES);
            }
        }
        // The occluders: chunks generated or dug in place where drawn, and
        // chunks drawn finer than before (the cut refining: what the sun's
        // pages were traced through changed), of every volume. A node drawn
        // coarser in place of finer ones changes nothing worth tracing
        // again (a still view's cut breathes; the finer pages stand).
        let occluders = std::time::Instant::now();
        if self.sun_occlusion.is_some() {
            let mut over_removed = FxHashSet::default();
            for &r in &removed {
                let Some(vol) = self.volumes[usize::from(r.volume)].as_ref() else {
                    continue;
                };
                let mut k = r;
                while k.level + 1 < vol.grid.levels() {
                    k = k.parent();
                    if !over_removed.insert(k) {
                        break;
                    }
                }
            }
            let added_keys: FxHashSet<NodeKey> = added.iter().map(|a| a.0).collect();
            let changed = self
                .changed_nodes
                .iter()
                .filter(|k| self.drawn_set.holds(**k) && !added_keys.contains(*k));
            // A refinement draws a node's children: their parent's box once.
            let finer: FxHashSet<NodeKey> = added_keys
                .iter()
                .filter(|k| !over_removed.contains(*k))
                .map(|k| {
                    let levels = self.volumes[usize::from(k.volume)]
                        .as_ref()
                        .map_or(0, |v| v.grid.levels());
                    if k.level + 1 < levels {
                        k.parent()
                    } else {
                        *k
                    }
                })
                .collect();
            for (key, refined) in changed
                .map(|k| (k, false))
                .chain(finer.iter().map(|k| (k, true)))
            {
                if let Some(vol) = self.volumes[usize::from(key.volume)].as_ref() {
                    let b = world_box(node_box(&vol.grid, *key), &vol.transform);
                    if refined {
                        self.occluder_refined.push(b);
                    } else {
                        self.occluder_changes.push(b);
                    }
                }
            }
            for list in [&mut self.occluder_changes, &mut self.occluder_refined] {
                if list.len() > GROUND_CHANGES {
                    list.drain(..list.len() - GROUND_CHANGES);
                }
            }
        }
        self.occluder_ms = occluders.elapsed().as_secs_f64() * 1e3;
        let nodes = &self.nodes;
        let chunk_of = |key: NodeKey| {
            nodes.get(&key).map(|c| drawn::NodeChunk {
                slot: c.slot,
                z: c.z
                    .map_or((0.0, 64.0), |(a, b)| (f64::from(a), f64::from(b))),
            })
        };
        // Uniform nodes show their volume's rock, or the material of the
        // brush that added them.
        for &(key, kind) in &added {
            if kind != Kind::Mixed {
                if let Some(vol) = self.volumes[usize::from(key.volume)].as_ref() {
                    let rock = vol.edits.added_material(key).map_or(vol.rock, |m| m as u8);
                    self.drawn_set.set_rock(key, rock);
                }
            }
        }
        self.drawn_set
            .update(&added, &removed, &self.changed_nodes, frame, chunk_of);
        for (key, _) in &added {
            if let Some(c) = self.nodes.get_mut(key) {
                stats.shown_chunks += usize::from(!c.shown);
                c.shown = true;
            }
        }
        self.changed_nodes.clear();
        lap(&mut stats, 4);
        // One upload of the changed entries, scattered on the GPU.
        let changes = self.drawn_set.take_changes();
        if !changes.is_empty() {
            let bytes = 16 + std::mem::size_of_val(&changes[..]) as u64;
            if bytes > self.table_changes.size() {
                self.table_changes = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("chunk table changes"),
                    size: bytes.next_power_of_two(),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            let head = [(changes.len() / 3) as i32, 0, 0, 0];
            self.uploads
                .write(encoder, &self.table_changes, 0, bytemuck::cast_slice(&head));
            self.uploads.write(
                encoder,
                &self.table_changes,
                16,
                bytemuck::cast_slice(&changes),
            );
            let group = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk table"),
                    layout: &self.table_layout,
                    entries: &[entry(0, &self.table_changes), entry(1, &self.chunk_table)],
                },
            );
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk table"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.table_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((changes.len() as u32 / 3).div_ceil(64), 1, 1);
        }
        lap(&mut stats, 5);
        stats.drawn_nodes = self.drawn_set.node_count();
        stats.fading_nodes = self.drawn_set.fading();
        self.last_generated = (stats.generated_footprints, stats.generated_chunks);
        // Settled: plans that saw every event want nothing more.
        let generated = stats.generated_chunks > 0;
        let mut waiting = 0;
        for vol in self.volumes.iter_mut().flatten() {
            let nodes = &self.nodes;
            vol.last_requests = vol
                .plan
                .requests
                .iter()
                .filter(|k| !nodes.contains_key(k))
                .count()
                + usize::from(
                    unseen || vol.events.iter().any(residency::Event::structural) || generated,
                );
            waiting += vol.last_requests;
        }
        // Rays must report the cut as it ends up: after the last change and
        // the last fade (what a finer node uncovers shows once its fade ends).
        if waiting > 0 || stats.fading_nodes > 0 {
            self.quiet_from = self.worker.reports_sent();
        }
        stats.drawn_chunks = self.drawn_set.list.len();
        stats.detail_pages = (self.detail_used_seen, self.detail_capacity);
        let pages = self.leaf_capacity / LEAF_PAGE;
        stats.chunk_slots = self.slot_capacity;
        stats.leaf_pages = (pages.saturating_sub(self.free_pages_seen), pages);
        stats.resident_chunks = self.nodes.len();
        self.flush_records(encoder);
        self.uploads.flush();
        stats
    }

    /// Chunk `slot`'s frame `record` and key `key`, written with the
    /// frame's others ([`Self::flush_records`]; the last write of a slot
    /// wins).
    fn write_record(&mut self, slot: u32, record: &ChunkStatic, key: [i32; 4]) {
        let mut r = [[0i32; 4]; RECORD_VEC4S];
        r[0][0] = slot as i32;
        r[1..RECORD_VEC4S - 1].copy_from_slice(bytemuck::cast_slice(bytemuck::bytes_of(record)));
        r[RECORD_VEC4S - 1] = key;
        match self.record_at.entry(slot) {
            std::collections::hash_map::Entry::Occupied(at) => self.records[*at.get()] = r,
            std::collections::hash_map::Entry::Vacant(at) => {
                at.insert(self.records.len());
                self.records.push(r);
            }
        }
    }

    /// The frame's chunk records in one upload, scattered into `statics`
    /// and `chunk_keys` by one dispatch, as Unreal's scatter uploads do (a
    /// write a chunk was a wgpu staging buffer a chunk: hundreds a frame
    /// while loading).
    fn flush_records(&mut self, encoder: &mut helio_core::CommandRecorder<'_>) {
        if self.records.is_empty() {
            return;
        }
        let count = self.records.len();
        let bytes = 16 + (count * RECORD_BYTES) as u64;
        if bytes > self.records_buffer.size() {
            self.records_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chunk records"),
                size: bytes.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let head = [count as i32, 0, 0, 0];
        self.uploads.write(
            encoder,
            &self.records_buffer,
            0,
            bytemuck::cast_slice(&head),
        );
        self.uploads.write(
            encoder,
            &self.records_buffer,
            16,
            bytemuck::cast_slice(&self.records),
        );
        let group = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk records"),
                layout: &self.records_layout,
                entries: &[
                    entry(0, &self.records_buffer),
                    entry(1, &self.statics),
                    entry(2, &self.chunk_keys),
                ],
            },
        );
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("chunk records"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.records_pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(((count * (RECORD_VEC4S - 1)) as u32).div_ceil(64), 1, 1);
        drop(pass);
        self.records.clear();
        self.record_at.clear();
    }

    /// Copy the last draw's hits under `picks` (request id, view point
    /// 0..1 from the top left) for readback, at most [`Self::pick_room`] a
    /// frame; [`Self::submitted`] answers them into `answers`. Returns how
    /// many it took (the rest wait a frame).
    pub fn copy_picks(
        &mut self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        picks: &[(u64, [f32; 2])],
        answers: &std::sync::Arc<crate::engine::VolumeAnswers>,
    ) -> usize {
        let Some(targets) = &self.targets else {
            return 0;
        };
        let Some(slot) = self
            .pick_slots
            .iter_mut()
            .find(|s| matches!(s.state, ReachState::Free))
        else {
            return 0;
        };
        let requests = &picks[..picks.len().min(MAX_PICKS)];
        if requests.is_empty() {
            return 0;
        }
        let size = targets.size;
        for (n, (_, uv)) in requests.iter().enumerate() {
            let x = ((uv[0].clamp(0.0, 1.0) * size[0] as f32) as u32).min(size[0].max(1) - 1);
            let y = ((uv[1].clamp(0.0, 1.0) * size[1] as f32) as u32).min(size[1].max(1) - 1);
            for (texture, at) in [(&targets.visibility, 0), (&targets.distance, 16)] {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x, y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &slot.buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 256 * n as u64 + at,
                            bytes_per_row: None,
                            rows_per_image: None,
                        },
                    },
                    wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        slot.requests = requests.iter().map(|(id, _)| *id).collect();
        slot.sink = Some(answers.clone());
        slot.state = ReachState::Copied;
        requests.len()
    }

    /// Whether picks are on their way back.
    pub fn picks_in_flight(&self) -> bool {
        self.pick_slots
            .iter()
            .any(|s| !matches!(s.state, ReachState::Free))
    }

    /// The hit of a pixel's visibility (slot, cell) and distance.
    fn pick_hit(&self, [slot, cell]: [u32; 2], t: f32) -> Option<crate::engine::PickHit> {
        let key = *self.slot_keys.get(slot as usize)?;
        if key[0] < 0 || !(t > 0.0) {
            return None;
        }
        let node = NodeKey::from_table_key(key);
        let volume = self.volumes.get(usize::from(node.volume))?.as_ref()?;
        let local = [cell & 63, (cell >> 6) & 63, (cell >> 12) & 63].map(|c| c as i32);
        let axis = (cell >> 18) & 3;
        // Visibility's side: 1 when the ray stepped down the axis.
        let down = (cell >> 20) & 1 == 1;
        Some(crate::engine::PickHit {
            volume: volume.tag,
            distance: f64::from(t),
            cell: crate::grid::Cell::new(
                node.face,
                key[1] * CHUNK + local[0],
                key[2] * CHUNK + local[1],
                key[3] * CHUNK + local[2],
            ),
            level: node.level,
            entered: Some(2 * axis + u32::from(!down)),
            material: (cell >> 21) & 31,
        })
    }

    pub fn submitted(&mut self) {
        // The sun's occlusion requests just copied: mapped.
        if let Some(o) = &mut self.sun_occlusion {
            for r in &mut o.request_slots {
                if matches!(r.slot.state, ReachState::Copied) {
                    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let flag = ready.clone();
                    r.slot
                        .buffer
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |result| {
                            if result.is_ok() {
                                flag.store(true, std::sync::atomic::Ordering::Release);
                            }
                        });
                    r.slot.state = ReachState::Mapping(ready);
                }
            }
        }
        self.gpu_cache.end_frame();
        self.uploads.end_frame();
        // Picks: answer those mapped, map those just copied.
        for n in 0..self.pick_slots.len() {
            match &self.pick_slots[n].state {
                ReachState::Copied => {
                    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let flag = ready.clone();
                    self.pick_slots[n]
                        .buffer
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |r| {
                            if r.is_ok() {
                                flag.store(true, std::sync::atomic::Ordering::Release);
                            }
                        });
                    self.pick_slots[n].state = ReachState::Mapping(ready);
                }
                ReachState::Mapping(ready) if ready.load(std::sync::atomic::Ordering::Acquire) => {
                    let results: Vec<crate::engine::PickResult> = {
                        let data = self.pick_slots[n]
                            .buffer
                            .slice(..)
                            .get_mapped_range()
                            .unwrap();
                        let words: &[u32] = bytemuck::cast_slice(&data);
                        self.pick_slots[n]
                            .requests
                            .iter()
                            .enumerate()
                            .map(|(i, &id)| crate::engine::PickResult {
                                id,
                                hit: self.pick_hit(
                                    [words[i * 64], words[i * 64 + 1]],
                                    f32::from_bits(words[i * 64 + 4]),
                                ),
                            })
                            .collect()
                    };
                    let slot = &mut self.pick_slots[n];
                    slot.buffer.unmap();
                    if let Some(sink) = slot.sink.take() {
                        for result in results {
                            sink.push(crate::engine::VolumeAnswer::Pick(result));
                        }
                    }
                    slot.requests.clear();
                    slot.state = ReachState::Free;
                }
                _ => {}
            }
        }
        // Ray reports: Buried nodes met, and the nodes the sampled hits
        // want (met, with their ancestors).
        helio_core::cpu_scope!("VoxelChunks: ray reports");
        let reached = &mut self.reached;
        take_reports(&mut self.buried_slots, |data| {
            let words: &[[i32; 4]] = bytemuck::cast_slice(data);
            let count = (words[0][0] as u32 as usize).min(words.len() - 1);
            reached.extend(words[1..=count].iter().map(|k| NodeKey::from_table_key(*k)));
        });
        let (seen, reports) = (&mut self.seen, &mut self.reports_unsent);
        take_reports(&mut self.feedback_slots, |data| {
            *reports += 1;
            let words: &[[i32; 4]] = bytemuck::cast_slice(data);
            let count = (words[0][0] as u32 as usize).min(words.len() - 1);
            seen.push(words[1..=count].into());
        });
        // Timestamps: map those just submitted, read those mapped.
        helio_core::cpu_scope!("VoxelChunks: timestamps and results");
        let period = f64::from(self.queue.get_timestamp_period()) / 1.0e6;
        for slot in &mut self.timing_slots {
            if let Some(ready) = &slot.ready {
                if !ready.load(std::sync::atomic::Ordering::Acquire) {
                    continue;
                }
                let (frame, tiles, chunks) = slot.frame.take().unwrap();
                {
                    let data = slot.buffer.slice(..).get_mapped_range().unwrap();
                    let t: &[u64] = bytemuck::cast_slice(&data);
                    let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * period;
                    let stages = [ms(t[0], t[10]), ms(t[10], t[5]), ms(t[5], t[6])];
                    let tile_stages = [
                        ms(t[0], t[8]),
                        ms(t[8], t[9]),
                        ms(t[9], t[1]),
                        ms(t[1], t[10]),
                    ];
                    let shadow_stages = [
                        ms(t[4], t[12]),
                        ms(t[12], t[13]),
                        ms(t[13], t[14]),
                        ms(t[14], t[15]),
                        ms(t[15], t[7]),
                    ];
                    let timing = FrameTimings {
                        frame,
                        draw: ms(t[2], t[4]),
                        seed: ms(t[2], t[3]),
                        shadow: ms(t[4], t[7]),
                        shadow_stages,
                        fallbacks: ms(t[18], t[14]),
                        occlusion: ms(t[16], t[17]),
                        denoise: ms(t[7], t[11]),
                        generate: ms(t[0], t[6]),
                        tiles,
                        chunks,
                        stages,
                        tile_stages,
                    };
                    // Footprints computed and chunks generated are measured
                    // apart: with the footprint pool a frame may generate
                    // chunks of footprints it does not compute.
                    let mix = |old: f64, new: f64| old * 0.9 + new * 0.1;
                    if tiles > 0 {
                        self.cost.tiles.add(tiles as f64, stages[0]);
                    }
                    if chunks > 0 {
                        self.cost.chunks.add(chunks as f64, stages[1] + stages[2]);
                    }
                    if tiles >= 8 {
                        self.cost.tile = mix(self.cost.tile, stages[0] / tiles as f64);
                        self.cost.chunks_per_tile =
                            mix(self.cost.chunks_per_tile, chunks as f64 / tiles as f64);
                    }
                    if chunks >= 8 {
                        self.cost.chunk =
                            mix(self.cost.chunk, (stages[1] + stages[2]) / chunks as f64);
                    }
                    self.timings.push(timing);
                }
                slot.buffer.unmap();
                slot.ready = None;
            } else if slot.frame.is_some() {
                let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let flag = ready.clone();
                slot.buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| {
                        if r.is_ok() {
                            flag.store(true, std::sync::atomic::Ordering::Release);
                        }
                    });
                slot.ready = Some(ready);
            }
        }
        if let Some(r) = self.pending.take() {
            r.buffer.map_read(r.ready.clone());
            self.readbacks.push_back(r);
        }
        if let Some(r) = self.pack_pending.take() {
            r.buffer.map_read(r.ready.clone());
            self.pack_readbacks.push_back(r);
        }
    }

    /// Whether the residency has settled ([`Self::residency_settled`]) and
    /// the sun's occlusion pages with it: none asked for and missing, none
    /// whose terrain changed, none traced through a coarser cut still to
    /// trace again (or the atlas is full, or [`PAGES_SETTLE_FRAMES`] went by
    /// since the residency settled: what it cannot finish never holds a
    /// host's wait). Shots and tests that wait for this see converged
    /// shadows.
    pub fn settled(&self) -> bool {
        self.residency_settled()
            && self.sun_occlusion.as_ref().is_none_or(|o| {
                let s = o.pages.stats;
                (s.missing == 0 && s.stale == 0 && s.coarse == 0)
                    || s.out_of_pages
                    || o.settled_since
                        .is_some_and(|f| o.drawn_frame + 1 >= f + PAGES_SETTLE_FRAMES)
            })
    }

    /// Whether nothing waits to be generated or read back, no switch
    /// between levels is fading (fades move with drawn frames), and the
    /// rays have swept the view since residency last changed.
    pub fn residency_settled(&self) -> bool {
        // Every pixel reports within `REPORT_SWEEP` frames: a plan that has
        // not seen a sweep of the cut as it is may still want more where
        // rays meet what the eye's distance does not ask for.
        let swept = self.targets.is_none()
            || self
                .planned
                .is_some_and(|p| p.1 >= self.quiet_from + REPORT_FRAMES);
        // Each volume's plan shown was made for its edits: what they changed
        // where the cut draws air (no node to mark) is planned.
        let volumes = self.volumes.iter().flatten().all(|v| {
            v.planned_for.is_some_and(|p| p.edits == v.edits.version)
                && v.last_requests == 0
                && v.urgent.is_empty()
                && v.stale.is_empty()
        });
        volumes
            && self.unplanned.is_empty()
            && self.pending.is_none()
            && self.readbacks.is_empty()
            && self.drawn_set.fading() == 0
            && swept
    }

    /// What keeps the world from settling ([`Self::settled`]'s terms), for
    /// diagnostics.
    pub fn settle_report(&self) -> String {
        let volumes: Vec<String> = self
            .volumes
            .iter()
            .flatten()
            .map(|v| {
                format!(
                    "volume {}: current {} requests {} urgent {} stale {}",
                    v.tag,
                    v.planned_for.is_some_and(|p| p.edits == v.edits.version),
                    v.last_requests,
                    v.urgent.len(),
                    v.stale.len(),
                )
            })
            .collect();
        format!(
            "{}; unplanned {} pending {} readbacks {} fading {} reports {:?}/{}",
            volumes.join("; "),
            self.unplanned.len(),
            self.pending.is_some(),
            self.readbacks.len(),
            self.drawn_set.fading(),
            self.planned.map(|p| p.1),
            self.quiet_from + REPORT_FRAMES,
        )
    }

    /// Node and occupied layers of the chunk in `slot`.
    pub fn chunk_at(&self, slot: u32) -> Option<(NodeKey, Option<(i32, i32)>)> {
        self.nodes
            .iter()
            .find(|(_, c)| c.slot == slot)
            .map(|(k, c)| (*k, c.z))
    }

    /// Resident chunks of volume `volume` over base column (`face`, `i`,
    /// `j`): node, occupied layers, drawn.
    pub fn chunks_over(
        &self,
        volume: VolumeId,
        face: u8,
        i: i32,
        j: i32,
    ) -> Vec<(NodeKey, Option<(i32, i32)>, bool)> {
        let mut out: Vec<_> = self
            .nodes
            .iter()
            .filter(|(k, _)| {
                k.volume == volume.0
                    && k.face == face
                    && (i >> k.level).div_euclid(CHUNK) == k.cx
                    && (j >> k.level).div_euclid(CHUNK) == k.cy
            })
            .map(|(k, c)| (*k, c.z, self.drawn_set.contains(c.slot)))
            .collect();
        out.sort_by_key(|c| (c.0.level, c.0.cz));
        out
    }

    /// `node`'s drawn state (diagnostics, `DrawnSet::describe`).
    pub fn describe_drawn(&self, node: NodeKey) -> (bool, Option<i32>, Option<(u64, bool, bool)>) {
        self.drawn_set.describe(node)
    }

    /// Whether draws write the plain colour target ([`Targets::colour`]).
    pub fn set_plain_colour(&mut self, on: bool) {
        self.plain_colour = on;
    }

    /// The renderer-wide debug mode for this voxel world.
    pub fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode & 0xff;
    }

    /// Repeats of the primary trace a draw (1: none; timing the trace).
    pub fn set_trace_rounds(&mut self, rounds: u32) {
        self.rounds = rounds.max(1);
    }

    /// The pixel whose boxes a counting build records (`pixel_trace`).
    pub fn set_debug_pixel(&mut self, pixel: Option<[u32; 2]>) {
        self.debug_pixel = pixel;
    }

    /// Chunks drawn by the last [`Self::update`].
    pub fn chunk_count(&self) -> usize {
        self.drawn_set.list.len()
    }

    /// Bytes of the resident trees: leaves with their two material bytes, and
    /// per resident chunk its mid nodes (masks, first leaf, material) and
    /// root (reads the leaf allocator back).
    pub fn resident_bytes(&self) -> u64 {
        let alloc: Vec<[i32; 2]> = self.read(&self.leaf_alloc, 1);
        let chunks = self.nodes.len();
        let leaves =
            (self.leaf_capacity / LEAF_PAGE).saturating_sub(alloc[0][0].max(0) as u64) * LEAF_PAGE;
        leaves * 10 + chunks as u64 * (64 * 24 + 48 + LEAF_PAGES * 4)
    }

    /// Every pixel's hit in the last [`Self::draw`], row by row (waits for
    /// the GPU: tests and diagnostics).
    pub fn read_hits(&self) -> Vec<Option<crate::engine::PickHit>> {
        let Some(targets) = &self.targets else {
            return Vec::new();
        };
        let [w, h] = targets.size;
        let rows = |bytes: u32| (w * bytes).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let read = |texture: &wgpu::Texture, bytes: u32| -> Vec<u8> {
            let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(rows(bytes) * h),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = self.device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(rows(bytes)),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit([encoder.finish()]);
            staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            let out = staging.slice(..).get_mapped_range().unwrap().to_vec();
            out
        };
        let (visibility, distance) = (read(&targets.visibility, 8), read(&targets.distance, 4));
        let word =
            |data: &[u8], at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let v = (y * rows(8) + x * 8) as usize;
                self.pick_hit(
                    [word(&visibility, v), word(&visibility, v + 4)],
                    f32::from_bits(word(&distance, (y * rows(4) + x * 4) as usize)),
                )
            })
            .collect()
    }

    /// The targets of the last [`Self::draw`].
    pub fn targets(&self) -> Option<&Targets> {
        self.targets.as_ref()
    }

    /// The chunk an eye at `eye` in volume `vol`'s frame walks from
    /// (`DrawParams::cut.z`, `NONE` over the ground): the finest drawn chunk
    /// of the volume over the eye's column whose occupied layers reach above
    /// the eye. In a cave the chunks around the eye may be air (freed) or
    /// hold the eye under their occupied layers (out of their raster boxes),
    /// and the rock beside them (Buried nodes) has no box: a ray seeded by
    /// the boxes would start far away. An eye off a plane's face (beside a
    /// placed block: `locate` clamps it onto the edge) is over no chunk.
    fn eye_chunk(&self, vol: &volumes::Volume, eye: DVec3) -> u32 {
        let (cell, c) = vol.grid.locate(eye);
        let n = f64::from(vol.grid.cells());
        if !(0.0..n).contains(&c[0]) || !(0.0..n).contains(&c[1]) {
            return u32::MAX;
        }
        self.nodes
            .iter()
            .filter(|(k, c)| {
                k.volume == vol.index
                    && k.face == cell.face
                    && (cell.i >> k.level).div_euclid(CHUNK) == k.cx
                    && (cell.j >> k.level).div_euclid(CHUNK) == k.cy
                    && self.drawn_set.contains(c.slot)
                    && c.z.is_some_and(|(_, hi)| {
                        (i64::from(k.cz * CHUNK + hi) << k.level) > i64::from(cell.k)
                    })
            })
            .min_by_key(|(k, _)| k.level)
            .map_or(u32::MAX, |(_, c)| c.slot)
    }

    /// The draw parameters of view `v` (in the world frame), in volume
    /// `vol`'s frame when given (placed by its transform now, and by `prev`
    /// at the view's last frame: its points' motion), else in the world's
    /// (the camera's: the denoiser's, which reads no grid). The volume's
    /// pass this view is `pass` of `traced`; `flags` as `DrawParams::volume`.
    fn draw_params(
        &self,
        v: &ViewInput,
        vol: Option<(&volumes::Volume, crate::volume::VolumeTransform)>,
        pass: u32,
        traced: u32,
        flags: u32,
    ) -> DrawParams {
        // The view in the volume's frame: its eye there, the projections of
        // its points (rotated to the world, then projected; the last
        // frame's from where they were then), the sun and an orthographic
        // view's axis turned into it.
        let (eye, view_proj, prev_view_proj, sun, ortho, rotation) = match vol {
            None => (
                v.eye,
                v.view_proj,
                v.prev_view_proj,
                v.sun,
                v.ortho,
                Mat4::IDENTITY,
            ),
            Some((vol, prev)) => {
                let t = vol.transform;
                let local_eye = t.to_local(v.eye);
                let turned = t.rotation != glam::DQuat::IDENTITY;
                let r = Mat4::from_mat3(t.matrix().as_mat3());
                let view_proj = if turned { v.view_proj * r } else { v.view_proj };
                let prev_view_proj = if prev == t && !turned {
                    v.prev_view_proj
                } else {
                    v.prev_view_proj * moved_from(&prev, local_eye, v.eye)
                };
                (
                    local_eye,
                    view_proj,
                    prev_view_proj,
                    t.dir_to_local(v.sun.as_dvec3()).as_vec3(),
                    v.ortho
                        .map(|(dir, offset)| (t.dir_to_local(dir.as_dvec3()).as_vec3(), offset)),
                    r,
                )
            }
        };
        let grid = vol.map(|(vol, _)| vol);
        let (eye_hi, eye_lo) = split(eye);
        let near = if v.ortho.is_some() { 0.0 } else { v.near };
        DrawParams {
            view_proj: view_proj.to_cols_array_2d(),
            inverse_view_proj: view_proj.inverse().to_cols_array_2d(),
            prev_view_proj: prev_view_proj.to_cols_array_2d(),
            eye_hi,
            eye_lo,
            sun: sun.normalize().extend(near).to_array(),
            debug: [
                self.debug_mode
                    | (u32::from(self.plain_colour) << 8)
                    | (u32::from(self.steady_shadows) << 12),
                u32::from(self.counting),
                self.debug_pixel.map_or(u32::MAX, |p| p[0]),
                self.debug_pixel.map_or(u32::MAX, |p| p[1]),
            ],
            table: [
                self.drawn_set.mask(),
                grid.map_or(0, |g| g.face_cells),
                v.drawn,
                v.frame as u32,
            ],
            eye_domain: {
                let d = (eye / crate::grid::DOMAIN_UNIT).floor();
                [d.x as i32, d.y as i32, d.z as i32, 0]
            },
            eye_domain_frac: {
                let d = eye / crate::grid::DOMAIN_UNIT;
                (d - d.floor()).as_vec3().extend(0.0).to_array()
            },
            up: match grid {
                Some(g) if !g.grid.is_plane() => eye
                    .normalize_or_zero()
                    .as_vec3()
                    .extend(eye.length() as f32)
                    .to_array(),
                _ => [0.0, 1.0, 0.0, 0.0],
            },
            ceiling: match grid {
                Some(g) if g.grid.is_plane() => {
                    // Its band's bottom, the world's lowest solid (the
                    // nodes under it are rock no ray from the camera
                    // reaches: a plane's underside is no world's).
                    let bottom = g.floor + CEILING_MARGIN;
                    [
                        (eye.y - g.ceiling) as f32,
                        0.0,
                        (eye.y - bottom) as f32,
                        0.0,
                    ]
                }
                Some(g) => {
                    let c = g.grid.radius() + g.ceiling;
                    [(eye.length_squared() - c * c) as f32, 0.0, 0.0, 0.0]
                }
                None => [0.0; 4],
            },
            planet: grid.map_or([0.0; 4], |g| {
                [
                    g.grid.radius() as f32,
                    g.grid.voxel_size() as f32,
                    g.exact_level.min(99) as f32,
                    (eye.length_squared() - g.grid.radius() * g.grid.radius()) as f32,
                ]
            }),
            cut: [
                grid.map_or(0, |g| g.grid.levels() - 1),
                self.drawn_set.fading() as u32,
                match grid {
                    Some(g) if v.ortho.is_none() => self.eye_chunk(g, eye),
                    _ => u32::MAX,
                },
                grid.and_then(|g| self.anchors.get(usize::from(g.index)))
                    .copied()
                    .unwrap_or(u32::MAX),
            ],
            light_eye: v.light_eye.map_or([0.0; 4], |e| e.extend(1.0).to_array()),
            // Two of the finest probes' spacings: the probes' visibility
            // holds the occlusion past them.
            sky: [
                self.gi_clipmap
                    .filter(|_| v.gi)
                    .map_or(0.0, |c| 2.0 * c.finest as f32),
                self.sun_angular_radius,
                0.0,
                0.0,
            ],
            ortho: ortho.map_or([0.0; 4], |(dir, offset)| dir.extend(offset).to_array()),
            history: [
                v.jitter[0].x - v.jitter[1].x,
                v.jitter[0].y - v.jitter[1].y,
                v.jitter[0].x,
                v.jitter[0].y,
            ],
            // The camera's (an orthographic view's rays all run one way).
            rays: if near <= 0.0 {
                [[0.0; 4]; 3]
            } else {
                let mut rays = ray_plane(view_proj, v.size);
                // Still: the projections less their jitter agree (to their
                // rounding).
                let unjittered = |m: Mat4, j: Vec2| Mat4::from_translation(-j.extend(0.0)) * m;
                let (now, last) = (
                    unjittered(view_proj, v.jitter[0]),
                    unjittered(prev_view_proj, v.jitter[1]),
                );
                let scale = now
                    .to_cols_array()
                    .iter()
                    .fold(0.0f32, |a, v| a.max(v.abs()));
                let still = now
                    .to_cols_array()
                    .iter()
                    .zip(last.to_cols_array())
                    .all(|(a, b)| (a - b).abs() <= 1e-4 * scale);
                rays[1][3] = f32::from(u8::from(still));
                rays
            },
            rotation: [
                rotation.x_axis.to_array(),
                rotation.y_axis.to_array(),
                rotation.z_axis.to_array(),
            ],
            volume: [
                vol.map_or(0, |(v, _)| u32::from(v.index)),
                pass,
                traced,
                flags,
            ],
            rect: match vol {
                Some((vol, _)) if pass > 0 && v.ortho.is_none() => screen_rect(v, vol),
                _ => [0, 0, v.size[0], v.size[1]],
            },
        }
    }

    /// The volumes a view traces, nearest first (by their bounds; those
    /// holding the eye first): those with drawn chunks whose bounds the view
    /// can see (`view_proj`, eye-relative in the world; an orthographic view
    /// sees along `ortho`), and the primary one always (the first pass writes
    /// the pixels no volume hits, and the primary's probes trace it).
    fn traced_volumes(&self, eye: DVec3, view_proj: Mat4, drawn_in: &FxHashSet<u16>) -> Vec<u16> {
        let rows = [
            view_proj.row(3) + view_proj.row(0),
            view_proj.row(3) - view_proj.row(0),
            view_proj.row(3) + view_proj.row(1),
            view_proj.row(3) - view_proj.row(1),
            view_proj.row(3),
        ];
        let mut out: Vec<(f64, u16)> = self
            .volumes
            .iter()
            .flatten()
            .filter(|vol| {
                if Some(vol.index) == self.primary {
                    return true;
                }
                if !drawn_in.contains(&vol.index) {
                    return false;
                }
                let Some(radius) = vol.bounds else {
                    return true;
                };
                let centre = (vol.transform.position - eye).as_vec3();
                rows.iter().all(|p| {
                    let n = p.truncate().length();
                    n <= 0.0 || (p.truncate().dot(centre) + p.w) / n >= -(radius as f32) * 1.01
                })
            })
            .map(|vol| {
                let gap = vol.bounds.map_or(0.0, |r| {
                    ((vol.transform.position - eye).length() - r).max(0.0)
                });
                (gap, vol.index)
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        out.into_iter().map(|(_, i)| i).collect()
    }

    /// The drawing pipelines of volume `index`'s form.
    fn pipelines_of(&self, index: u16) -> &volumes::DrawPipelines {
        let plane = self.volumes[usize::from(index)]
            .as_ref()
            .is_some_and(|v| v.grid.is_plane());
        self.pipelines[usize::from(plane)].as_ref().unwrap()
    }

    /// The passes of volume `index`'s form lighting other volumes' points
    /// (built when the second volume came: `cross_volumes` lists none
    /// before).
    fn cross_pipelines(&self, index: u16) -> &volumes::CrossPipelines {
        self.pipelines_of(index)
            .cross
            .as_ref()
            .expect("a world of more than one volume has its cross pipelines")
    }

    /// The volumes with chunks in the drawn cut.
    fn volumes_drawn(&self) -> FxHashSet<u16> {
        let mut out = FxHashSet::default();
        if self.volumes.iter().flatten().count() <= 1 {
            out.extend(self.volumes.iter().flatten().map(|v| v.index));
            return out;
        }
        for c in &self.drawn_set.list {
            out.insert((self.slot_keys[c.slot as usize][0] >> 16) as u16);
        }
        out
    }

    /// Each volume's anchor for the walks that enter it from elsewhere
    /// (`enter_volume`), for an eye at `eye` (world): the finest drawn chunk
    /// finer than its exact levels over the eye's column (a sphere's frames
    /// hold near their chunks), else a plane's coarsest drawn chunk (its
    /// frames are one affine map); none with a single volume.
    fn place_anchors(&mut self, eye: DVec3) {
        self.anchors.clear();
        self.anchors.resize(self.volumes.len(), u32::MAX);
        if self.volumes.iter().flatten().count() < 2 {
            return;
        }
        // Each volume's eye column (face, i, j), exact level and form; its
        // best chunk: over the column, its level, slot.
        type Column = ((u8, i32, i32), u32, bool);
        let columns: Vec<Option<Column>> = self
            .volumes
            .iter()
            .map(|v| {
                v.as_ref().map(|v| {
                    let (cell, _) = v.grid.locate(v.transform.to_local(eye));
                    (
                        (cell.face, cell.i, cell.j),
                        v.exact_level,
                        v.grid.is_plane(),
                    )
                })
            })
            .collect();
        let mut best: Vec<Option<(bool, u32, u32)>> = vec![None; self.volumes.len()];
        for c in &self.drawn_set.list {
            let key = self.slot_keys[c.slot as usize];
            let index = (key[0] >> 16) as usize;
            let Some(Some(((face, i, j), exact, plane))) = columns.get(index) else {
                continue;
            };
            let level = ((key[0] >> 8) & 0xff) as u32;
            if !plane && level >= *exact {
                continue;
            }
            let over = (key[0] & 0xff) as u8 == *face
                && (i >> level).div_euclid(CHUNK) == key[1]
                && (j >> level).div_euclid(CHUNK) == key[2];
            if !over && !plane {
                continue;
            }
            let better = best[index].is_none_or(|(o, l, _)| {
                (over && !o) || (over == o && if over { level < l } else { level > l })
            });
            if better {
                best[index] = Some((over, level, c.slot));
            }
        }
        for (anchor, best) in self.anchors.iter_mut().zip(best) {
            if let Some((_, _, slot)) = best {
                *anchor = slot;
            }
        }
    }

    /// The volumes that light the other volumes' points in view `v` (those
    /// with drawn chunks, `drawn_in`, when more than one is held): those
    /// whose sun shadow can fall on another traced volume (`order`,
    /// `shadow_reaches`), over the rectangle it can fall in (`shadow_rect`);
    /// those the GI probes' rays reach (`gi`: they trace this view) and
    /// foliage's probes may meet, but the primary (whose own passes walk
    /// them); with their parameters for it (the camera's, placed by `prev`
    /// a frame ago, over the shadow's rectangle) and bind group.
    fn cross_volumes(
        &self,
        v: &ViewInput,
        order: &[u16],
        drawn_in: &FxHashSet<u16>,
        gi: bool,
        prev: &[(u16, crate::volume::VolumeTransform)],
    ) -> Vec<CrossVolume> {
        let mut out = Vec::new();
        if self.volumes.iter().flatten().count() < 2 {
            return out;
        }
        let sun = v.sun.as_dvec3().normalize_or_zero();
        // The GI clipmap's coarsest level's side from the eye: its probes
        // and their rays.
        let gi_reach = self
            .gi_clipmap
            .filter(|_| gi)
            .map(|c| c.spacing(helio_gi_api::LEVELS - 1) * helio_gi_api::SIDE as f64);
        for vol in self
            .volumes
            .iter()
            .flatten()
            .filter(|w| drawn_in.contains(&w.index))
        {
            let within = |reach: f64| {
                vol.bounds
                    .is_none_or(|b| (vol.transform.position - v.eye).length() - b < reach)
            };
            let casts = sun != DVec3::ZERO
                && order.iter().any(|&r| {
                    r != vol.index
                        && self.volumes[usize::from(r)]
                            .as_ref()
                            .is_some_and(|rv| shadow_reaches(vol, rv, sun))
                });
            let others = Some(vol.index) != self.primary;
            let gi = others && gi_reach.is_some_and(within);
            let ground = others && within(FOLIAGE_REACH);
            if !(casts || gi || ground) {
                continue;
            }
            // Its pass in the view (past the traced where it is not one).
            let pass = order
                .iter()
                .position(|&i| i == vol.index)
                .unwrap_or(order.len());
            let traced = (order.len() + usize::from(pass == order.len())).max(2);
            let last = prev
                .iter()
                .find(|(i, _)| *i == vol.index)
                .map_or(vol.transform, |(_, t)| *t);
            let mut params = self.draw_params(v, Some((vol, last)), pass as u32, traced as u32, 0);
            if casts {
                params.rect = shadow_rect(v, vol);
            }
            self.queue
                .write_buffer(&vol.views.cross, 0, bytemuck::bytes_of(&params));
            out.push(CrossVolume {
                index: vol.index,
                group: self.draw_group(&vol.views.cross, vol, self.camera_view(vol)),
                shadow: casts.then_some(params.rect),
                gi,
                ground,
            });
        }
        out
    }

    /// Each traced volume's motion since its last draw for the denoiser
    /// (`motions`: the last frame's projection of this frame's eye-relative
    /// world points); whether any moved. Its transform as drawn now is kept
    /// for the next draw of eye `eye_index`.
    fn volume_motions(
        &mut self,
        v: &ViewInput,
        eye_index: usize,
    ) -> (bool, Vec<(u16, crate::volume::VolumeTransform)>) {
        let mut prev = Vec::new();
        let mut moved = false;
        for vol in self.volumes.iter_mut().flatten() {
            let last = vol.drawn_transform[eye_index].unwrap_or(vol.transform);
            moved |= last != vol.transform;
            prev.push((vol.index, last));
            vol.drawn_transform[eye_index] = Some(vol.transform);
        }
        if moved {
            // Prev clip of a world point p (eye-relative): the volume's
            // point there, where the last frame's transform had it.
            for &(index, last) in &prev {
                let vol = self.volumes[usize::from(index)].as_ref().unwrap();
                let motion = v.prev_view_proj * world_moved_from(&vol.transform, &last, v.eye);
                self.queue.write_buffer(
                    &self.motions,
                    u64::from(index) * 64,
                    bytemuck::cast_slice(&motion.to_cols_array()),
                );
            }
        }
        (moved, prev)
    }

    /// The sun's occlusion by the terrain (`helio_pass_shadow_matrix::
    /// sun_occlusion`, `sun_pages`): the pages the shadow passes asked for
    /// (read back), those the terrain changed under (chunks generated or
    /// dug, volumes moved), the epochs for the sun toward `sun`; then the
    /// pages planned, traced as orthographic views along their epoch's sun
    /// through every volume (`trace_view`, their pages' pixels only) and
    /// copied into their physical pages; the tables, near planes and frame
    /// for the eye at `eye` (world).
    pub fn draw_sun_occlusion(
        &mut self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: u64,
        eye: DVec3,
        sun: Vec3,
    ) {
        use helio_light_api::SUN_OCCLUSION_TABLE_WORDS;
        let toward = sun.as_dvec3().normalize();
        let Some(voxel) = self
            .primary
            .and_then(|p| self.volumes[usize::from(p)].as_ref())
            .map(|v| v.grid.voxel_size())
        else {
            return;
        };
        if !toward.is_finite() {
            return;
        }
        if self.timing_now.is_none() {
            self.claim_timing(frame);
        }
        let ring = self
            .timing_now
            .filter(|&r| r == (frame % TIMING_RING) as usize)
            .map(|r| r as u32);
        let stamp = |world: &Self, encoder: &mut helio_core::CommandRecorder<'_>, index: u32| {
            if let (Some((set, _)), Some(ring)) = (world.queries.as_ref(), ring) {
                drop(encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("chunk sun occlusion time"),
                    timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                        query_set: set,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(QUERIES * ring + index),
                    }),
                }));
            }
        };
        stamp(self, encoder, 16);
        let clock = std::time::Instant::now();
        let changes = std::mem::take(&mut self.occluder_changes);
        let refined = std::mem::take(&mut self.occluder_refined);
        let settled = self.residency_settled();
        let o = self
            .sun_occlusion
            .get_or_insert_with(|| SunOcclusion::new(&self.device, voxel));
        let mut job = sun_pages::PlanJob {
            frame,
            eye,
            sun: toward,
            settled,
            asked: Vec::new(),
            changes,
            refined,
        };
        // Volumes moved since: where they were and are now.
        for vol in self.volumes.iter().flatten() {
            let last = o.placed.insert(vol.index, vol.transform);
            if last.is_some_and(|t| t != vol.transform) {
                match vol.bounds {
                    Some(r) => {
                        for t in [last.unwrap(), vol.transform] {
                            job.changes
                                .push([t.position - DVec3::splat(r), t.position + DVec3::splat(r)]);
                        }
                    }
                    None => job.changes.push([DVec3::splat(-1e30), DVec3::splat(1e30)]),
                }
            }
        }
        // The pages asked for, back (copied for the planner's thread).
        for r in &mut o.request_slots {
            if let ReachState::Mapping(ready) = &r.slot.state {
                if ready.load(std::sync::atomic::Ordering::Acquire) {
                    {
                        let data = r.slot.buffer.slice(..).get_mapped_range().unwrap();
                        if let Some(asking) = r.asking {
                            let mut words = o.pages.spare.pop().unwrap_or_default();
                            words.clear();
                            words.extend_from_slice(bytemuck::cast_slice(&data));
                            job.asked.push((asking, words));
                        }
                    }
                    r.slot.buffer.unmap();
                    r.slot.state = ReachState::Free;
                }
            }
        }
        let mut laps = [
            self.occluder_ms,
            clock.elapsed().as_secs_f64() * 1e3,
            0.0,
            0.0,
            0.0,
        ];
        o.drawn_frame = frame;
        o.settled_since = settled.then(|| o.settled_since.unwrap_or(frame));
        // This frame's inputs go to the planner's thread for the next frame
        // (at once when it is free, else with the next frames'); the plan it
        // made from earlier frames' is traced if it is done (made while the
        // last frame recorded: the frame never waits for it).
        job.frame = frame + 1;
        o.pages.queue(job);
        let (windows, rows, parts) = o
            .pages
            .try_take()
            .map_or_else(Default::default, |p| (p.windows, p.rows, Some(p.parts)));
        laps[2] = clock.elapsed().as_secs_f64() * 1e3;
        // (No epoch before the first plan: nothing is read against one.)
        o.asking = o.pages.current().map(|current| sun_pages::Asking {
            epoch: current.id,
            frame,
            eye_pages: o.pages.eye_pages(eye),
        });
        // Room in the ring for every view's every volume.
        let passes = (windows.len() * self.volumes.iter().flatten().count()) as u64;
        if passes > o.ring_passes {
            o.ring_passes = passes.next_power_of_two();
            o.params_ring = params_ring(&self.device, o.ring_passes);
        }
        let drawn_in = self.volumes_drawn();
        let mut at = 0;
        for window in &windows {
            let near = self.trace_occlusion_view(encoder, frame, eye, window, &drawn_in, &mut at);
            self.sun_occlusion
                .as_mut()
                .unwrap()
                .pages
                .traced(window, near);
        }
        laps[3] = clock.elapsed().as_secs_f64() * 1e3;
        // The tables' changed rows (runs of them a write), the near
        // planes, the frame; this frame's requests cleared.
        let o = self.sun_occlusion.as_mut().unwrap();
        for (from, words) in &rows {
            self.uploads.write(
                encoder,
                &o.pages_buffer,
                *from as u64 * 4,
                bytemuck::cast_slice(words),
            );
        }
        let (gpu, near) = o.pages.frame(eye);
        self.uploads.write(
            encoder,
            &o.pages_buffer,
            u64::from(SUN_OCCLUSION_TABLE_WORDS) * 4,
            bytemuck::cast_slice(&near),
        );
        self.uploads
            .write(encoder, &o.frame, 0, bytemuck::bytes_of(&gpu));
        encoder.clear_buffer(&o.requests, 0, None);
        self.uploads.flush();
        // The render thread's CPU time, by part (the update's occluder
        // boxes; readback; handing over and taking plans; recording;
        // uploads), and
        // the planner's on its thread; with HELIO_SUN_PAGES_LOG, the frames
        // that traced pages.
        laps[4] = clock.elapsed().as_secs_f64() * 1e3;
        let stats = &mut o.pages.stats;
        stats.cpu_ms = laps[4] + laps[0];
        stats.cpu_parts = [
            laps[0],
            laps[1],
            laps[2] - laps[1],
            laps[3] - laps[2],
            laps[4] - laps[3],
        ];
        if let Some(parts) = parts {
            stats.planner_ms = parts.iter().sum();
        }
        if std::env::var_os("HELIO_SUN_PAGES_LOG").is_some() && parts.is_some() && stats.traced > 0
        {
            eprintln!("SUNPAGES frame {frame}: {stats:?}, planner parts {parts:?}");
        }
        stamp(self, encoder, 17);
    }

    /// The world distances along the current epoch's sun of the near plane
    /// of a trace view over `lo`..`hi` (m along its right and down from its
    /// anchor) and of the farthest a receiver lies: over the highest of the
    /// volumes' ceilings above the view, under the lowest of their floors
    /// within it. None where no volume lies under the view.
    fn occlusion_depths(
        &self,
        e: &sun_pages::EpochGrid,
        lo: [f64; 2],
        hi: [f64; 2],
    ) -> Option<(f64, f64)> {
        let sun = e.sun;
        let corners = [lo, [hi[0], lo[1]], [lo[0], hi[1]], hi].map(|c| e.point(c));
        let (mut near, mut far) = (f64::NEG_INFINITY, f64::INFINITY);
        for vol in self.volumes.iter().flatten() {
            let margin = 2.0 + 4.0 * vol.grid.voxel_size();
            let t = vol.transform;
            if vol.grid.is_plane() {
                // The ceiling's and the floor's planes along the sun, in
                // the volume's frame.
                let s = t.dir_to_local(sun);
                if s.y < 0.02 {
                    continue;
                }
                let at = |q: DVec3, y: f64| q.dot(sun) + (y - t.to_local(q).y) / s.y;
                near = corners
                    .iter()
                    .map(|&q| at(q, vol.ceiling) + margin)
                    .fold(near, f64::max);
                far = corners
                    .iter()
                    .map(|&q| at(q, vol.floor) - margin)
                    .fold(far, f64::min);
                continue;
            }
            // The volume's centre across the sun, and the view's nearest
            // and farthest points from it.
            let c = e.across(t.position);
            let nearest = [c[0].clamp(lo[0], hi[0]), c[1].clamp(lo[1], hi[1])];
            let rho_min = ((c[0] - nearest[0]).powi(2) + (c[1] - nearest[1]).powi(2)).sqrt();
            let rho_max = ((c[0] - lo[0]).abs().max((c[0] - hi[0]).abs()).powi(2)
                + (c[1] - lo[1]).abs().max((c[1] - hi[1]).abs()).powi(2))
            .sqrt();
            let outer = vol
                .bounds
                .map_or(f64::INFINITY, |b| b)
                .min(vol.grid.radius() + vol.ceiling);
            let inner = vol.grid.radius() + vol.floor;
            if rho_min >= outer {
                continue;
            }
            let base = t.position.dot(sun);
            let top = (outer * outer - rho_min * rho_min).sqrt();
            let bottom = if rho_max < inner {
                (inner * inner - rho_max * rho_max).sqrt()
            } else {
                -top
            };
            near = near.max(base + top + margin);
            far = far.min(base + bottom - margin);
        }
        (near > far).then_some((near, far))
    }

    /// Traces `window`'s pages as an orthographic view along the current
    /// epoch's sun, through each volume it sees a pass (their parameters
    /// from the ring's pass `at` on), and copies them into their physical
    /// pages; their near plane (world distance along the sun).
    fn trace_occlusion_view(
        &self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: u64,
        eye: DVec3,
        window: &sun_pages::Window,
        drawn_in: &FxHashSet<u16>,
        at: &mut u64,
    ) -> f64 {
        use helio_light_api::{SUN_OCCLUSION_ATLAS_PAGES, SUN_OCCLUSION_PAGE};
        let o = self.sun_occlusion.as_ref().unwrap();
        let e = o.pages.current().unwrap();
        let [_, down, forward] = e.axes;
        let texel = o.pages.texel(usize::from(window.level));
        let page_m = f64::from(SUN_OCCLUSION_PAGE) * texel;
        let side = f64::from(OCCLUSION_VIEW) * texel;
        let lo = window.origin.map(|p| f64::from(p) * page_m);
        let hi = [lo[0] + side, lo[1] + side];
        let page_texel = |key: &sun_pages::PageKey| {
            [
                (key.page[0] - window.origin[0]) as u32 * SUN_OCCLUSION_PAGE,
                (key.page[1] - window.origin[1]) as u32 * SUN_OCCLUSION_PAGE,
            ]
        };
        let atlas_at = |physical: u16| wgpu::TexelCopyTextureInfo {
            texture: &o.atlas,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: u32::from(physical) % SUN_OCCLUSION_ATLAS_PAGES * SUN_OCCLUSION_PAGE,
                y: u32::from(physical) / SUN_OCCLUSION_ATLAS_PAGES * SUN_OCCLUSION_PAGE,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        };
        let page_extent = wgpu::Extent3d {
            width: SUN_OCCLUSION_PAGE,
            height: SUN_OCCLUSION_PAGE,
            depth_or_array_layers: 1,
        };
        let Some((near, far)) = self.occlusion_depths(e, lo, hi) else {
            // Nothing under it: pages of no occluder.
            for &(_, physical) in &window.pages {
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &o.empty,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(SUN_OCCLUSION_PAGE * 4),
                            rows_per_image: None,
                        },
                    },
                    atlas_at(physical),
                    page_extent,
                );
            }
            return 0.0;
        };
        // The view's near plane over its centre, its rays along the sun.
        let centre = e.point([(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5]);
        let origin = centre + e.sun * (near - centre.dot(e.sun));
        let rel = (origin - eye).as_vec3();
        let look = glam::camera::rh::view::look_to_mat4(rel, forward.as_vec3(), (-down).as_vec3());
        let half = (side * 0.5) as f32;
        // Reversed depth: the near plane at 1.
        let proj = glam::camera::rh::proj::directx::orthographic(
            -half,
            half,
            -half,
            half,
            (near - far) as f32,
            0.0,
        );
        let view_proj = proj * look;
        let drawn = self.drawn_set.list.len() as u32;
        let input = ViewInput {
            frame,
            eye,
            view_proj,
            prev_view_proj: view_proj,
            jitter: [Vec2::ZERO; 2],
            size: [OCCLUSION_VIEW; 2],
            near: 0.0,
            sun: e.sun.as_vec3(),
            drawn,
            light_eye: None,
            gi: false,
            ortho: Some((forward.as_vec3(), rel.dot(forward.as_vec3()))),
        };
        let rects: Vec<[u32; 4]> = window
            .pages
            .iter()
            .map(|(key, _)| {
                let [x, y] = page_texel(key);
                [x, y, SUN_OCCLUSION_PAGE, SUN_OCCLUSION_PAGE]
            })
            .collect();
        // Only the pages' bounding rect is seeded and traced.
        let rect = rects
            .iter()
            .fold([u32::MAX, u32::MAX, 0, 0], |r, &[x, y, w, h]| {
                [r[0].min(x), r[1].min(y), r[2].max(x + w), r[3].max(y + h)]
            });
        let order = self.traced_volumes(eye, view_proj, drawn_in);
        for (pass, &index) in order.iter().enumerate() {
            let vol = self.volumes[usize::from(index)].as_ref().unwrap();
            let mut params = self.draw_params(
                &input,
                Some((vol, vol.transform)),
                pass as u32,
                order.len() as u32,
                0,
            );
            params.rect = rect;
            let offset = *at * OCCLUSION_PARAMS_STRIDE;
            *at += 1;
            self.uploads
                .write(encoder, &o.params_ring, offset, bytemuck::bytes_of(&params));
            self.trace_view(
                encoder,
                ViewBuffers {
                    params: &vol.views.sun,
                    cull_state: &o.cull_state,
                    cull_bins: &o.cull_bins,
                    draw_list: &o.draw_list,
                    seed_args: &o.seed_args,
                    buried: &o.buried,
                    counters: &o.counters,
                },
                vol,
                &params,
                Some((&o.params_ring, offset)),
                o.targets.trace(),
                drawn,
                self.rounds,
                &self.no_scene_depth,
                None,
                Some(&rects),
            );
        }
        for (key, physical) in &window.pages {
            let [x, y] = page_texel(key);
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &o.targets.distance,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                atlas_at(*physical),
                page_extent,
            );
        }
        near
    }

    /// The sun's occlusion as published (`draw_sun_occlusion`): the atlas,
    /// the page tables and near planes, the frame.
    pub fn sun_occlusion(&self) -> Option<(&wgpu::TextureView, &wgpu::Buffer, &wgpu::Buffer)> {
        self.sun_occlusion
            .as_ref()
            .map(|o| (&o.atlas_view, &o.pages_buffer, &o.frame))
    }

    /// What the sun's occlusion pages did in the last
    /// [`Self::draw_sun_occlusion`].
    pub fn sun_page_stats(&self) -> Option<SunPageStats> {
        self.sun_occlusion.as_ref().map(|o| o.pages.stats)
    }

    /// The sun's occlusion as published, read back (diagnostics and tests;
    /// waits on the device).
    pub fn read_sun_occlusion(&self) -> Option<helio_light_api::SunOcclusionSnapshot> {
        use helio_light_api::{SUN_OCCLUSION_ATLAS_SIDE, SUN_OCCLUSION_PAGES_BYTES};
        let o = self.sun_occlusion.as_ref()?;
        let side = SUN_OCCLUSION_ATLAS_SIDE;
        let bytes = u64::from(side * side * 4);
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &o.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(side * 4),
                    rows_per_image: Some(side),
                },
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let atlas = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
        let frame: Vec<helio_light_api::SunOcclusionFrame> = self.read(&o.frame, 1);
        let pages: Vec<u32> = self.read(&o.pages_buffer, (SUN_OCCLUSION_PAGES_BYTES / 4) as usize);
        Some(helio_light_api::SunOcclusionSnapshot {
            frame: frame[0],
            pages,
            atlas,
        })
    }

    /// The camera's view buffers (`trace_view`), its parameters volume
    /// `vol`'s.
    fn camera_view<'a>(&'a self, vol: &'a volumes::Volume) -> ViewBuffers<'a> {
        ViewBuffers {
            params: &vol.views.camera,
            cull_state: &self.cull_state,
            cull_bins: &self.cull_bins,
            draw_list: &self.draw_list,
            seed_args: &self.seed_args,
            buried: &self.buried,
            counters: &self.counters,
        }
    }

    /// The draw bind group of parameters `params` with volume `vol`'s
    /// materials (the chunk storage, table and motions shared).
    fn draw_group(
        &self,
        params: &wgpu::Buffer,
        vol: &volumes::Volume,
        vb: ViewBuffers<'_>,
    ) -> wgpu::BindGroup {
        self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk draw"),
                layout: &self.layouts.draw,
                entries: &[
                    entry(0, params),
                    entry(1, &self.statics),
                    entry(2, &self.chunk_meta),
                    entry(3, &self.mids),
                    entry(4, &self.mid_leaves),
                    entry(5, &self.leaves),
                    entry(6, &self.leaf_materials),
                    entry(7, &vol.materials),
                    entry(8, &self.mid_materials),
                    entry(9, vb.counters),
                    entry(10, &self.chunk_table),
                    entry(11, &self.chunk_keys),
                    entry(12, vb.draw_list),
                    entry(13, vb.buried),
                    entry(14, &self.leaf_pages),
                    entry(15, &self.leaf_normals),
                    entry(16, &self.detail_pages),
                    entry(17, &self.detail_cells),
                    entry(18, &self.slot_details),
                    entry(19, &self.motions),
                ],
            },
        )
    }

    /// Records volume `vol`'s pass of one view's primary visibility
    /// (`params`: its parameters, uploaded into `view`'s, or copied there in
    /// order from `params_from`'s buffer and offset holding them): its
    /// drawn chunks culled and binned, then `rounds` of seed and trace into
    /// `targets` (over `DrawParams::rect`; `drawn` chunks), timed as
    /// `timing` says; with `open`, only the pixels in those rects (x, y,
    /// width, height). Its pass (`DrawParams::volume`) keeps the hits of the
    /// volumes traced before it that are nearer than its own; the rays end
    /// at the meshes' `scene_depth`. Its draw bind group, for the stages
    /// that shade its hits.
    #[allow(clippy::too_many_arguments)]
    fn trace_view(
        &self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        vb: ViewBuffers<'_>,
        vol: &volumes::Volume,
        params: &DrawParams,
        params_from: Option<(&wgpu::Buffer, u64)>,
        targets: TraceTextures<'_>,
        drawn: u32,
        rounds: u32,
        scene_depth: &wgpu::TextureView,
        timing: Option<(&wgpu::QuerySet, u32)>,
        open: Option<&[[u32; 4]]>,
    ) -> wgpu::BindGroup {
        let pipelines = self.pipelines[usize::from(vol.grid.is_plane())]
            .as_ref()
            .expect("a volume's form has its pipelines");
        let (pass_index, passes) = (params.volume[1], params.volume[2]);
        match params_from {
            Some((from, at)) => encoder.copy_buffer_to_buffer(
                from,
                at,
                vb.params,
                0,
                std::mem::size_of::<DrawParams>() as u64,
            ),
            None => self
                .uploads
                .write(encoder, vb.params, 0, bytemuck::bytes_of(params)),
        }
        // The atmosphere's sun disk (its frame's `sun.w`, the cosine of its
        // angular radius) into `sky.z`.
        if let Some(frame) = &self.atmosphere_frame {
            encoder.copy_buffer_to_buffer(
                frame,
                ATMOSPHERE_SUN_DISK,
                vb.params,
                std::mem::offset_of!(DrawParams, sky) as u64 + 8,
                4,
            );
        }
        encoder.clear_buffer(vb.cull_state, 0, Some(CULL_BYTES));
        if pass_index == 0 {
            encoder.clear_buffer(vb.buried, 0, Some(16));
        }
        let group = self.draw_group(vb.params, vol, vb);
        let (depth, seed, exits, distance) = (
            self.gpu_cache.view(&targets.depth),
            self.gpu_cache.view(&targets.seed),
            self.gpu_cache.view(&targets.exits),
            self.gpu_cache.view(&targets.distance),
        );
        // Every ray starts unresolved (0); with `open`, those outside its
        // rects resolved (-1).
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chunk exits clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &exits,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(if open.is_some() {
                        wgpu::Color {
                            r: -1.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }
                    } else {
                        wgpu::Color::TRANSPARENT
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some(rects) = open {
            pass.set_pipeline(&pipelines.exits_open);
            for &[x, y, w, h] in rects {
                pass.set_scissor_rect(x, y, w, h);
                pass.draw(0..3, 0..1);
            }
        }
        drop(pass);
        let seed_group = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk seed exits"),
                layout: &self.layouts.seed,
                entries: &[
                    view_entry(5, &exits),
                    view_entry(4, &distance),
                    entry(22, vb.cull_state),
                    view_entry(27, scene_depth),
                ],
            },
        );
        {
            // 0. The draw list: the volume's drawn chunks in view, binned,
            // ordered.
            let frames = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk cull frames"),
                    layout: &self.layouts.cull[0],
                    entries: &[
                        entry(0, vb.params),
                        entry(1, &self.statics),
                        entry(2, &self.chunk_meta),
                        entry(11, &self.chunk_keys),
                    ],
                },
            );
            let lists = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk cull"),
                    layout: &self.layouts.cull[1],
                    entries: &[
                        entry(17, &self.drawn_slots),
                        entry(18, vb.cull_state),
                        entry(19, vb.cull_bins),
                        entry(20, vb.draw_list),
                        entry(21, vb.seed_args),
                    ],
                },
            );
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk cull"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &frames, &[]);
            pass.set_bind_group(1, &lists, &[]);
            pass.set_pipeline(&pipelines.cull_bin);
            pass.dispatch_workgroups(drawn.div_ceil(64), 1, 1);
            pass.set_pipeline(&pipelines.cull_offsets);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&pipelines.cull_scatter);
            pass.dispatch_workgroups(drawn.div_ceil(64), 1, 1);
        }
        let trace_group = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk trace"),
                layout: &self.layouts.trace,
                entries: &[
                    view_entry(0, &seed),
                    view_entry(1, &self.gpu_cache.view(&targets.visibility)),
                    view_entry(2, &self.gpu_cache.view(&targets.colour)),
                    view_entry(3, &distance),
                    view_entry(6, &exits),
                    view_entry(25, &self.uv_grid_view),
                    wgpu::BindGroupEntry {
                        binding: 26,
                        resource: wgpu::BindingResource::Sampler(&self.uv_grid_sampler),
                    },
                    view_entry(27, scene_depth),
                ],
            },
        );
        // Rounds of seed and trace: each resolves the rays that hit in
        // their box; the rest are seeded beyond it.
        // An orthographic view's rays start on its near plane (`ortho_view`).
        let ortho = params.ortho[..3].iter().any(|&v| v != 0.0);
        for round in 0..rounds {
            let first = round == 0 && pass_index == 0;
            let last = round + 1 == rounds && pass_index + 1 == passes;
            {
                // 1. Seed: each pixel's nearest box (beyond its last exit).
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("chunk seed"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &seed,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: f64::from(u32::MAX),
                                g: 0.0,
                                b: 0.0,
                                a: 0.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(helio_core::depth::CLEAR),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: timing.filter(|_| first).map(|(set, ring)| {
                        wgpu::RenderPassTimestampWrites {
                            query_set: set,
                            beginning_of_pass_write_index: Some(QUERIES * ring + 2),
                            end_of_pass_write_index: Some(QUERIES * ring + 3),
                        }
                    }),
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_bind_group(0, &group, &[]);
                pass.set_bind_group(1, &seed_group, &[]);
                // An orthographic view of pages seeds their rect alone.
                if open.is_some() {
                    let [x0, y0, x1, y1] = params.rect;
                    pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
                }
                // Boxes holding the eye only in the first round.
                if round == 0 {
                    pass.set_pipeline(&pipelines.seed_inside);
                    pass.draw_indirect(vb.seed_args, 0);
                }
                pass.set_pipeline(&pipelines.seed_outside);
                pass.draw_indirect(vb.seed_args, 16);
            }
            // 2. Trace each pixel's ray through its seed chunk.
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk trace"),
                timestamp_writes: timing.filter(|_| last).map(|(set, ring)| {
                    wgpu::ComputePassTimestampWrites {
                        query_set: set,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(QUERIES * ring + 4),
                    }
                }),
            });
            pass.set_pipeline(if ortho {
                &pipelines.trace_ortho
            } else if self.debug_mode == 32 {
                &pipelines.trace_heat
            } else {
                &pipelines.trace
            });
            pass.set_bind_group(0, &group, &[]);
            pass.set_bind_group(1, &trace_group, &[]);
            let (x, y) = rect_groups(params.rect, 8);
            pass.dispatch_workgroups(x, y, 1);
        }
        group
    }

    /// Draw the volumes seen from world point `eye` (`view_proj`: the
    /// camera's, eye-relative in the world's axes; the last frame's
    /// `prev_view_proj`; `jitter` this frame's projection jitter and the
    /// last frame's, in NDC) at `size` pixels into [`Self::targets`],
    /// timing it as draw number `frame` ([`Self::draw_ms`]).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: u64,
        eye: DVec3,
        view_proj: Mat4,
        prev_view_proj: Mat4,
        jitter: [Vec2; 2],
        near: f32,
        sun: Vec3,
        size: [u32; 2],
        scene_depth: Option<&wgpu::TextureView>,
        lights: Option<&SceneLights<'_>>,
        gi: Option<&GiRays<'_>>,
    ) {
        self.draw_for_eye(
            encoder,
            frame,
            eye,
            view_proj,
            prev_view_proj,
            jitter,
            near,
            sun,
            size,
            scene_depth,
            lights,
            gi,
            0,
        );
    }

    /// [`Self::draw`] for eye `eye_index` (0 the camera's, 1 a stereo
    /// pair's right), its writes uploaded.
    #[allow(clippy::too_many_arguments)]
    fn draw_for_eye(
        &mut self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: u64,
        eye: DVec3,
        view_proj: Mat4,
        prev_view_proj: Mat4,
        jitter: [Vec2; 2],
        near: f32,
        sun: Vec3,
        size: [u32; 2],
        scene_depth: Option<&wgpu::TextureView>,
        lights: Option<&SceneLights<'_>>,
        gi: Option<&GiRays<'_>>,
        eye_index: usize,
    ) {
        self.record_eye(
            encoder,
            frame,
            eye,
            view_proj,
            prev_view_proj,
            jitter,
            near,
            sun,
            size,
            scene_depth,
            lights,
            gi,
            eye_index,
        );
        self.uploads.flush();
    }

    #[allow(clippy::too_many_arguments)]
    fn record_eye(
        &mut self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: u64,
        eye: DVec3,
        view_proj: Mat4,
        prev_view_proj: Mat4,
        jitter: [Vec2; 2],
        near: f32,
        sun: Vec3,
        size: [u32; 2],
        scene_depth: Option<&wgpu::TextureView>,
        lights: Option<&SceneLights<'_>>,
        gi: Option<&GiRays<'_>>,
        eye_index: usize,
    ) {
        let Some(primary) = self.primary else {
            return;
        };
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(Targets::new(&self.device, size));
            // What was bound over the old targets goes with them.
            self.gpu_cache.clear();
        }
        // GI hits, meshes and fine cells see the sun by its occlusion pages
        // (`draw_sun_occlusion`; none drawn yet: lit, and meshes walk).
        if self.sun_occlusion.is_none() {
            let voxel = self.volumes[usize::from(primary)]
                .as_ref()
                .map_or(1.0, |v| v.grid.voxel_size());
            self.sun_occlusion = Some(SunOcclusion::new(&self.device, voxel));
        }
        let targets = self.targets.as_mut().unwrap();
        if lights.is_some() && targets.local.is_none() {
            targets.local = Some(LocalTargets::new(&self.device, size));
            self.gpu_cache.clear();
        }
        let eye_index = eye_index.min(1);
        if eye_index == 1 {
            targets.ensure_right_history(&self.device);
            if let Some(local) = &mut targets.local {
                local.ensure_right_history(&self.device, size);
            }
        }
        targets.denoised[eye_index] += 1;
        if self.timing_now.is_none() {
            self.claim_timing(frame);
        }
        // The drawn chunks' slots, when they changed; the GPU culls them
        // (`cull_bin`) into each volume's seed draw list.
        let drawn = self.drawn_set.list.len() as u32;
        if self.slots_version != Some(self.drawn_set.version()) {
            let slots: Vec<u32> = self.drawn_set.list.iter().map(|c| c.slot).collect();
            if !slots.is_empty() {
                self.uploads
                    .write(encoder, &self.drawn_slots, 0, bytemuck::cast_slice(&slots));
            }
            self.slots_version = Some(self.drawn_set.version());
        }
        let input = ViewInput {
            frame,
            eye,
            view_proj,
            prev_view_proj,
            jitter,
            size,
            near,
            sun,
            drawn,
            light_eye: lights.map(|l| l.eye),
            gi: gi.is_some(),
            ortho: None,
        };
        self.place_anchors(eye);
        let (moved, prev) = self.volume_motions(&input, eye_index);
        let timing = self
            .queries
            .as_ref()
            .zip(
                self.timing_now
                    .filter(|&r| r == (frame % TIMING_RING) as usize),
            )
            .map(|((set, resolve), ring)| (set, resolve, ring as u32));
        // The volumes in view, nearest first; their motion since the last
        // draw (the denoiser reprojects by it).
        let drawn_in = self.volumes_drawn();
        let order = self.traced_volumes(eye, view_proj, &drawn_in);
        let traced = order.len() as u32;
        let prev_of = |index: u16| {
            prev.iter()
                .find(|(i, _)| *i == index)
                .map(|(_, t)| *t)
                .unwrap()
        };
        // The camera's own parameters (the world frame), for the denoiser.
        let camera = self.draw_params(&input, None, 0, traced, u32::from(moved));
        self.uploads
            .write(encoder, &self.params, 0, bytemuck::bytes_of(&camera));
        if let Some(frame) = &self.atmosphere_frame {
            encoder.copy_buffer_to_buffer(
                frame,
                ATMOSPHERE_SUN_DISK,
                &self.params,
                std::mem::offset_of!(DrawParams, sky) as u64 + 8,
                4,
            );
        }
        // The meshes' depth (the frame's size): the rays end there, and
        // meshes in front of the terrain walk to the sun from it.
        let scene_depth = scene_depth.unwrap_or(&self.no_scene_depth);
        let primary_vol = self.volumes[usize::from(primary)].as_ref().unwrap();
        let camera_group =
            self.draw_group(&self.params, primary_vol, self.camera_view(primary_vol));
        let mut groups: Vec<(u16, wgpu::BindGroup, [u32; 4])> = Vec::with_capacity(order.len());
        for (pass, &index) in order.iter().enumerate() {
            let vol = self.volumes[usize::from(index)].as_ref().unwrap();
            let last = prev_of(index);
            let flags = u32::from(moved) | (u32::from(last != vol.transform) << 1);
            let params = self.draw_params(&input, Some((vol, last)), pass as u32, traced, flags);
            let group = self.trace_view(
                encoder,
                self.camera_view(vol),
                vol,
                &params,
                None,
                self.targets.as_ref().unwrap().trace(),
                drawn,
                self.rounds,
                scene_depth,
                timing.map(|(set, _, ring)| (set, ring)),
                None,
            );
            groups.push((index, group, params.rect));
        }
        let cross = self.cross_volumes(&input, &order, &drawn_in, gi.is_some(), &prev);
        let targets = self.targets.as_ref().unwrap();
        {
            // 4. Sun shadows: samples, then every hit, and meshes before
            // their hits; each volume's hits by its own pass.
            let occlusion = self.sun_occlusion.as_ref().unwrap();
            let (distance, visibility, samples, normal, sky) = (
                self.gpu_cache.view(&targets.distance),
                self.gpu_cache.view(&targets.visibility),
                self.gpu_cache.view(&targets.sun_samples),
                self.gpu_cache.view(&targets.normal),
                self.gpu_cache.view(&targets.sky_samples),
            );
            let sample_group = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk sun samples"),
                    layout: &self.layouts.shadow_sample,
                    entries: &[
                        view_entry(4, &distance),
                        view_entry(7, &visibility),
                        view_entry(9, &samples),
                    ],
                },
            );
            let shadow_group = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk shadow"),
                    layout: &self.layouts.shadow,
                    entries: &[
                        view_entry(4, &distance),
                        view_entry(7, &visibility),
                        entry(41, &targets.sun_walked),
                        view_entry(10, &samples),
                        entry(11, &targets.fallbacks),
                        entry(12, &targets.fallback_groups),
                        view_entry(13, &normal),
                        view_entry(16, &sky),
                        view_entry(27, scene_depth),
                        entry(62, &occlusion.frame),
                        view_entry(63, &occlusion.atlas_view),
                        view_entry(64, &targets.stochastic_view),
                        entry(65, &occlusion.pages_buffer),
                        entry(66, &occlusion.requests),
                    ],
                },
            );
            let sky_group = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk sky samples"),
                    layout: &self.layouts.sky_sample,
                    entries: &[
                        view_entry(4, &distance),
                        view_entry(7, &visibility),
                        view_entry(15, &sky),
                    ],
                },
            );
            let fallback_group = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk shadow fallbacks"),
                    layout: &self.layouts.shadow_fallback,
                    entries: &[
                        view_entry(4, &distance),
                        view_entry(7, &visibility),
                        entry(41, &targets.sun_walked),
                        entry(11, &targets.fallbacks),
                        view_entry(27, scene_depth),
                    ],
                },
            );
            // No fallbacks yet.
            self.uploads.write(
                encoder,
                &targets.fallbacks,
                0,
                bytemuck::cast_slice(&[0u32]),
            );
            self.uploads.write(
                encoder,
                &targets.fallback_groups,
                0,
                bytemuck::cast_slice(&[0u32, 1, 1]),
            );
            // Each part its own pass, its end timestamped (`shadow_stages`);
            // in it, each volume's hits by its own dispatches.
            let end = |index: u32| {
                timing.map(|(set, _, ring)| wgpu::ComputePassTimestampWrites {
                    query_set: set,
                    beginning_of_pass_write_index: None,
                    end_of_pass_write_index: Some(QUERIES * ring + index),
                })
            };
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("chunk sun samples"),
                    timestamp_writes: end(12),
                });
                pass.set_bind_group(1, &sample_group, &[]);
                for (index, group, rect) in &groups {
                    let p = self.pipelines_of(*index);
                    pass.set_bind_group(0, group, &[]);
                    for pipeline in [&p.shadow_sample, &p.shadow_sample_far] {
                        pass.set_pipeline(pipeline);
                        let (x, y) = rect_groups(*rect, 16);
                        pass.dispatch_workgroups(x, y, 1);
                    }
                }
            }
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("chunk sky samples"),
                    timestamp_writes: end(13),
                });
                pass.set_bind_group(1, &sky_group, &[]);
                for (index, group, rect) in &groups {
                    let p = self.pipelines_of(*index);
                    pass.set_bind_group(0, group, &[]);
                    for pipeline in [&p.sky_sample, &p.sky_sample_far] {
                        pass.set_pipeline(pipeline);
                        let (x, y) = rect_groups(*rect, 32);
                        pass.dispatch_workgroups(x, y, 1);
                    }
                }
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk shadow"),
                timestamp_writes: end(18),
            });
            pass.set_bind_group(1, &shadow_group, &[]);
            for (index, group, rect) in &groups {
                pass.set_pipeline(&self.pipelines_of(*index).shadow);
                pass.set_bind_group(0, group, &[]);
                let (x, y) = rect_groups(*rect, 8);
                pass.dispatch_workgroups(x, y, 1);
            }
            drop(pass);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk shadow fallbacks"),
                timestamp_writes: end(14),
            });
            pass.set_bind_group(1, &fallback_group, &[]);
            for (index, group, _) in &groups {
                pass.set_pipeline(&self.pipelines_of(*index).shadow_fallback);
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups_indirect(&targets.fallback_groups, 0);
            }
            // 4b. Each volume's shadow on the others' hits and the meshes,
            // over the rectangle it can fall in, once all have their own.
            for c in &cross {
                let Some(rect) = c.shadow else { continue };
                pass.set_pipeline(&self.cross_pipelines(c.index).shadow);
                pass.set_bind_group(0, &c.group, &[]);
                let (x, y) = rect_groups(rect, 8);
                pass.dispatch_workgroups(x, y, 1);
            }
            drop(pass);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk local shadows"),
                timestamp_writes: end(15),
            });
            // 4b. Local lights' terrain shadows, with the hits' normals.
            let local_group = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk local shadows"),
                    layout: &self.layouts.local,
                    entries: &[
                        view_entry(4, &distance),
                        view_entry(7, &visibility),
                        view_entry(27, scene_depth),
                        view_entry(29, &normal),
                        entry(30, &targets.local_seen),
                        entry(31, lights.map_or(&self.no_lights, |l| l.lights)),
                        entry(32, lights.map_or(&self.no_lights, |l| l.tile_lists)),
                        entry(33, lights.map_or(&self.no_lights, |l| l.tile_counts)),
                        entry(34, &targets.local_samples),
                    ],
                },
            );
            pass.set_bind_group(1, &local_group, &[]);
            for (index, group, rect) in &groups {
                let p = self.pipelines_of(*index);
                pass.set_bind_group(0, group, &[]);
                for pipeline in [&p.local_sample, &p.local_sample_far] {
                    pass.set_pipeline(pipeline);
                    let (x, y) = rect_groups(*rect, 16);
                    pass.dispatch_workgroups(x, y, 1);
                }
            }
            for (index, group, rect) in &groups {
                let p = self.pipelines_of(*index);
                pass.set_bind_group(0, group, &[]);
                for pipeline in [&p.local_shadow, &p.local_shadow_far] {
                    pass.set_pipeline(pipeline);
                    let (x, y) = rect_groups(*rect, 8);
                    pass.dispatch_workgroups(x, y, 1);
                }
            }
            drop(pass);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk GI rays"),
                timestamp_writes: end(7),
            });
            // 6. The GI probes' rays (a thread a ray, past the frame's
            // count none), through the primary volume, then each other
            // volume they reach (the nearer hit kept).
            if let (Some(gi), true, Some(occlusion), Some((_, group, _))) = (
                gi,
                self.gi_clipmap.is_some(),
                &self.sun_occlusion,
                groups.iter().find(|(i, ..)| *i == primary),
            ) {
                // Hits see the sun by its occlusion maps (none drawn: lit).
                let gi_group = self.gpu_cache.bind_group(
                    &self.device,
                    &wgpu::BindGroupDescriptor {
                        label: Some("chunk GI rays"),
                        layout: &self.layouts.gi,
                        entries: &[
                            entry(35, gi.frame),
                            entry(36, gi.rays),
                            entry(37, gi.hits),
                            entry(38, &self.gi_cells),
                            entry(39, gi.sun),
                            entry(62, &occlusion.frame),
                            view_entry(63, &occlusion.atlas_view),
                            entry(65, &occlusion.pages_buffer),
                        ],
                    },
                );
                pass.set_bind_group(0, group, &[]);
                pass.set_bind_group(1, &gi_group, &[]);
                pass.set_pipeline(&self.pipelines_of(primary).gi_trace);
                pass.dispatch_workgroups(helio_gi_api::MAX_RAYS.div_ceil(64), 1, 1);
                for c in cross.iter().filter(|c| c.gi) {
                    pass.set_bind_group(0, &c.group, &[]);
                    pass.set_pipeline(&self.cross_pipelines(c.index).gi_trace);
                    pass.dispatch_workgroups(helio_gi_api::MAX_RAYS.div_ceil(64), 1, 1);
                }
            }
            drop(pass);
            self.denoise(
                encoder,
                &camera_group,
                self.pipelines_of(primary),
                size,
                lights,
                eye_index,
                timing,
            );
        }
        // The sun's occlusion pages this frame's pixels asked for, back
        // without waiting (when a slot is free).
        if let Some(o) = &mut self.sun_occlusion {
            if let Some(asking) = o.asking.take() {
                if let Some(r) = o
                    .request_slots
                    .iter_mut()
                    .find(|r| matches!(r.slot.state, ReachState::Free))
                {
                    encoder.copy_buffer_to_buffer(
                        &o.requests,
                        0,
                        &r.slot.buffer,
                        0,
                        SUN_REQUEST_BYTES,
                    );
                    r.slot.state = ReachState::Copied;
                    r.asking = Some(asking);
                }
            }
        }
        // Ray reports, back without waiting (a frame's copy when a slot is
        // free): the nodes the sampled hits want, and the Buried nodes rays
        // met.
        let room = u64::from(size[0].div_ceil(FEEDBACK_BLOCK))
            * u64::from(size[1].div_ceil(FEEDBACK_BLOCK));
        if self
            .feedback_slots
            .iter()
            .any(|s| matches!(s.state, ReachState::Free))
        {
            let (distance, visibility) = (
                self.gpu_cache.view(&targets.distance),
                self.gpu_cache.view(&targets.visibility),
            );
            let feedback_group = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk feedback"),
                    layout: &self.layouts.feedback,
                    entries: &[
                        view_entry(4, &distance),
                        view_entry(7, &visibility),
                        entry(24, &self.feedback),
                    ],
                },
            );
            encoder.clear_buffer(&self.feedback, 0, Some(16));
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("chunk feedback"),
                    timestamp_writes: None,
                });
                pass.set_bind_group(1, &feedback_group, &[]);
                for (index, group, rect) in &groups {
                    pass.set_pipeline(&self.pipelines_of(*index).feedback);
                    pass.set_bind_group(0, group, &[]);
                    let (x, y) = rect_groups(*rect, 32);
                    pass.dispatch_workgroups(x, y, 1);
                }
            }
            let slot = self
                .feedback_slots
                .iter_mut()
                .find(|s| matches!(s.state, ReachState::Free))
                .unwrap();
            slot.bytes = (16 + room * 16).min(FEEDBACK_BYTES);
            encoder.copy_buffer_to_buffer(&self.feedback, 0, &slot.buffer, 0, slot.bytes);
            slot.state = ReachState::Copied;
        }
        if let Some(slot) = self
            .buried_slots
            .iter_mut()
            .find(|s| matches!(s.state, ReachState::Free))
        {
            encoder.copy_buffer_to_buffer(&self.buried, 0, &slot.buffer, 0, BURIED_BYTES);
            slot.state = ReachState::Copied;
        }
        if let Some((set, resolve, ring)) = timing {
            // Resolve offsets are 256-byte aligned.
            let at = u64::from(ring) * u64::from(wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT);
            encoder.resolve_query_set(set, QUERIES * ring..QUERIES * ring + QUERIES, resolve, at);
            let slot = &mut self.timing_slots[ring as usize];
            encoder.copy_buffer_to_buffer(resolve, at, &slot.buffer, 0, u64::from(QUERIES) * 8);
            slot.frame = Some((frame, self.last_generated.0, self.last_generated.1));
            self.timing_now = None;
        }
        self.last_groups = Some(DrawGroups {
            volumes: groups,
            ground: cross
                .into_iter()
                .filter(|c| c.ground)
                .map(|c| (c.index, c.group))
                .collect(),
        });
    }

    /// The traces' counts since the last call (with
    /// `HELIO_VOXEL_CHUNKS_COUNT`).
    pub fn take_counts(&self) -> TraceCounts {
        let words: Vec<[u32; 4]> = self.read(&self.counters, 64);
        self.queue.write_buffer(&self.counters, 0, &[0u8; 1024]);
        TraceCounts {
            seeded: words[0][0],
            steps: words[0][1],
            hits: words[0][2],
            boxes: words[0][3],
            missed: words[2][0],
            missed_boxes: words[2][1],
            lost: words[2][2],
            lost_pixel: (words[3][2] & 0xffff, words[3][2] >> 16),
            lost_sites: [
                words[1][1],
                words[1][2],
                words[1][3],
                words[2][3],
                words[3][0],
                words[3][1],
                words[3][3],
            ],
            rock_ends: [words[60][0], words[60][1]],
            rock_pixels: [words[60][2], words[60][3]].map(|p| (p & 0xffff, p >> 16)),
            rock_nodes: [
                [words[61][0], words[61][1], words[61][2], words[62][2]].map(|w| w as i32),
                [words[61][3], words[62][0], words[62][1], words[62][3]].map(|w| w as i32),
            ],
        }
    }

    /// The boxes the debug pixel's ray crossed (counting builds): the frame
    /// chunk's slot, the box's low corner in its cells, whether it is that
    /// chunk, its level; and its key and layer.
    pub fn pixel_trace(&self) -> Vec<(u32, [i32; 3], bool, u32, Option<NodeKey>)> {
        let words: Vec<[u32; 4]> = self.read(&self.counters, 64);
        let flat: Vec<u32> = words.iter().flatten().copied().collect();
        let n = flat[4] as usize;
        (0..n.min(24))
            .map(|c| {
                let w = &flat[16 + c * 5..21 + c * 5];
                let slot = w[0];
                let owner = self
                    .nodes
                    .iter()
                    .find(|(_, c)| c.slot == slot)
                    .map(|(k, _)| *k);
                (
                    slot,
                    [w[1] as i32, w[2] as i32, w[3] as i32],
                    w[4] & 1 != 0,
                    w[4] >> 1,
                    owner,
                )
            })
            .collect()
    }

    /// Where the last search past the top began (counting builds): face,
    /// level and cell. Read before `take_counts` (which clears it).
    pub fn lost_where(&self) -> (u8, u32, [i32; 3]) {
        let words: Vec<[u32; 4]> = self.read(&self.counters, 64);
        let w = words[50];
        (
            (w[0] & 0xff) as u8,
            w[0] >> 8,
            [w[1] as i32, w[2] as i32, w[3] as i32],
        )
    }

    /// Boxes drawn by the last [`Self::draw`]: from outside, from inside
    /// (reads the cull back: diagnostics).
    /// Follows the GI clipmap `clipmap`, around world point `world_eye`
    /// (the eye, at `eye` in the planet's coordinates): the base cell of
    /// each probe that came in, located in f64, for its rays' walks.
    /// World boxes (planet coordinates) whose drawn ground changed since the last
    /// call: where foliage is placed again (`chunks::ground`).
    pub fn take_ground_changes(&mut self) -> Vec<[DVec3; 2]> {
        std::mem::take(&mut self.ground_changes)
    }

    /// Answer foliage's ground queries (`chunks::ground`): `queries` (render space,
    /// `tiles.len()` tiles of `per_tile` each) walked down the drawn cut into `hits`, for
    /// an eye at world `eye` and render-space `camera`. After [`Self::draw`] (its cut and
    /// bind group); recorded on `encoder`.
    ///
    /// A walk starts only where drawn chunks are near: each tile's queries are walked
    /// from a little over the highest terrain `planet` has at the tile's corners and
    /// centre (overhangs included), the same segments below that. Features taller than
    /// that start the walk in them: no ground under a tree's trunk or in a building.
    #[allow(clippy::too_many_arguments)]
    pub fn probe_ground(
        &mut self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        queries: &wgpu::Buffer,
        hits: &wgpu::Buffer,
        tiles: &[helio_foliage_api::AskedTile],
        planet: &Planet,
        eye: DVec3,
        camera: Vec3,
    ) {
        // Foliage is tiled on the primary volume: its walks in its frame.
        let Some(primary) = self.primary else { return };
        let Some((_, group, _)) = self
            .last_groups
            .as_ref()
            .and_then(|g| g.volumes.iter().find(|(i, ..)| *i == primary).cloned())
        else {
            return;
        };
        let vol = self.volumes[usize::from(primary)].as_ref().unwrap();
        let (grid, transform, plane) = (vol.grid, vol.transform, vol.grid.is_plane());
        let eye = transform.to_local(eye);
        let queries_in_all = tiles
            .iter()
            .take(PROBE_TILES)
            .map(|t| t.first + t.count)
            .max()
            .unwrap_or(0);
        let margin = planet.overhang_height() + PROBE_MARGIN_M;
        let places: Vec<ground::ProbeTile> = tiles
            .iter()
            .take(PROBE_TILES)
            .map(|t| {
                // The tile's frame in the volume's.
                let f = &helio_foliage_api::TileFrame {
                    origin: transform.to_local(t.frame.origin),
                    axis_u: transform.dir_to_local(t.frame.axis_u),
                    axis_v: transform.dir_to_local(t.frame.axis_v),
                    up: transform.dir_to_local(t.frame.up),
                };
                let top = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)]
                    .into_iter()
                    .map(|(u, v)| {
                        let (cell, _) = grid.locate(f.origin + f.axis_u * u + f.axis_v * v);
                        // A bound is enough (the probe starts over it):
                        // the field without its streams, cheap on the CPU.
                        grid.layer_radius(f64::from(
                            planet.column_top_bound(cell.face, cell.i, cell.j, 0),
                        )) - grid.radius()
                    })
                    .fold(f64::NEG_INFINITY, f64::max);
                ground::ProbeTile::new(&grid, f, t.start_height, top + margin, eye, t.first)
                    .unwrap_or_default()
            })
            .collect();
        if places.is_empty() || queries_in_all == 0 {
            return;
        }
        self.queue
            .write_buffer(&self.probe_tiles, 0, bytemuck::cast_slice(&places));
        let frame = ground::ProbeFrame {
            eye: [camera.x, camera.y, camera.z, 0.0],
            counts: [queries_in_all, places.len() as u32, 0, 0],
        };
        self.queue
            .write_buffer(&self.probe_frame, 0, bytemuck::bytes_of(&frame));
        let probe_group = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk ground probes"),
                layout: &self.layouts.ground,
                entries: &[
                    entry(58, &self.probe_frame),
                    entry(59, queries),
                    entry(60, hits),
                    entry(61, &self.probe_tiles),
                ],
            },
        );
        let count = queries_in_all;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("chunk ground probes"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, &probe_group, &[]);
        pass.set_pipeline(
            &self.pipelines[usize::from(plane)]
                .as_ref()
                .unwrap()
                .ground_probe,
        );
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        // The other volumes near the eye, where they stand on that ground or
        // hold it (`ground_probe_cross`).
        for (index, group) in self.last_groups.iter().flat_map(|g| &g.ground) {
            pass.set_bind_group(0, group, &[]);
            pass.set_pipeline(&self.cross_pipelines(*index).ground_probe);
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
    }

    pub fn follow_gi(&mut self, clipmap: helio_gi_api::Clipmap, world_eye: DVec3, eye: DVec3) {
        let Some(vol) = self
            .primary
            .and_then(|p| self.volumes[usize::from(p)].as_ref())
        else {
            return;
        };
        // Probes are located in the primary volume's frame; a volume that
        // moves relocates them all.
        let (grid, transform) = (vol.grid, vol.transform);
        let moved = vol.drawn_transform[0].is_some_and(|t| t != transform);
        if self.gi_clipmap == Some(clipmap) && !moved {
            return;
        }
        let entered = if moved {
            (0..helio_gi_api::PROBES).collect()
        } else {
            clipmap.entered(self.gi_clipmap.as_ref())
        };
        self.gi_clipmap = Some(clipmap);
        let (mut lo, mut hi) = (usize::MAX, 0);
        for &index in &entered {
            let (cell, _) =
                grid.locate(transform.to_local(eye + (clipmap.position(index) - world_eye)));
            self.gi_cell_data[index as usize] = [cell.i, cell.j, cell.k, i32::from(cell.face) | 8];
            lo = lo.min(index as usize);
            hi = hi.max(index as usize + 1);
        }
        if lo < hi {
            self.queue.write_buffer(
                &self.gi_cells,
                lo as u64 * 16,
                bytemuck::cast_slice(&self.gi_cell_data[lo..hi]),
            );
        }
    }

    /// Boxes the last volume traced drew: from outside, from inside (reads
    /// the cull back: diagnostics).
    pub fn drawn(&self) -> (u32, u32) {
        let counts: Vec<u32> = self.read(&self.cull_state, 2);
        (counts[1] - counts[0], counts[0])
    }

    /// This frame's timestamp slot, when it is free again (its last
    /// frame's times were read).
    fn claim_timing(&mut self, frame: u64) {
        let ring = (frame % TIMING_RING) as usize;
        self.timing_now = self
            .timing_slots
            .get(ring)
            .filter(|t| t.frame.is_none() && t.ready.is_none())
            .map(|_| ring);
    }

    /// Shade volume `id` with `appearance` (the terrain's materials and
    /// detail).
    pub fn set_appearance(&mut self, id: VolumeId, appearance: &crate::engine::TerrainAppearance) {
        let Some(vol) = self.volume(id) else { return };
        let (rows, variation, detail, shape) = crate::engine::material_table(appearance);
        self.queue.write_buffer(
            &vol.materials,
            0,
            bytemuck::bytes_of(&ChunkMaterials {
                rows,
                variation,
                detail,
                shape,
            }),
        );
    }

    /// The share of each pixel's light the last draw's sun samples gave
    /// stochastically (helio-pass-tsr's `tsr_stochastic`).
    pub fn stochastic_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.stochastic_view)
    }

    /// The last draw's sun visibility (Helio's `directional_visibility`).
    pub fn sun_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.sun_view)
    }

    /// The last draw's local light visibility (Helio's `local_visibility`).
    /// None before a draw with lights (every light seen).
    pub fn local_visibility(&self) -> Option<&wgpu::Buffer> {
        self.targets
            .as_ref()
            .and_then(|t| t.local.as_ref())
            .map(|l| &l.visibility)
    }

    /// The local lights the last draw's walks saw past the terrain, before
    /// denoising: two words a pixel, bit i of the first the tile list's
    /// light i, the second 1 where they are its own walks'.
    pub fn local_walked(&self) -> Option<&wgpu::Buffer> {
        self.targets.as_ref().map(|t| &t.local_seen)
    }

    /// The last draw's hits into Helio's gbuffer (`helio_frame_api::gbuffer::TARGET_FORMATS`)
    /// and depth (Depth32Float, tested against what is there).
    pub fn publish(
        &self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        gbuffer: [&wgpu::TextureView; 8],
        depth: &wgpu::TextureView,
    ) {
        self.publish_scene(encoder, gbuffer, depth, None);
    }

    /// Voxel materials show the scene material rows of `map` (written to the
    /// GPU when it changes).
    pub fn set_material_map(&mut self, map: &scene::VoxelMaterialMap) {
        let Some(scene) = &mut self.scene else { return };
        if scene.map_written.as_ref() != Some(map) {
            self.queue
                .write_buffer(&scene.map, 0, bytemuck::bytes_of(map));
            scene.map_written = Some(*map);
        }
    }

    /// 4c. The shadows' denoiser (`chunk_draw.wgsl`): the sun's and the
    /// sky's history and two filter passes into `directional_visibility`
    /// and the gbuffer's sky; with lights, theirs into `local_visibility`.
    #[allow(clippy::too_many_arguments)]
    fn denoise(
        &self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        group: &wgpu::BindGroup,
        pipelines: &volumes::DrawPipelines,
        size: [u32; 2],
        lights: Option<&SceneLights<'_>>,
        eye_index: usize,
        timing: Option<(&wgpu::QuerySet, &wgpu::Buffer, u32)>,
    ) {
        let targets = self.targets.as_ref().unwrap();
        let eye_index = eye_index.min(1);
        let (now, last) = (
            (targets.denoised[eye_index] & 1) as usize,
            (!targets.denoised[eye_index] & 1) as usize,
        );
        let (distance, visibility, normal, sun_walked) = (
            self.gpu_cache.view(&targets.distance),
            self.gpu_cache.view(&targets.visibility),
            self.gpu_cache.view(&targets.normal),
            &targets.sun_walked,
        );
        let (history_pair, geometry_pair, short_pair) = if eye_index == 1 {
            (
                targets.right_history.as_ref().unwrap(),
                targets.right_geometry.as_ref().unwrap(),
                targets.right_history_short.as_ref().unwrap(),
            )
        } else {
            (&targets.history, &targets.geometry, &targets.history_short)
        };
        let (history, history_last, geometry, geometry_last) = (
            &history_pair[now],
            &history_pair[last],
            &geometry_pair[now],
            &geometry_pair[last],
        );
        let (short, short_last) = (&short_pair[now], &short_pair[last]);
        let filtered = &targets.filtered;
        let temporal = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk shadow history"),
                layout: &self.layouts.temporal,
                entries: &[
                    view_entry(4, &distance),
                    view_entry(7, &visibility),
                    view_entry(29, &normal),
                    entry(40, sun_walked),
                    entry(42, history_last),
                    entry(43, geometry_last),
                    entry(54, short_last),
                    entry(44, history),
                    entry(45, geometry),
                    entry(55, short),
                    entry(48, &filtered[0]),
                ],
            },
        );
        let first = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk shadow filter"),
                layout: &self.layouts.filter,
                entries: &[
                    view_entry(4, &distance),
                    entry(46, &filtered[0]),
                    entry(47, geometry),
                    entry(48, &filtered[1]),
                ],
            },
        );
        let second = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk shadow filter"),
                layout: &self.layouts.filter,
                entries: &[
                    view_entry(4, &distance),
                    entry(46, &filtered[1]),
                    entry(47, geometry),
                    entry(48, &filtered[0]),
                ],
            },
        );
        let resolve = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk shadow resolve"),
                layout: &self.layouts.resolve_shadows,
                entries: &[
                    view_entry(4, &distance),
                    view_entry(7, &visibility),
                    view_entry(29, &normal),
                    entry(40, sun_walked),
                    entry(46, &filtered[0]),
                    entry(47, geometry),
                    view_entry(2, &self.gpu_cache.view(&targets.colour)),
                    view_entry(8, &targets.sun_view),
                    view_entry(49, &self.gpu_cache.view(&targets.sky)),
                ],
            },
        );
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("chunk shadow denoise"),
            timestamp_writes: timing.map(|(set, _, ring)| wgpu::ComputePassTimestampWrites {
                query_set: set,
                beginning_of_pass_write_index: None,
                end_of_pass_write_index: Some(QUERIES * ring + 11),
            }),
        });
        // 16x16 pixels a workgroup (the local lights': 8x8, in a light tile).
        pass.set_bind_group(0, group, &[]);
        for (pipeline, bind) in [
            (&pipelines.temporal, &temporal),
            (&pipelines.filter[0], &first),
            (&pipelines.filter[1], &second),
            (&pipelines.resolve_shadows, &resolve),
        ] {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, bind, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(16), size[1].div_ceil(16), 1);
        }
        if let (Some(lights), Some(local)) = (lights, &targets.local) {
            let history = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk local light history"),
                    layout: &self.layouts.local_denoise[0],
                    entries: &[
                        view_entry(4, &distance),
                        view_entry(7, &visibility),
                        view_entry(29, &normal),
                        entry(40, sun_walked),
                        entry(43, geometry_last),
                        entry(30, &targets.local_seen),
                        entry(31, lights.lights),
                        entry(32, lights.tile_lists),
                        entry(33, lights.tile_counts),
                        entry(
                            51,
                            if eye_index == 1 {
                                &local.right_history.as_ref().unwrap()[last]
                            } else {
                                &local.history[last]
                            },
                        ),
                        entry(
                            52,
                            if eye_index == 1 {
                                &local.right_history.as_ref().unwrap()[now]
                            } else {
                                &local.history[now]
                            },
                        ),
                        entry(53, &local.visibility),
                        entry(57, &local.dispatch),
                    ],
                },
            );
            let gate = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk local light gate"),
                    layout: &self.layouts.local_denoise[2],
                    entries: &[
                        view_entry(4, &distance),
                        entry(31, lights.lights),
                        entry(56, &local.dispatch),
                    ],
                },
            );
            // None without local lights in the scene.
            pass.set_pipeline(&pipelines.local_gate);
            pass.set_bind_group(1, &gate, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            let filter = self.gpu_cache.bind_group(
                &self.device,
                &wgpu::BindGroupDescriptor {
                    label: Some("chunk local light filter"),
                    layout: &self.layouts.local_denoise[1],
                    entries: &[
                        view_entry(4, &distance),
                        entry(47, geometry),
                        entry(
                            52,
                            if eye_index == 1 {
                                &local.right_history.as_ref().unwrap()[now]
                            } else {
                                &local.history[now]
                            },
                        ),
                        entry(53, &local.visibility),
                    ],
                },
            );
            for (pipeline, bind) in [
                (&pipelines.local_temporal, &history),
                (&pipelines.local_filter, &filter),
            ] {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(1, bind, &[]);
                pass.dispatch_workgroups_indirect(&local.dispatch, 0);
            }
        }
    }

    /// The sun's angular radius (radians; Earth's by default) where no
    /// atmosphere frame gives one: its shadows' penumbrae.
    pub fn set_sun_angular_radius(&mut self, radians: f32) {
        self.sun_angular_radius = radians;
    }

    /// The host's atmosphere frame (helio-pass-sky's `atmosphere_frame`):
    /// its sun disk's angular radius, read on the GPU, sizes the shadows'
    /// penumbrae.
    pub fn set_atmosphere_frame(&mut self, frame: Option<wgpu::Buffer>) {
        self.atmosphere_frame = frame;
    }

    /// Every hit walks the sun's disk (SUN_DISK_RAYS rays) and the sky rays
    /// hold still (steady shadows, dearer: the reference) instead of
    /// samples that move each frame; the denoiser passes the walks' own on.
    pub fn set_steady_shadows(&mut self, on: bool) {
        self.steady_shadows = on;
    }

    /// [`Self::publish`], textured by the host's scene materials where
    /// `inputs` are given, a map is set and the device supports them.
    pub fn publish_scene(
        &self,
        encoder: &mut helio_core::CommandRecorder<'_>,
        gbuffer: [&wgpu::TextureView; 8],
        depth: &wgpu::TextureView,
        inputs: Option<scene::SceneMaterialInputs<'_>>,
    ) {
        let Some(targets) = &self.targets else { return };
        let Some(groups) = &self.last_groups else {
            return;
        };
        let resolve_group = self.gpu_cache.bind_group(
            &self.device,
            &wgpu::BindGroupDescriptor {
                label: Some("chunk gbuffer"),
                layout: &self.layouts.resolve,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(
                            &self.gpu_cache.view(&targets.distance),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(
                            &self.gpu_cache.view(&targets.visibility),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: wgpu::BindingResource::TextureView(
                            &self.gpu_cache.view(&targets.normal),
                        ),
                    },
                    view_entry(28, &self.gpu_cache.view(&targets.sky)),
                    view_entry(25, &self.uv_grid_view),
                    wgpu::BindGroupEntry {
                        binding: 26,
                        resource: wgpu::BindingResource::Sampler(&self.uv_grid_sampler),
                    },
                ],
            },
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chunk gbuffer"),
            color_attachments: &gbuffer.map(loaded),
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // The host's scene materials, when a map is written and the table is
        // the length the pipeline was built for.
        let scene = self.scene.as_ref().zip(inputs).filter(|(s, i)| {
            s.map_written.is_some()
                && i.texture_views.len() == s.capacity as usize
                && i.samplers.len() == s.capacity as usize
        });
        let scene_group = scene.as_ref().map(|(s, i)| {
            let mut cached = s.group.lock().unwrap();
            let version = (
                i.version.0,
                i.version.1,
                i.water_shore_fields.is_some() && i.water_shore_params.is_some(),
            );
            if cached
                .as_ref()
                .map(|(cached_version, _, _)| *cached_version)
                != Some(version)
            {
                let group = self.gpu_cache.bind_group(
                    &self.device,
                    &wgpu::BindGroupDescriptor {
                        label: Some("chunk scene materials"),
                        layout: &s.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: s.map.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: i.materials.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureViewArray(i.texture_views),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::SamplerArray(i.samplers),
                            },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: wgpu::BindingResource::TextureView(
                                    i.water_shore_fields.unwrap_or(&s.swash_fallback_view),
                                ),
                            },
                        ],
                    },
                );
                let shore_params = self.gpu_cache.bind_group(
                    &self.device,
                    &wgpu::BindGroupDescriptor {
                        label: Some("chunk water shore parameters"),
                        layout: &s.shore_params_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: i
                                .water_shore_params
                                .unwrap_or(&s.swash_fallback_params)
                                .as_entire_binding(),
                        }],
                    },
                );
                *cached = Some((version, group, shore_params));
            }
            cached
                .as_ref()
                .map(|(_, materials, shore)| (materials.clone(), shore.clone()))
                .unwrap()
        });
        if let Some((textures, shore_params)) = &scene_group {
            pass.set_bind_group(2, textures, &[]);
            pass.set_bind_group(3, shore_params, &[]);
        }
        pass.set_bind_group(1, &resolve_group, &[]);
        // Each volume's hits by its own draw (its frame, its materials).
        for (index, group, rect) in &groups.volumes {
            let plane = self.volumes[usize::from(*index)]
                .as_ref()
                .is_some_and(|v| v.grid.is_plane());
            let pipelines = self.pipelines[usize::from(plane)].as_ref().unwrap();
            match (&pipelines.publish_scene, &scene_group) {
                (Some(textured), Some(_)) => pass.set_pipeline(textured),
                _ => pass.set_pipeline(&pipelines.publish),
            }
            pass.set_bind_group(0, group, &[]);
            if rect[2] <= rect[0] || rect[3] <= rect[1] {
                continue;
            }
            pass.set_scissor_rect(rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Frames whose GPU times came back since the last call.
    pub fn take_timings(&mut self) -> Vec<FrameTimings> {
        std::mem::take(&mut self.timings)
    }

    /// Pack every resident edited chunk not kept yet (over the next
    /// updates, as many a frame as packs fit), so [`Self::kept_chunks`]
    /// holds the whole world's edited chunks: before saving.
    pub fn keep_resident(&mut self) {
        let keys: Vec<NodeKey> = self
            .nodes
            .iter()
            .filter(|(k, c)| c.edited && !self.kept.contains_key(k))
            .map(|(k, _)| *k)
            .collect();
        self.to_pack = keys;
    }

    /// Edited chunks [`Self::keep_resident`] still has to pack or read back.
    pub fn keeping(&self) -> usize {
        self.to_pack.len() + self.packed_since.len()
    }

    /// The kept chunks, for `planet`'s edits as this world follows them
    /// (after [`Self::keep_resident`]: the whole world's edited chunks):
    /// what a save stores ([`KeptChunks::to_bytes`]). Shares the packs.
    pub fn kept_chunks(&self, id: VolumeId, planet: &Planet) -> KeptChunks {
        let edits = planet.edits();
        let mut chunks: Vec<_> = self
            .kept
            .iter()
            .filter(|(key, _)| key.volume == id.0)
            .map(|(key, kept)| (*key, kept.flags, kept.measured, kept.pack.clone()))
            .collect();
        chunks.sort_unstable_by_key(|(k, ..)| (k.face, k.level, k.cx, k.cy, k.cz));
        KeptChunks {
            fingerprint: self.volume(id).map_or(0, |v| v.fingerprint),
            history: (edits.len() as u64, edits.hash()),
            chunks,
        }
    }

    /// `kept`'s chunks kept, restored by copy when wanted: how many. Before
    /// the volume's first update or after `sync_edits` resynced it;
    /// refused when they were made by other generation code, terrain or
    /// world, or for another history than `planet`'s.
    pub fn load_kept(
        &mut self,
        id: VolumeId,
        planet: &Planet,
        kept: &KeptChunks,
    ) -> Result<usize, String> {
        let Some(vol) = self.volume(id) else {
            return Err("no such volume".into());
        };
        if kept.fingerprint != vol.fingerprint {
            return Err("kept chunks of other generation code, terrain or world".into());
        }
        let edits = planet.edits();
        if kept.history != (edits.len() as u64, edits.hash()) {
            return Err("kept chunks of another history".into());
        }
        let frame = self.frame_now;
        for (key, flags, measured, pack) in &kept.chunks {
            self.keep(
                NodeKey {
                    volume: id.0,
                    ..*key
                },
                Kept {
                    pack: pack.clone(),
                    flags: *flags,
                    measured: *measured,
                    used: frame,
                },
            );
        }
        Ok(kept.chunks.len())
    }

    /// Every resident node generated again (benchmarks: the same work
    /// each time), drawn as it is meanwhile.
    pub fn regenerate_all(&mut self) {
        let keys: Vec<NodeKey> = self.nodes.keys().copied().collect();
        for key in keys {
            if let Some(vol) = self.volumes[usize::from(key.volume)].as_mut() {
                vol.stale.insert(key);
            }
        }
    }

    /// The footprints the pool keeps with their slots, and the buffer of
    /// their placements (`CompiledFeatures::stride` word pairs a slot, a
    /// footprint's near spawns' squares as its level lays them out): what
    /// tests compare with the CPU's.
    pub fn footprint_placements(&self, id: VolumeId) -> (Vec<(Footprint, u32)>, &wgpu::Buffer) {
        let vol = self.volume(id).expect("a volume of this world");
        (
            vol.pool
                .slots
                .iter()
                .map(|(&fp, &slot)| (fp, slot))
                .collect(),
            &vol.tile_placements,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{moved_from, walk_templates, world_moved_from, CostLine, GenCost};
    use glam::{DVec3, Mat4};

    /// Both walks come out of the templates whatever ends the lines (a
    /// checkout's \r\n lost the fine ones).
    /// A batch's fixed part apart from its tiles': batches of spread sizes
    /// find both; batches of one size find neither (no line).
    #[test]
    fn generation_cost_separates_a_batch_from_its_tiles() {
        let mut line = CostLine::EMPTY;
        for n in 0..200 {
            let tiles = 8 + (n * 37) % 120;
            line.add(tiles as f64, 0.2 + 0.01 * tiles as f64);
        }
        let (fixed, per) = line.fit().unwrap();
        assert!(
            (fixed - 0.2).abs() < 1e-6 && (per - 0.01).abs() < 1e-8,
            "{fixed} {per}"
        );
        let mut same = CostLine::EMPTY;
        for _ in 0..50 {
            same.add(14.0, 0.34);
        }
        assert!(same.fit().is_none());
        // Its budget: a 3 ms frame holds 280 such tiles, not the 75 their
        // small batches' average (0.04 ms) would allow.
        let cost = GenCost {
            tiles: line,
            chunks: CostLine::EMPTY,
            chunk: 0.0,
            ..GenCost::PRIOR
        };
        let (fixed, tile, _) = cost.model();
        assert_eq!(((3.0 - fixed) / tile).round(), 280.0);
    }

    /// Both shaders parse and validate as naga sees them, for spheres and
    /// planes, plain and counting builds (no GPU needed).
    #[test]
    fn chunk_shaders_validate() {
        let planet = crate::Planet::new(crate::PlanetRecipe::default()).unwrap();
        let program = planet.field().program();
        for plane in [false, true] {
            for counting in [false, true] {
                let [generate, (label, draw)] =
                    super::shader_sources(plane, &program, false, counting);
                // The draw again with the scene materials spliced in.
                let textured = (
                    "chunk draw (scene materials)",
                    super::scene::scene_source(&draw, 64),
                );
                let verify = (
                    "terrain verification",
                    crate::engine::verify_source(plane, &program, false),
                );
                for (label, source) in [generate, (label, draw), textured, verify] {
                    // HELIO_DUMP_SHADERS=<dir>: the sources, for offline tools.
                    if let Some(dir) = std::env::var_os("HELIO_DUMP_SHADERS") {
                        let name = format!("{label} {plane} {counting}.wgsl").replace(' ', "_");
                        std::fs::write(std::path::Path::new(&dir).join(name), &source).unwrap();
                    }
                    let module = naga::front::wgsl::parse_str(&source)
                        .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(&source)));
                    naga::valid::Validator::new(
                        naga::valid::ValidationFlags::all(),
                        naga::valid::Capabilities::all(),
                    )
                    .validate(&module)
                    .unwrap_or_else(|e| {
                        panic!(
                            "{label} (plane {plane}, counting {counting}): {}",
                            e.emit_to_string(&source)
                        )
                    });
                }
            }
        }
    }

    /// Kept chunks' bytes hold each chunk's footprint measurement, quarter
    /// by quarter (one past a face's edge measured nothing; a chunk kept
    /// before its footprint's came back has none).
    #[test]
    fn kept_chunks_keep_their_footprints_measurement() {
        use super::{KeptChunks, NodeKey, Ranges};
        let r = |a: i32| Ranges {
            volume: (a - 9, a + 3),
            surface: (a - 2, a + 3),
        };
        let chunks = vec![
            (
                NodeKey {
                    volume: 0,
                    face: 2,
                    level: 0,
                    cx: 7,
                    cy: -3,
                    cz: 1,
                },
                5,
                Some([Some(r(10)), None, Some(r(-40)), Some(r(7))]),
                vec![1u32, 2, 3].into(),
            ),
            (
                NodeKey {
                    volume: 0,
                    face: 4,
                    level: 3,
                    cx: 1,
                    cy: 2,
                    cz: -1,
                },
                2,
                None,
                Vec::new().into(),
            ),
        ];
        let kept = KeptChunks {
            fingerprint: 11,
            history: (3, 99),
            chunks,
        };
        let back = KeptChunks::from_bytes(&kept.to_bytes()).unwrap();
        assert_eq!((back.fingerprint, back.history), (11, (3, 99)));
        for (a, b) in kept.chunks.iter().zip(&back.chunks) {
            assert_eq!((a.0, a.1, a.2, &a.3[..]), (b.0, b.1, b.2, &b.3[..]));
        }
        assert_eq!(back.chunks.len(), 2);
    }

    /// A moving, spinning volume's points far from the origin: the last
    /// frame's position of a point the eye sees, by the matrices the draw
    /// and the denoiser use, is where the last transform put it (relative
    /// to this frame's eye), to a millimetre at 10 000 km.
    #[test]
    fn moved_points_project_from_where_they_were() {
        use crate::volume::VolumeTransform;
        use glam::DQuat;
        let now = VolumeTransform::new(
            DVec3::new(1.0e7, -2.0e6, 3.5e6),
            DQuat::from_axis_angle(DVec3::new(0.2, 1.0, -0.4).normalize(), 0.8),
        );
        let prev = VolumeTransform::new(
            now.position - DVec3::new(31.0, -4.5, 12.25),
            DQuat::from_axis_angle(DVec3::new(0.2, 1.0, -0.4).normalize(), 0.79999),
        );
        let eye = now.to_world(DVec3::new(6_371_002.0, 5.0, -3.0));
        let local_eye = now.to_local(eye);
        for local in [
            DVec3::new(6_371_000.0, 2.0, 4.0),
            DVec3::new(6_370_990.0, -7.0, 1.0),
        ] {
            let was = prev.to_world(local) - eye;
            let by_local = moved_from(&prev, local_eye, eye)
                .transform_point3((local - local_eye).as_vec3())
                .as_dvec3();
            assert!(by_local.distance(was) < 1e-3, "{by_local} {was}");
            let by_world = world_moved_from(&now, &prev, eye)
                .transform_point3((now.to_world(local) - eye).as_vec3())
                .as_dvec3();
            assert!(by_world.distance(was) < 1e-3, "{by_world} {was}");
        }
        // Still: the identity.
        let still = world_moved_from(&now, &now, eye);
        assert!(still.abs_diff_eq(Mat4::IDENTITY, 1e-6), "{still}");
    }

    #[test]
    fn walk_templates_expand_with_any_line_ends() {
        for end in ["\n", "\r\n"] {
            let source = [
                "head",
                "// WALK {",
                "fn hit_walk() { walk(Walk(0)); }",
                "// } WALK",
                "tail",
                "",
            ]
            .join(end);
            let out = walk_templates(&source);
            assert!(out.contains("fn hit_walk() { walk(Walk(0)); }"), "{out:?}");
            assert!(
                out.contains("fn hit_walk_fine() { walk_fine(Walk(0)); }"),
                "{out:?}"
            );
            assert!(
                !out.contains("WALK {")
                    && out.starts_with("head")
                    && out.trim_end().ends_with("tail"),
                "{out:?}"
            );
        }
    }
}
