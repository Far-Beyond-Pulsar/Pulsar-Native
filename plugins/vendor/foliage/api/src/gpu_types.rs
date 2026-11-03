//! GPU-visible foliage records: blade instances, tile headers and type descriptors.
//!
//! These three structs are the entire contract between `FoliagePlacePass` (which writes
//! blades and tiles) and `FoliageGBufferPass` (which reads them), and they are mirrored
//! field-for-field in WGSL. Their sizes are asserted at compile time in the style of
//! the removed virtual-geometry pass, because a size change that the shader does not follow does not
//! fail loudly — the shader reads every field from the wrong offset and you get a field
//! of grass with garbage rotations, which is a full day of debugging away from "someone
//! added a `u32`".
//!
//! # WGSL mirroring rules
//!
//! Declare these in WGSL using **scalar** fields, never `vec3<f32>`. WGSL gives `vec3`
//! a 16-byte alignment, so a single `vec3<f32>` in the middle of a struct silently
//! shifts every field after it and breaks the layout that these asserts are protecting.
//! `vec2<f32>` (8-byte aligned) and `vec4` are safe where the Rust offset is already
//! aligned to match; the per-field comments below say which is which.

use bytemuck::{Pod, Zeroable};

use super::packing::{
    f16_bits_to_f32, f32_to_f16_bits, pack_unorm16, pack_unorm8, unpack_unorm16, unpack_unorm8,
};

// ═══════════════════════════════════════════════════════════════════════════════
// Blade instance
// ═══════════════════════════════════════════════════════════════════════════════

/// One placed blade of a tuft, or (in the far bands) a whole tuft. Exactly 16 bytes.
///
/// 16 bytes is not an arbitrary target: the blade arena is the single largest foliage
/// allocation. Every byte added here costs a proportional slice of the arena and of the
/// placement pass's write bandwidth, which is the dominant cost when the camera moves
/// fast enough to refill the ring perimeter. If a field genuinely will not fit, add a
/// parallel sparse buffer indexed by blade id rather than widening this record.
///
/// A placement candidate is a *tuft*: in the near bands it is written as one record per
/// blade (`blade` 0..`blades`, the rest identical), in the far bands as one record for
/// the tuft (`blade` 0), drawn as one shape. Each blade's own shape (direction, lean,
/// height, whether it lies over) is a function of the record's seed, its index and its
/// type, so nothing else needs storing.
///
/// Positions are tile-local so a blade's encoding does not depend on where in the world
/// its tile sits, which is what lets placement be a pure function of
/// `(tile_coord, lane, generation)` and therefore reproducible across GPUs — see
/// [`crate::blade_seed`].
///
/// # Layout (16 bytes, 4-byte aligned)
/// ```text
///  0..4   packed_pos:          u32  U unorm16 (bits 0..16) | V unorm16 (bits 16..32)
///  4..8   packed_height_tuft:  u32  height offset f16 (bits 0..16) | blade u4 | blades u4 | rank u8
///  8..12  packed_scale_type:   u32  height u8 | aux u8 | type id u8 | variant u8
/// 12..16  packed_colour_seed:  u32  ground colour rgb565 (bits 0..16, 0: none) | seed u16
/// ```
#[repr(C)]
// `PartialEq`/`Eq` are part of the determinism contract, not a convenience: the placement
// tests assert that re-placing a tile yields a byte-identical blade list, and comparing
// packed records directly is what makes that assertion mean "identical".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuBladeInstance {
    /// The tuft's tile-local position as two 16-bit unorms over the tile's axes.
    ///
    /// 16 bits over a 128 m far tile is 2 mm; the precision is spent on making candidates
    /// land exactly where they were asked (on a voxel's cube grid, at the point the ground
    /// answered for).
    pub packed_pos: u32,

    /// Ground height offset in metres as f16 (bits 0..16) | the blade's index in its tuft
    /// (bits 16..20) | the tuft's blade count (bits 20..24) | the tuft's rank (bits
    /// 24..32, unorm8).
    ///
    /// The height offset is relative to the tile's `bounds_center`, not absolute: f16 has
    /// ~3 decimal digits, centimetres at 100 m. The rank decides the distance at which the
    /// tuft drops out as the bands thin with distance (`foliage_keep`): whole tufts go,
    /// lowest-ranked last.
    pub packed_height_tuft: u32,

    /// Height scale u8 (bits 0..8, the tuft's lerp into the type's `height_range`) | aux
    /// u8 (bits 8..16: the tuft's turn, in 256ths) | type id u8 (bits 16..24) | variant u8
    /// (bits 24..32).
    ///
    /// Scales are lerp factors, not metres, so retuning a type's sizes does not invalidate
    /// resident tiles. The 8-bit type id caps a scene at 256 foliage types.
    pub packed_scale_type: u32,

    /// The ground's colour under the tuft as RGB565 (bits 0..16; `0`: the ground gave
    /// none) | stable per-tuft seed u16 (bits 16..32).
    ///
    /// The seed is the low half of [`crate::blade_seed`] and keys every per-blade choice,
    /// the wind phase and the dithered fades, so it must come from
    /// `(tile_coord, lane, generation)` and **never** from frame index or time.
    pub packed_colour_seed: u32,
}

/// The most blades one tuft holds: its count is 4 bits.
pub const FOLIAGE_MAX_TUFT_BLADES: u32 = 15;

/// Unpacked, human-scale view of a [`GpuBladeInstance`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BladeParams {
    /// Tile-local position of the tuft in `0.0..=1.0` over the tile's axes, `[u, v]`.
    pub tile_uv: [f32; 2],
    /// Height offset in metres from the tile's `bounds_center`.
    pub height_offset: f32,
    /// This blade's index in its tuft (0 for a far tuft record).
    pub blade: u8,
    /// Blades in the tuft, `1..=FOLIAGE_MAX_TUFT_BLADES`.
    pub blades: u8,
    /// The tuft's rank, `0.0..=1.0` (unorm8): the order tufts drop out in with distance.
    pub rank: f32,
    /// Lerp factor in `0.0..=1.0` into the foliage type's `height_range` (the tuft's).
    pub height_scale: f32,
    /// The tuft's turn, in 256ths.
    pub aux: u8,
    /// Index into the foliage type descriptor array.
    pub type_id: u8,
    /// Variant selector consumed by the material.
    pub variant: u8,
    /// The ground's linear colour under the tuft (RGB565), or `None`.
    pub colour: Option<[f32; 3]>,
    /// Low 16 bits of [`crate::blade_seed`].
    pub seed: u16,
}

impl Default for BladeParams {
    fn default() -> Self {
        Self {
            tile_uv: [0.0, 0.0],
            height_offset: 0.0,
            blade: 0,
            blades: 1,
            rank: 0.0,
            height_scale: 1.0,
            aux: 0,
            type_id: 0,
            variant: 0,
            colour: None,
            seed: 0,
        }
    }
}

/// Pack tile-local UV into `packed_pos`. U occupies the low half.
#[inline]
pub fn pack_blade_pos(u: f32, v: f32) -> u32 {
    (pack_unorm16(u) as u32) | ((pack_unorm16(v) as u32) << 16)
}

/// Inverse of [`pack_blade_pos`].
#[inline]
pub fn unpack_blade_pos(packed: u32) -> [f32; 2] {
    [
        unpack_unorm16((packed & 0xffff) as u16),
        unpack_unorm16((packed >> 16) as u16),
    ]
}

/// Pack the f16 height offset, the blade's index and count in its tuft and the rank.
#[inline]
pub fn pack_blade_height_tuft(height_offset: f32, blade: u8, blades: u8, rank: f32) -> u32 {
    (f32_to_f16_bits(height_offset) as u32)
        | ((u32::from(blade) & 15) << 16)
        | ((u32::from(blades) & 15) << 20)
        | ((pack_unorm8(rank) as u32) << 24)
}

/// Inverse of [`pack_blade_height_tuft`]: `(height_offset, blade, blades, rank)`.
#[inline]
pub fn unpack_blade_height_tuft(packed: u32) -> (f32, u8, u8, f32) {
    (
        f16_bits_to_f32((packed & 0xffff) as u16),
        ((packed >> 16) & 15) as u8,
        ((packed >> 20) & 15) as u8,
        unpack_unorm8((packed >> 24) as u8),
    )
}

/// Pack the height scale, the aux byte and the two 8-bit identifiers, low byte first.
#[inline]
pub fn pack_blade_scale_type(height_scale: f32, aux: u8, type_id: u8, variant: u8) -> u32 {
    (pack_unorm8(height_scale) as u32)
        | ((aux as u32) << 8)
        | ((type_id as u32) << 16)
        | ((variant as u32) << 24)
}

/// Inverse of [`pack_blade_scale_type`]: `(height_scale, aux, type_id, variant)`.
#[inline]
pub fn unpack_blade_scale_type(packed: u32) -> (f32, u8, u8, u8) {
    (
        unpack_unorm8((packed & 0xff) as u8),
        ((packed >> 8) & 0xff) as u8,
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 24) & 0xff) as u8,
    )
}

/// A linear colour as RGB565, never 0 (0 is "no colour": a black ground packs as 0x0001).
#[inline]
pub fn pack_rgb565(colour: [f32; 3]) -> u16 {
    let q = |v: f32, max: f32| (v.clamp(0.0, 1.0) * max + 0.5) as u16;
    let packed = q(colour[0], 31.0) | (q(colour[1], 63.0) << 5) | (q(colour[2], 31.0) << 11);
    packed.max(1)
}

/// Inverse of [`pack_rgb565`] (`None` for 0).
#[inline]
pub fn unpack_rgb565(packed: u16) -> Option<[f32; 3]> {
    (packed != 0).then(|| {
        [
            f32::from(packed & 31) / 31.0,
            f32::from((packed >> 5) & 63) / 63.0,
            f32::from(packed >> 11) / 31.0,
        ]
    })
}

/// Pack the ground colour (low half) and the 16-bit seed (high half).
#[inline]
pub fn pack_blade_colour_seed(colour: Option<[f32; 3]>, seed: u16) -> u32 {
    u32::from(colour.map_or(0, pack_rgb565)) | ((seed as u32) << 16)
}

/// Inverse of [`pack_blade_colour_seed`]: `(colour, seed)`.
#[inline]
pub fn unpack_blade_colour_seed(packed: u32) -> (Option<[f32; 3]>, u16) {
    (
        unpack_rgb565((packed & 0xffff) as u16),
        (packed >> 16) as u16,
    )
}

/// Encode a blade for the GPU arena: the CPU mirror of the tail of the placement shader.
pub fn pack_blade(params: BladeParams) -> GpuBladeInstance {
    GpuBladeInstance {
        packed_pos: pack_blade_pos(params.tile_uv[0], params.tile_uv[1]),
        packed_height_tuft: pack_blade_height_tuft(
            params.height_offset,
            params.blade,
            params.blades,
            params.rank,
        ),
        packed_scale_type: pack_blade_scale_type(
            params.height_scale,
            params.aux,
            params.type_id,
            params.variant,
        ),
        packed_colour_seed: pack_blade_colour_seed(params.colour, params.seed),
    }
}

/// Decode a blade from the GPU arena.
///
/// `unpack_blade(pack_blade(p))` is not bit-identical to `p`: positions quantise to
/// unorm16, height to f16, rank and scale to unorm8 and the colour to RGB565. It *is*
/// idempotent from the second application onward.
pub fn unpack_blade(instance: &GpuBladeInstance) -> BladeParams {
    let tile_uv = unpack_blade_pos(instance.packed_pos);
    let (height_offset, blade, blades, rank) =
        unpack_blade_height_tuft(instance.packed_height_tuft);
    let (height_scale, aux, type_id, variant) = unpack_blade_scale_type(instance.packed_scale_type);
    let (colour, seed) = unpack_blade_colour_seed(instance.packed_colour_seed);
    BladeParams {
        tile_uv,
        height_offset,
        blade,
        blades,
        rank,
        height_scale,
        aux,
        type_id,
        variant,
        colour,
        seed,
    }
}

impl GpuBladeInstance {
    /// Index into the foliage type descriptor array.
    #[inline]
    pub fn type_id(&self) -> u8 {
        ((self.packed_scale_type >> 16) & 0xff) as u8
    }

    /// Material-defined variant selector.
    #[inline]
    pub fn variant(&self) -> u8 {
        ((self.packed_scale_type >> 24) & 0xff) as u8
    }

    /// Stable per-tuft hash seed. See [`GpuBladeInstance::packed_colour_seed`].
    #[inline]
    pub fn seed(&self) -> u16 {
        (self.packed_colour_seed >> 16) as u16
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tile header
// ═══════════════════════════════════════════════════════════════════════════════

/// Residency state of a foliage tile slot.
///
/// Stored as a plain `u32` inside [`GpuFoliageTile`] because the struct must be [`Pod`],
/// and a `Pod` type has to accept every bit pattern — an enum cannot. Read it back
/// through [`GpuFoliageTile::state`], which is honest about not recognising a value
/// instead of guessing.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileState {
    /// Slot owns no arena range and may be handed out by the allocator.
    #[default]
    Free = 0,
    /// The placement dispatch for this tile is in flight. The arena range is reserved
    /// but its contents are undefined, so the cull pass must skip it — drawing a
    /// `Placing` tile renders whatever the previous tenant left behind.
    Placing = 1,
    /// Blades are written and the tile is drawable.
    Resident = 2,
    /// Scheduled for reclaim. Still drawable this frame, which is what keeps a tile
    /// leaving the ring from popping out a frame before its scale-out finishes.
    Evicting = 3,
}

impl TileState {
    /// Recover a state from its GPU representation.
    ///
    /// Returns `None` for anything outside the known set rather than defaulting,
    /// because there is no safe default: guessing [`TileState::Free`] hands a live arena
    /// slab back to the allocator and produces two tiles writing the same range, while
    /// guessing [`TileState::Resident`] draws from a slab that was never written. An
    /// unknown value means the WGSL and Rust enums have drifted, and the only correct
    /// response is to treat the tile as unusable and say so.
    #[inline]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(TileState::Free),
            1 => Some(TileState::Placing),
            2 => Some(TileState::Resident),
            3 => Some(TileState::Evicting),
            _ => None,
        }
    }

    /// GPU representation of this state.
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Header for one slot in the resident tile ring. Exactly 32 bytes.
///
/// Each band ([`crate::GpuFoliageBand`]) keeps a ring of its tiles around the eye; this
/// is the record the residency manager, the placement dispatch and the tile cull share.
/// The bands' slots ([`crate::GpuFoliageBands::slots`]) are a few hundred, so the tile
/// cull reads the table linearly without an acceleration structure. `tile_coord` is in
/// the band's own tiles.
///
/// # Layout (32 bytes, 4-byte aligned)
/// ```text
///  0..8   tile_coord:      vec2<i32>   world tile grid coordinate (WGSL vec2 is safe here)
///  8..12  blade_offset:    u32
/// 12..16  blade_count:     u32
/// 16..20  bounds_center: f32
/// 20..24  bounds_half:   f32
/// 24..28  state:           u32         TileState
/// 28..32  generation:      u32
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuFoliageTile {
    /// World tile grid coordinate. Signed, and it must stay signed — an unsigned tile
    /// grid puts a hard wall at the world origin that only shows up when someone walks
    /// west of it and the entire ring wraps to the far side of the world.
    pub tile_coord: [i32; 2],

    /// First blade index owned by this tile in the shared arena.
    pub blade_offset: u32,

    /// Number of blades written by placement. Read as the instance count for this
    /// tile's clusters; a stale value here draws blades belonging to the previous tenant.
    pub blade_count: u32,

    /// Centre of the tile's vertical extent, taken from the terrain capture. XZ come
    /// from `tile_coord` and are not stored.
    pub bounds_center: f32,

    /// Half-extent of the tile's vertical span. The tile cull dilates this by the
    /// type's max height plus its WPO extent before the frustum and Hi-Z tests — see
    /// the plan's §6.2. Under-dilating here is the classic wind-culling bug: blades
    /// blow outside the tile bounds and the tile is culled while its geometry is still
    /// on screen.
    pub bounds_half: f32,

    /// [`TileState`] as a `u32`. Access through [`GpuFoliageTile::state`].
    pub state: u32,

    /// Bumped whenever the density map or terrain under this tile is edited.
    ///
    /// Residency is keyed on `(tile_coord, generation)`, so a bump invalidates the
    /// cached blades without needing an explicit flush, and it feeds
    /// [`crate::blade_seed`] so the re-placed blades are a *different* deterministic
    /// set rather than the same one. If this were left out of the seed, editing density
    /// would move blades in and out of existence but never reshuffle the survivors,
    /// which looks like the edit did nothing in the areas that stayed dense.
    pub generation: u32,
}

impl GpuFoliageTile {
    /// A slot owning no arena range, at the origin, in generation zero.
    pub const EMPTY: Self = Self {
        tile_coord: [0, 0],
        blade_offset: 0,
        blade_count: 0,
        bounds_center: 0.0,
        bounds_half: 0.0,
        state: TileState::Free as u32,
        generation: 0,
    };

    /// Decode [`GpuFoliageTile::state`]. `None` means the value is not one this build
    /// knows — see [`TileState::from_u32`] for why that is not defaulted away.
    #[inline]
    pub const fn state(&self) -> Option<TileState> {
        TileState::from_u32(self.state)
    }

    /// Whether the cull pass may emit draws for this tile's blades.
    ///
    /// `Placing` is excluded because its arena range holds undefined data, and an
    /// unrecognised state is excluded for the same reason.
    #[inline]
    pub const fn is_drawable(&self) -> bool {
        matches!(
            TileState::from_u32(self.state),
            Some(TileState::Resident) | Some(TileState::Evicting)
        )
    }
}

impl Default for GpuFoliageTile {
    fn default() -> Self {
        Self::EMPTY
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Foliage type descriptor
// ═══════════════════════════════════════════════════════════════════════════════

/// What geometry a foliage type resolves to.
///
/// Stored packed into [`GpuFoliageType::kind_and_flags`]; see [`pack_kind_and_flags`].
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FoliageKind {
    /// Procedural blade strip generated in the vertex shader. No vertex or index buffer
    /// is bound — see the plan's §6.3.
    #[default]
    Blade = 0,
    /// Single textured cutout card, camera- or yaw-oriented.
    Card = 1,
    /// A virtual-geometry mesh, culled and LOD-selected by the existing meshlet path.
    /// `mesh_or_impostor_id` is a `VirtualMeshId` for this kind.
    Mesh = 2,
    /// Voxel worlds' grass: a stack of cubes (edge `width_range[0]`, as many as the
    /// height rounds to, at most [`crate::FOLIAGE_STACK_CUBES`]) upright on one grid,
    /// no yaw. Placement stands each on a cube's centre (no two share one) and shapes
    /// tufts: stacks lean out of their tuft in whole-cube steps, tallest at its centre.
    /// Wind moves each cube whole. Drawn as boxes, the faces turned to the eye
    /// (`FoliageBoxesPass` expands them, the draw places nine corners a box).
    Block = 3,
}

impl FoliageKind {
    /// Recover a kind from its packed representation.
    ///
    /// `None` rather than a default for the same reason as [`TileState::from_u32`]:
    /// silently treating an unknown kind as `Blade` would send a tree through the grass
    /// pipeline and produce a field of 20-metre green strips with no obvious cause.
    #[inline]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(FoliageKind::Blade),
            1 => Some(FoliageKind::Card),
            2 => Some(FoliageKind::Mesh),
            3 => Some(FoliageKind::Block),
            _ => None,
        }
    }

    /// Packed representation of this kind.
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Mask selecting the [`FoliageKind`] discriminant out of `kind_and_flags`.
///
/// A full byte for what is currently a 2-bit value. The spare room is cheap here and
/// expensive later: flags start at bit 8 precisely so adding a fourth or fifth kind
/// never has to renumber a flag that a shipped shader already tests.
pub const FOLIAGE_KIND_MASK: u32 = 0x0000_00ff;

/// Render both faces. Grass blades and most leaf cards need this; anything with real
/// thickness should leave it off so backface culling can halve its raster cost.
pub const FOLIAGE_FLAG_TWO_SIDED: u32 = 1 << 8;

/// Include this type in shadow rendering, subject to the staging rules in the plan's
/// §10 (grass casts only in cascade 0, at L2 cards, with wind frozen).
pub const FOLIAGE_FLAG_CASTS_SHADOW: u32 = 1 << 9;

/// Sample the interaction field and bend. Types that ignore it — tree trunks, anything
/// rigid — skip a texture fetch per vertex by leaving this clear.
pub const FOLIAGE_FLAG_RECEIVES_INTERACTION: u32 = 1 << 10;

/// Take the ground's colour (its [`crate::GpuGroundHit::colour`] under the tuft) times
/// twice `base_color` (0.5 grey: the ground's own colour; brighter is lighter): grass the
/// colour of the turf it grows from, so past the last blades the ground already reads as
/// the meadow.
pub const FOLIAGE_FLAG_GROUND_COLOUR: u32 = 1 << 11;

/// Every flag bit currently defined. Bits outside this mask and outside
/// [`FOLIAGE_KIND_MASK`] are reserved and must be written as zero, so that a future
/// build can tell "this flag was not set" from "this build predates the flag".
pub const FOLIAGE_FLAG_MASK: u32 = FOLIAGE_FLAG_TWO_SIDED
    | FOLIAGE_FLAG_CASTS_SHADOW
    | FOLIAGE_FLAG_RECEIVES_INTERACTION
    | FOLIAGE_FLAG_GROUND_COLOUR;

/// Combine a [`FoliageKind`] and a set of `FOLIAGE_FLAG_*` bits into `kind_and_flags`.
///
/// Flag bits that collide with [`FOLIAGE_KIND_MASK`] are dropped rather than allowed to
/// corrupt the discriminant.
#[inline]
pub const fn pack_kind_and_flags(kind: FoliageKind, flags: u32) -> u32 {
    (kind as u32) | (flags & !FOLIAGE_KIND_MASK)
}

/// Foliage type descriptor shared by placement and rasterisation. Exactly 128 bytes.
///
/// One entry per *authored type* — not per instance. A scene has tens of foliage types,
/// so the whole table is a few kilobytes and stays permanently hot in L1 no matter how
/// often the placement and raster shaders read it. That is the fact that sets the size
/// policy for this struct: there is no measurable win from squeezing it, so every field
/// is stored in its natural form and nothing needs decoding on either side of the
/// CPU/GPU boundary.
///
/// # Why 128 (and earlier 96, not 64)
///
/// The plan's §4.3 heads this struct "64 bytes", but its own field list sums to **84** —
/// the header was simply wrong, and the fields are what matter. Rounding up to 96 left
/// 12 bytes of tail padding, which was **deliberate and meant to be spent**: §4.3 is a
/// first draft of what a foliage type needs, and this table will grow. The authored
/// material ([`FoliageMaterial`]) spent eight of them and the ground mask
/// ([`GpuFoliageType::ground_mask`]) the last four; the tufts (how blades gather, lean,
/// curve and fall, their tips' colour and the type's patches) grew it to 128.
///
/// The earlier draft of this type hit 64 bytes by storing `height_range`, `width_range`,
/// `slope_range` and `lod_distances` as `f16` pairs. That bought about two kilobytes —
/// nothing — and cost a decode on both sides, a standing risk that the WGSL
/// `unpack2x16float` path and the Rust packer disagree at some edge value, and a struct
/// with no room to grow. Packing is the wrong trade for a per-type table. It is the right
/// trade for [`GpuBladeInstance`], which is per-instance and multiplied by a million.
///
/// `altitude_range` is worth a specific note even now that everything is `f32`: it is the
/// one range here that can carry planetary magnitudes, and it must never be narrowed. At
/// 10 km, `f16` resolution is ±8 m — enough to move a treeline visibly. If a future
/// packing pass ever comes back to this struct, this is the field to leave alone.
///
/// The material words are the one packed exception, and a safe one. They are only
/// written into G-buffer targets that are 8-bit unorm themselves (albedo and ORM), so
/// an 8-bit colour and 16-bit roughness/metallic lose nothing the frame keeps, and the
/// decode is WGSL's exactly specified `unpack4x8unorm`/`unpack2x16unorm` (`x / 255`,
/// `x / 65535`), mirrored by [`FoliageMaterial::unpack`]. Stored unpacked they would
/// need 20 bytes, more than the headroom had.
///
/// # WGSL mirroring — every field is a scalar
///
/// **Declare all of these as scalars or `array<f32, N>` in WGSL. Not one of them may be
/// a vector type.** With this field order no member lands on the alignment a WGSL vector
/// requires: `height_range` at offset 4, `width_range` at 12, `slope_range` at 20 and
/// `altitude_range` at 28 are all misaligned for `vec2<f32>` (8), `lod_distances` at 36
/// is misaligned for `vec4<f32>` (16), and `wind_response` at 52 is misaligned for
/// `vec3<f32>` (16).
///
/// `wind_response` is the dangerous one, because it is the one someone will reach for
/// `vec3<f32>` on without thinking. WGSL gives `vec3<f32>` a 16-byte alignment, so that
/// single declaration would push the field from offset 52 to 64 and shift **every field
/// after it** — `interaction_stiffness`, `material_id`, `density_layer`,
/// `kind_and_flags` and `mesh_or_impostor_id` all read from the wrong offset. Nothing
/// crashes: trees render with a random material, foliage kinds resolve to the wrong
/// pipeline, and the cause is twelve bytes of padding rules in a language spec.
///
/// # Layout (128 bytes, 4-byte aligned)
/// ```text
///  0..4   density:               f32
///  4..12  height_range:          f32 × 2    scalars in WGSL (offset 4 is not vec2-aligned)
/// 12..20  width_range:           f32 × 2    scalars in WGSL (offset 12)
/// 20..28  slope_range:           f32 × 2    scalars in WGSL (offset 20), cos(slope) band
/// 28..36  altitude_range:        f32 × 2    scalars in WGSL (offset 28)
/// 36..52  lod_distances:         f32 × 4    scalars in WGSL (offset 36 is not vec4-aligned)
/// 52..64  wind_response:         f32 × 3    scalars in WGSL — NEVER vec3, see above
/// 64..68  interaction_stiffness: f32
/// 68..72  material_id:           u32
/// 72..76  density_layer:         u32
/// 76..80  kind_and_flags:        u32
/// 80..84  mesh_or_impostor_id:   u32
/// 84..88  base_color:            u32        RGBA8 unorm, 0 = no authored material
/// 88..92  roughness_metallic:    u32        unorm16 × 2, roughness in the low half
/// 92..96  ground_mask:           u32        ground ids it grows on (bit per id; 0: any)
///  96..100 blades_per_tuft:      u32        min (low 16 bits) and max blades a tuft
/// 100..108 lean_range:           f32 × 2    scalars in WGSL (offset 100): lean at the tuft's centre, rim
/// 108..112 curve:                f32        extra lean towards the tip
/// 112..116 fall_chance:          f32        share of blades lying over
/// 116..120 tip_color:            u32        RGBA8 unorm tip colour (flowers), 0 = none
/// 120..124 patch_scale:          f32        metres across the type's bare and dense patches, 0 = none
/// 124..128 patch_strength:       f32        0 uniform .. 1 fully bare between patches
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuFoliageType {
    /// Instances per square metre at full density weight, before the quality
    /// multiplier. The placement shader's candidate count derives from this, so it is
    /// also the knob that decides whether the arena overflows.
    pub density: f32,

    /// Min and max blade/plant height in metres. Also available as
    /// [`GpuFoliageType::height_range`].
    pub height_range: [f32; 2],

    /// Min and max width in metres. Also available as [`GpuFoliageType::width_range`].
    pub width_range: [f32; 2],

    /// Acceptance band on `cos(slope)`. Also available as
    /// [`GpuFoliageType::slope_range`].
    ///
    /// Cosine rather than the angle because that is what the terrain capture's normal
    /// gives directly — comparing cosines avoids an `acos` in the placement inner loop,
    /// which runs once per candidate rather than once per surviving blade.
    pub slope_range: [f32; 2],

    /// Min and max world altitude in metres. See the note on the struct: this is the one
    /// range that carries planetary magnitudes and must stay full `f32`.
    pub altitude_range: [f32; 2],

    /// LOD distances: `[0]` the near band's L0 → L1 boundary (full blades to coarser
    /// ones, switched a tuft at a time), `[1]` and `[2]` unused (the bands decide L2 and L3), `[3]`
    /// the type's reach: where it fades out (0: as far as the foliage goes).
    pub lod_distances: [f32; 4],

    /// Wind band gains: trunk, branch, leaf.
    ///
    /// **Three scalars in WGSL. Never `vec3<f32>`** — that declaration silently shifts
    /// every field after this one. See the struct's WGSL mirroring section.
    pub wind_response: [f32; 3],

    /// Interaction recovery constant. Larger is stiffer, i.e. it snaps back faster;
    /// the interaction field's decay uses `tau` derived from this.
    pub interaction_stiffness: f32,

    /// Index into the scene material table.
    pub material_id: u32,

    /// Slice in the density texture array that supplies this type's painted weight.
    pub density_layer: u32,

    /// [`FoliageKind`] in the low byte, `FOLIAGE_FLAG_*` bits above. Build with
    /// [`pack_kind_and_flags`], read with [`GpuFoliageType::kind`] and
    /// [`GpuFoliageType::flags`].
    pub kind_and_flags: u32,

    /// `VirtualMeshId` when the kind is [`FoliageKind::Mesh`], impostor atlas page
    /// otherwise. Overloading one slot costs nothing: no type is ever both.
    pub mesh_or_impostor_id: u32,

    /// Authored linear base colour, RGBA8 unorm with red in the low byte (WGSL
    /// `unpack4x8unorm`). An authored material always writes alpha 255, so **zero
    /// means "no authored material"** and the G-buffer pass shades the blade with its
    /// procedural colour. Write with [`GpuFoliageType::set_material`].
    pub base_color: u32,

    /// Authored roughness (low half) and metallic (high half) as unorm16 (WGSL
    /// `unpack2x16unorm`). Read only when [`GpuFoliageType::base_color`] is non-zero.
    pub roughness_metallic: u32,

    /// The ground ids this type grows on, a bit per id below 32 (the ground's
    /// [`crate::GpuGroundHit::ground`]: a terrain material, a biome); **0: any ground**.
    /// A heightfield's ground is id 0.
    pub ground_mask: u32,

    /// Blades a tuft gathers: the minimum in the low 16 bits, the maximum in the high
    /// ([`GpuFoliageType::set_blades_per_tuft`]). A placement candidate is a tuft; `1..1`
    /// is a lone blade.
    pub blades_per_tuft: u32,

    /// How far a tuft's blades lean out from its centre, as run over rise: at the centre
    /// blade, then at the rim. Smooth blades tilt by it; blocky ones step sideways a cube
    /// per `1 / lean` cubes up.
    pub lean_range: [f32; 2],

    /// Extra lean at the tip over the base (the blade bends more as it rises).
    pub curve: f32,

    /// Share of blades lying over (`0..=1`): low, leaning far out.
    pub fall_chance: f32,

    /// The top of each blade's colour, RGBA8 unorm as `base_color` (flower heads); `0`:
    /// the blade's own colour throughout.
    pub tip_color: u32,

    /// Metres across the type's bare and dense patches (value noise over the ground);
    /// `0`: no patches.
    pub patch_scale: f32,

    /// How far patches thin the type: `0` none, `1` bare between them.
    pub patch_strength: f32,
}

/// A foliage type's authored surface: linear base colour, roughness and metallic.
///
/// Stored in [`GpuFoliageType::base_color`] and
/// [`GpuFoliageType::roughness_metallic`]; see the note on packing there. Every
/// blade of the type takes this colour, varied by its per-blade tint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoliageMaterial {
    /// Linear RGB, `0.0..=1.0`.
    pub base_color: [f32; 3],
    /// `0.0` smooth to `1.0` rough.
    pub roughness: f32,
    /// `0.0` dielectric to `1.0` metal.
    pub metallic: f32,
}

impl FoliageMaterial {
    /// The `[base_color, roughness_metallic]` row words. Out-of-range and non-finite
    /// inputs clamp as [`pack_unorm8`] and [`pack_unorm16`] do. The alpha byte is
    /// always 255, so the colour word is never zero.
    pub fn pack(&self) -> [u32; 2] {
        let [r, g, b] = self.base_color.map(|c| u32::from(pack_unorm8(c)));
        [
            r | g << 8 | b << 16 | 0xff << 24,
            u32::from(pack_unorm16(self.roughness)) | u32::from(pack_unorm16(self.metallic)) << 16,
        ]
    }

    /// Decodes the row words exactly as `foliage_gbuffer.wgsl` does. `None` when the
    /// colour word is zero: the type has no authored material.
    pub fn unpack([base_color, roughness_metallic]: [u32; 2]) -> Option<Self> {
        (base_color != 0).then(|| Self {
            base_color: [0, 8, 16].map(|shift| unpack_unorm8((base_color >> shift) as u8)),
            roughness: unpack_unorm16(roughness_metallic as u16),
            metallic: unpack_unorm16((roughness_metallic >> 16) as u16),
        })
    }
}

impl GpuFoliageType {
    /// `[min, max]` height in metres.
    ///
    /// A plain field read. The accessor pair is kept — here and for the three ranges
    /// below — so the storage form stays an implementation detail: an earlier draft
    /// packed these as `f16` pairs, and if a future one ever has cause to again, it is a
    /// change to four functions rather than to every call site in the foliage stack.
    #[inline]
    pub fn height_range(&self) -> [f32; 2] {
        self.height_range
    }

    /// Set the height range.
    #[inline]
    pub fn set_height_range(&mut self, range: [f32; 2]) {
        self.height_range = range;
    }

    /// `[min, max]` width in metres.
    #[inline]
    pub fn width_range(&self) -> [f32; 2] {
        self.width_range
    }

    /// Set the width range.
    #[inline]
    pub fn set_width_range(&mut self, range: [f32; 2]) {
        self.width_range = range;
    }

    /// `[min, max]` acceptance band on `cos(slope)`.
    #[inline]
    pub fn slope_range(&self) -> [f32; 2] {
        self.slope_range
    }

    /// Set the slope acceptance band.
    #[inline]
    pub fn set_slope_range(&mut self, range: [f32; 2]) {
        self.slope_range = range;
    }

    /// LOD distances: the near band's L0 → L1 boundary, two unused, the type's reach.
    #[inline]
    pub fn lod_distances(&self) -> [f32; 4] {
        self.lod_distances
    }

    /// Set the LOD transition distances.
    #[inline]
    pub fn set_lod_distances(&mut self, distances: [f32; 4]) {
        self.lod_distances = distances;
    }

    /// Decoded [`FoliageKind`]. `None` means the low byte holds a discriminant this
    /// build does not know — see [`FoliageKind::from_u32`].
    #[inline]
    pub const fn kind(&self) -> Option<FoliageKind> {
        FoliageKind::from_u32(self.kind_and_flags & FOLIAGE_KIND_MASK)
    }

    /// The `FOLIAGE_FLAG_*` bits, with the kind discriminant masked off.
    #[inline]
    pub const fn flags(&self) -> u32 {
        self.kind_and_flags & !FOLIAGE_KIND_MASK
    }

    /// Test one `FOLIAGE_FLAG_*` bit.
    #[inline]
    pub const fn has_flag(&self, flag: u32) -> bool {
        (self.kind_and_flags & flag) != 0
    }

    /// Set the kind and the full flag set at once.
    #[inline]
    pub fn set_kind_and_flags(&mut self, kind: FoliageKind, flags: u32) {
        self.kind_and_flags = pack_kind_and_flags(kind, flags);
    }

    /// The authored material, or `None` when blades shade procedurally.
    #[inline]
    pub fn material(&self) -> Option<FoliageMaterial> {
        FoliageMaterial::unpack([self.base_color, self.roughness_metallic])
    }

    /// Set the authored material; `None` returns the type to procedural shading.
    #[inline]
    pub fn set_material(&mut self, material: Option<FoliageMaterial>) {
        [self.base_color, self.roughness_metallic] = material.map_or([0; 2], |m| m.pack());
    }

    /// `[min, max]` blades a tuft gathers.
    #[inline]
    pub fn blades_per_tuft(&self) -> [u32; 2] {
        let min = (self.blades_per_tuft & 0xffff).max(1);
        [min, (self.blades_per_tuft >> 16).max(min)]
    }

    /// Set the blades a tuft gathers, `min..=max` (each at least 1, at most
    /// [`crate::FOLIAGE_MAX_TUFT_BLADES`]).
    #[inline]
    pub fn set_blades_per_tuft(&mut self, [min, max]: [u32; 2]) {
        let min = min.clamp(1, crate::FOLIAGE_MAX_TUFT_BLADES);
        let max = max.clamp(min, crate::FOLIAGE_MAX_TUFT_BLADES);
        self.blades_per_tuft = min | (max << 16);
    }

    /// Set the tips' colour (linear RGB); `None`: blades keep their colour to the tip.
    #[inline]
    pub fn set_tip_color(&mut self, color: Option<[f32; 3]>) {
        self.tip_color = color.map_or(0, |c| {
            let [r, g, b] = c.map(|v| u32::from(pack_unorm8(v)));
            r | g << 8 | b << 16 | 0xff << 24
        });
    }
}

impl Default for GpuFoliageType {
    /// A grass-shaped default matching the plan's §11 example, so a type authored with
    /// `..Default::default()` renders something plausible rather than nothing.
    fn default() -> Self {
        Self {
            density: 40.0,
            height_range: [0.15, 0.45],
            width_range: [0.01, 0.03],
            // cos(35°)..cos(0°): accept anything flatter than a 35° slope. Written as
            // the computation rather than 0.8191 so the intent survives a retune.
            slope_range: [35f32.to_radians().cos(), 1.0],
            altitude_range: [f32::MIN, f32::MAX],
            lod_distances: crate::DEFAULT_LOD_DISTANCES,
            wind_response: [0.0, 0.3, 1.0],
            interaction_stiffness: 6.0,
            material_id: 0,
            density_layer: 0,
            kind_and_flags: pack_kind_and_flags(
                FoliageKind::Blade,
                FOLIAGE_FLAG_TWO_SIDED | FOLIAGE_FLAG_RECEIVES_INTERACTION,
            ),
            mesh_or_impostor_id: 0,
            base_color: 0,
            roughness_metallic: 0,
            ground_mask: 0,
            // A lone upright blade, no patches: what a type meant before tufts.
            blades_per_tuft: 1 | (1 << 16),
            lean_range: [0.0, 0.0],
            curve: 0.0,
            fall_chance: 0.0,
            tip_color: 0,
            patch_scale: 0.0,
            patch_strength: 0.0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Layer
// ═══════════════════════════════════════════════════════════════════════════════

/// GPU mirror of one authored foliage layer — the world-space box a layer's types grow in.
///
/// The placement shader accepts a candidate only if it lies inside at least one layer's
/// AABB, or inside a layer marked infinite. Empty tables are the legacy "carpet the whole
/// ring" behaviour, so a publisher that predates this table is not silently culled.
///
/// # Layout (32 bytes, 4-byte aligned)
///
/// Two `vec4<f32>`, chosen deliberately: WGSL gives `vec4` 16-byte alignment, so any
/// field added after these two would start on its own 16-byte cell and only widen the
/// struct without buying density. The `w` channels that would be padding anyway carry the
/// two spare scalars — `_pad` on the min, and the infinite-extent flag on the max.
/// ```text
///  0..16  bounds_min: vec4<f32>  xyz = world-space AABB minimum, w unused
/// 16..32  bounds_max: vec4<f32>  xyz = world-space AABB maximum, w = 1.0 ⇔ has_infinite_extent
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuFoliageLayer {
    /// World-space AABB minimum, `[min_x, min_y, min_z, _pad]`.
    pub bounds_min: [f32; 4],
    /// World-space AABB maximum; the `w` channel is `1.0` when the layer has infinite
    /// extent and every candidate counts as inside it, `0.0` otherwise.
    pub bounds_max: [f32; 4],
}

// ═══════════════════════════════════════════════════════════════════════════════
// Layout contract
// ═══════════════════════════════════════════════════════════════════════════════

const _: () = {
    assert!(
        std::mem::size_of::<GpuBladeInstance>() == 16,
        "GpuBladeInstance must be exactly 16 bytes"
    );
    assert!(
        std::mem::size_of::<GpuFoliageTile>() == 32,
        "GpuFoliageTile must be exactly 32 bytes"
    );
    assert!(
        std::mem::size_of::<GpuFoliageType>() == 128,
        "GpuFoliageType must be exactly 128 bytes"
    );
    assert!(
        std::mem::size_of::<GpuFoliageLayer>() == 32,
        "GpuFoliageLayer must be exactly 32 bytes"
    );
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_foliage_layouts_are_stable() {
        assert_eq!(std::mem::size_of::<GpuBladeInstance>(), 16);
        assert_eq!(std::mem::size_of::<GpuFoliageTile>(), 32);
        assert_eq!(std::mem::size_of::<GpuFoliageType>(), 128);
        assert_eq!(std::mem::size_of::<GpuFoliageLayer>(), 32);
        assert_eq!(std::mem::align_of::<GpuBladeInstance>(), 4);
        assert_eq!(std::mem::align_of::<GpuFoliageTile>(), 4);
        assert_eq!(std::mem::align_of::<GpuFoliageType>(), 4);
        assert_eq!(std::mem::align_of::<GpuFoliageLayer>(), 4);
        assert_eq!(std::mem::size_of::<TileState>(), 4);
        assert_eq!(std::mem::size_of::<FoliageKind>(), 4);
    }

    #[test]
    fn foliage_type_field_offsets_match_the_documented_layout() {
        // The WGSL mirror is written against the offset table in this struct's doc
        // comment. Pinning the offsets here means a reordered or resized field fails in
        // CI rather than in a shader that reads `material_id` out of `wind_response`.
        let value = GpuFoliageType::zeroed();
        let base = &value as *const _ as usize;
        let offset_of = |field: *const u8| field as usize - base;

        assert_eq!(offset_of(&value.density as *const f32 as *const u8), 0);
        assert_eq!(offset_of(value.height_range.as_ptr() as *const u8), 4);
        assert_eq!(offset_of(value.width_range.as_ptr() as *const u8), 12);
        assert_eq!(offset_of(value.slope_range.as_ptr() as *const u8), 20);
        assert_eq!(offset_of(value.altitude_range.as_ptr() as *const u8), 28);
        assert_eq!(offset_of(value.lod_distances.as_ptr() as *const u8), 36);
        assert_eq!(offset_of(value.wind_response.as_ptr() as *const u8), 52);
        assert_eq!(
            offset_of(&value.interaction_stiffness as *const f32 as *const u8),
            64
        );
        assert_eq!(offset_of(&value.material_id as *const u32 as *const u8), 68);
        assert_eq!(
            offset_of(&value.density_layer as *const u32 as *const u8),
            72
        );
        assert_eq!(
            offset_of(&value.kind_and_flags as *const u32 as *const u8),
            76
        );
        assert_eq!(
            offset_of(&value.mesh_or_impostor_id as *const u32 as *const u8),
            80
        );
        assert_eq!(offset_of(&value.base_color as *const u32 as *const u8), 84);
        assert_eq!(
            offset_of(&value.roughness_metallic as *const u32 as *const u8),
            88
        );
        assert_eq!(offset_of(&value.ground_mask as *const u32 as *const u8), 92);
        assert_eq!(
            offset_of(&value.blades_per_tuft as *const u32 as *const u8),
            96
        );
        assert_eq!(offset_of(value.lean_range.as_ptr() as *const u8), 100);
        assert_eq!(offset_of(&value.curve as *const f32 as *const u8), 108);
        assert_eq!(
            offset_of(&value.fall_chance as *const f32 as *const u8),
            112
        );
        assert_eq!(offset_of(&value.tip_color as *const u32 as *const u8), 116);
        assert_eq!(
            offset_of(&value.patch_scale as *const f32 as *const u8),
            120
        );
        assert_eq!(
            offset_of(&value.patch_strength as *const f32 as *const u8),
            124
        );
        assert_eq!(std::mem::size_of::<GpuFoliageType>(), 128);
    }

    #[test]
    fn foliage_layer_field_offsets_match_the_documented_layout() {
        let value = GpuFoliageLayer::zeroed();
        let base = &value as *const _ as usize;
        let offset_of = |field: *const u8| field as usize - base;

        assert_eq!(offset_of(value.bounds_min.as_ptr() as *const u8), 0);
        assert_eq!(offset_of(value.bounds_max.as_ptr() as *const u8), 16);
        assert_eq!(std::mem::size_of::<GpuFoliageLayer>(), 32);
    }

    #[test]
    fn no_foliage_type_field_is_wgsl_vector_aligned() {
        // Every field in this struct must be declared as a scalar in WGSL. This test
        // proves the claim rather than trusting the comment: if a future reorder happens
        // to land `wind_response` on a 16-byte boundary, someone will reach for
        // `vec3<f32>`, it will work, and the next reorder after that will break it
        // silently. Better to keep "all scalars, always" unconditionally true.
        let value = GpuFoliageType::zeroed();
        let base = &value as *const _ as usize;
        let offset_of = |field: *const u8| field as usize - base;

        for offset in [
            offset_of(value.height_range.as_ptr() as *const u8),
            offset_of(value.width_range.as_ptr() as *const u8),
            offset_of(value.slope_range.as_ptr() as *const u8),
            offset_of(value.altitude_range.as_ptr() as *const u8),
            offset_of(value.lean_range.as_ptr() as *const u8),
        ] {
            assert_ne!(
                offset % 8,
                0,
                "offset {offset} is vec2<f32>-aligned in WGSL"
            );
        }
        for offset in [
            offset_of(value.lod_distances.as_ptr() as *const u8),
            offset_of(value.wind_response.as_ptr() as *const u8),
        ] {
            assert_ne!(
                offset % 16,
                0,
                "offset {offset} is vec3/vec4-aligned in WGSL"
            );
        }
    }

    #[test]
    fn foliage_material_round_trips_and_zero_means_procedural() {
        let mut ty = GpuFoliageType::default();
        assert_eq!(ty.material(), None, "the default type shades procedurally");
        assert_eq!(GpuFoliageType::zeroed().material(), None);

        let material = FoliageMaterial {
            base_color: [1.0, 0.0, 0.2],
            roughness: 0.85,
            metallic: 0.0,
        };
        ty.set_material(Some(material));
        // Red in the low byte, alpha forced to 255: what `unpack4x8unorm` reads.
        assert_eq!(ty.base_color, 0xff33_00ff);
        assert_eq!(
            ty.roughness_metallic & 0xffff,
            u32::from(pack_unorm16(0.85))
        );
        assert_eq!(ty.roughness_metallic >> 16, 0);
        let decoded = ty.material().unwrap();
        for (got, want) in decoded.base_color.iter().zip(material.base_color) {
            assert!((got - want).abs() <= 0.5 / 255.0);
        }
        assert!((decoded.roughness - 0.85).abs() <= 0.5 / 65535.0);
        assert_eq!(decoded.metallic, 0.0);

        // Black is a real colour, not "unset".
        ty.set_material(Some(FoliageMaterial {
            base_color: [0.0; 3],
            roughness: 0.0,
            metallic: 1.0,
        }));
        assert_eq!(ty.material().unwrap().base_color, [0.0; 3]);
        assert_eq!(ty.material().unwrap().metallic, 1.0);

        ty.set_material(None);
        assert_eq!((ty.base_color, ty.roughness_metallic), (0, 0));
    }

    #[test]
    fn gpu_types_are_zeroable_and_castable() {
        // Zeroed is the state every one of these buffers starts in; if `Zeroable` ever
        // stopped holding, the first frame would read uninitialised memory.
        let tile = GpuFoliageTile::zeroed();
        assert_eq!(tile.state(), Some(TileState::Free));
        assert!(!tile.is_drawable());

        let blades = [GpuBladeInstance::zeroed(); 4];
        assert_eq!(bytemuck::cast_slice::<_, u8>(&blades).len(), 64);
    }

    #[test]
    fn blade_pos_packs_x_into_the_low_half() {
        let packed = pack_blade_pos(0.0, 1.0);
        assert_eq!(packed & 0xffff, 0);
        assert_eq!(packed >> 16, 0xffff);
        assert_eq!(unpack_blade_pos(packed), [0.0, 1.0]);
    }

    #[test]
    fn blade_field_packers_do_not_bleed_into_each_other() {
        // Saturate one field at a time and assert the others stay clean. This is the
        // test that catches a shifted mask.
        assert_eq!(pack_blade_scale_type(0.0, 0, 0xff, 0x00), 0x00ff_0000);
        assert_eq!(pack_blade_scale_type(0.0, 0, 0x00, 0xff), 0xff00_0000);
        assert_eq!(pack_blade_scale_type(1.0, 0, 0x00, 0x00), 0x0000_00ff);
        assert_eq!(pack_blade_scale_type(0.0, 0xff, 0x00, 0x00), 0x0000_ff00);
        assert_eq!(pack_blade_height_tuft(0.0, 15, 0, 0.0), 0x000f_0000);
        assert_eq!(pack_blade_height_tuft(0.0, 0, 15, 0.0), 0x00f0_0000);
        assert_eq!(pack_blade_height_tuft(0.0, 0, 0, 1.0), 0xff00_0000);
        assert_eq!(pack_blade_colour_seed(None, 0xffff), 0xffff_0000);
        assert_eq!(pack_blade_colour_seed(Some([1.0; 3]), 0), 0x0000_ffff);
        // A black ground is still a colour.
        assert_eq!(pack_blade_colour_seed(Some([0.0; 3]), 0), 1);
    }

    #[test]
    fn blade_round_trips_within_quantisation_tolerance() {
        let cases = [
            BladeParams::default(),
            BladeParams {
                tile_uv: [1.0, 1.0],
                height_offset: -12.5,
                blade: 14,
                blades: 15,
                rank: 1.0,
                height_scale: 1.0,
                aux: 255,
                type_id: 255,
                variant: 255,
                colour: Some([1.0, 0.5, 0.0]),
                seed: u16::MAX,
            },
            BladeParams {
                tile_uv: [0.5, 0.25],
                height_offset: 3.75,
                blade: 3,
                blades: 9,
                rank: 0.3,
                height_scale: 0.5,
                aux: 77,
                type_id: 7,
                variant: 3,
                colour: Some([0.2, 0.4, 0.1]),
                seed: 0xbeef,
            },
        ];

        for original in cases {
            let decoded = unpack_blade(&pack_blade(original));
            assert!(
                (decoded.tile_uv[0] - original.tile_uv[0]).abs() <= 1.0 / 65535.0,
                "{decoded:?} vs {original:?}"
            );
            assert!((decoded.tile_uv[1] - original.tile_uv[1]).abs() <= 1.0 / 65535.0);
            assert!(
                (decoded.height_offset - original.height_offset).abs()
                    <= original.height_offset.abs() * 1.0e-3 + 1.0e-6
            );
            assert!((decoded.height_scale - original.height_scale).abs() <= 1.0 / 255.0);
            assert!((decoded.rank - original.rank).abs() <= 1.0 / 255.0);
            match (decoded.colour, original.colour) {
                (Some(d), Some(o)) => {
                    assert!(d.iter().zip(o).all(|(d, o)| (d - o).abs() <= 1.0 / 31.0))
                }
                (d, o) => assert_eq!(d.is_some(), o.is_some()),
            }
            // Identifiers and the seed are exact.
            assert_eq!(
                (decoded.blade, decoded.blades, decoded.aux),
                (original.blade, original.blades, original.aux)
            );
            assert_eq!(decoded.type_id, original.type_id);
            assert_eq!(decoded.variant, original.variant);
            assert_eq!(decoded.seed, original.seed);
        }
    }

    #[test]
    fn blade_repack_is_idempotent_after_the_first_pass() {
        // An evict/re-place cycle must not drift a blade.
        for i in 0..512u32 {
            let t = i as f32 / 511.0;
            let original = BladeParams {
                tile_uv: [t, 1.0 - t],
                height_offset: t * 20.0 - 10.0,
                blade: (i % 15) as u8,
                blades: (i % 15 + 1) as u8,
                rank: t,
                height_scale: t,
                aux: (i * 3) as u8,
                type_id: (i & 0xff) as u8,
                variant: ((i >> 3) & 0xff) as u8,
                colour: (i % 3 != 0).then_some([t, 1.0 - t, t * 0.5]),
                seed: (i * 7919) as u16,
            };
            let first = pack_blade(original);
            let second = pack_blade(unpack_blade(&first));
            assert_eq!(first, second);
        }
    }

    #[test]
    fn blade_accessors_agree_with_the_unpacked_view() {
        let params = BladeParams {
            type_id: 42,
            variant: 2,
            seed: 0x1234,
            ..BladeParams::default()
        };
        let instance = pack_blade(params);
        assert_eq!(instance.type_id(), 42);
        assert_eq!(instance.variant(), 2);
        assert_eq!(instance.seed(), 0x1234);
        let decoded = unpack_blade(&instance);
        assert_eq!(decoded.type_id, instance.type_id());
        assert_eq!(decoded.variant, instance.variant());
        assert_eq!(decoded.seed, instance.seed());
    }

    #[test]
    fn tile_state_round_trips_and_refuses_to_guess() {
        for state in [
            TileState::Free,
            TileState::Placing,
            TileState::Resident,
            TileState::Evicting,
        ] {
            assert_eq!(TileState::from_u32(state.as_u32()), Some(state));
        }
        assert_eq!(TileState::from_u32(4), None);
        assert_eq!(TileState::from_u32(u32::MAX), None);

        let mut tile = GpuFoliageTile::EMPTY;
        assert!(!tile.is_drawable());
        tile.state = TileState::Placing.as_u32();
        assert!(
            !tile.is_drawable(),
            "a tile mid-placement holds undefined arena data and must not draw"
        );
        tile.state = TileState::Resident.as_u32();
        assert!(tile.is_drawable());
        tile.state = TileState::Evicting.as_u32();
        assert!(
            tile.is_drawable(),
            "evicting tiles still draw for one frame"
        );
        tile.state = 99;
        assert_eq!(tile.state(), None);
        assert!(
            !tile.is_drawable(),
            "an unknown state must not be assumed drawable"
        );
    }

    #[test]
    fn foliage_kind_and_flags_share_the_word_without_collision() {
        let packed = pack_kind_and_flags(
            FoliageKind::Mesh,
            FOLIAGE_FLAG_TWO_SIDED | FOLIAGE_FLAG_CASTS_SHADOW,
        );
        assert_eq!(packed & FOLIAGE_KIND_MASK, FoliageKind::Mesh.as_u32());
        assert_eq!(
            packed & !FOLIAGE_KIND_MASK,
            FOLIAGE_FLAG_TWO_SIDED | FOLIAGE_FLAG_CASTS_SHADOW
        );

        // Every flag must live outside the kind byte, or setting one would rewrite the
        // discriminant.
        assert_eq!(FOLIAGE_FLAG_MASK & FOLIAGE_KIND_MASK, 0);

        // A flag argument that trespasses on the kind byte is dropped, not merged.
        let defended = pack_kind_and_flags(FoliageKind::Card, 0xff | FOLIAGE_FLAG_TWO_SIDED);
        assert_eq!(
            FoliageKind::from_u32(defended & FOLIAGE_KIND_MASK),
            Some(FoliageKind::Card)
        );

        for kind in [
            FoliageKind::Blade,
            FoliageKind::Card,
            FoliageKind::Mesh,
            FoliageKind::Block,
        ] {
            assert_eq!(FoliageKind::from_u32(kind.as_u32()), Some(kind));
        }
        assert_eq!(FoliageKind::from_u32(4), None);
        assert_eq!(FoliageKind::from_u32(u32::MAX), None);
    }

    #[test]
    fn foliage_type_accessors_are_lossless() {
        // Nothing in this struct quantises any more, so these are exact equalities
        // rather than tolerances — which is the whole point of the 96-byte layout.
        let mut ty = GpuFoliageType::zeroed();
        ty.set_height_range([0.15, 0.45]);
        ty.set_width_range([0.01, 0.03]);
        ty.set_slope_range([-1.0, 1.0]);
        ty.set_lod_distances([8.0, 20.0, 45.0, 120.0]);
        ty.set_kind_and_flags(FoliageKind::Card, FOLIAGE_FLAG_CASTS_SHADOW);

        assert_eq!(ty.height_range(), [0.15, 0.45]);
        assert_eq!(ty.width_range(), [0.01, 0.03]);
        assert_eq!(ty.slope_range(), [-1.0, 1.0]);
        assert_eq!(ty.lod_distances(), [8.0, 20.0, 45.0, 120.0]);
        assert_eq!(ty.kind(), Some(FoliageKind::Card));
        assert!(ty.has_flag(FOLIAGE_FLAG_CASTS_SHADOW));
        assert!(!ty.has_flag(FOLIAGE_FLAG_TWO_SIDED));
        assert_eq!(ty.flags(), FOLIAGE_FLAG_CASTS_SHADOW);

        // The accessors and the public fields are two views of the same storage.
        assert_eq!(ty.height_range(), ty.height_range);
        assert_eq!(ty.width_range(), ty.width_range);
        assert_eq!(ty.slope_range(), ty.slope_range);
        assert_eq!(ty.lod_distances(), ty.lod_distances);

        // Planetary altitudes survive intact — the reason altitude_range was never a
        // candidate for narrowing.
        ty.altitude_range = [-11_000.0, 9_000.5];
        assert_eq!(ty.altitude_range, [-11_000.0, 9_000.5]);
    }

    #[test]
    fn default_foliage_type_is_a_usable_grass() {
        let ty = GpuFoliageType::default();
        assert_eq!(ty.kind(), Some(FoliageKind::Blade));
        assert!(ty.has_flag(FOLIAGE_FLAG_TWO_SIDED));
        assert!(ty.has_flag(FOLIAGE_FLAG_RECEIVES_INTERACTION));
        assert_eq!(ty.lod_distances(), crate::DEFAULT_LOD_DISTANCES);
        let height = ty.height_range();
        assert!(height[0] > 0.0 && height[1] > height[0]);
        let slope = ty.slope_range();
        assert!(slope[0] < slope[1] && slope[1] <= 1.0);
        assert!(ty.density > 0.0);
    }
}
