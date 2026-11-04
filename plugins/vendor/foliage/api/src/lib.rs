//! Foliage GPU types, packing helpers and CPU mirrors of the foliage shader math.
//!
//! The foliage feature's public contract, the `*-core` crate of the foliage stack: shared POD types live in a crate
//! with no `wgpu` dependency, so the passes that produce foliage data and the passes
//! that consume it agree on layout without depending on each other, and so the layout
//! and the maths can be tested on a machine with no GPU at all.
//!
//! The no-`wgpu` rule is load-bearing, not stylistic. Everything here is on the critical
//! path of a CI test that must run in a headless container: the layout asserts, the
//! packing round-trips, and — most importantly — the placement determinism reference. If
//! this crate ever grows a graphics dependency, those tests become GPU tests, GPU tests
//! become optional, and the determinism contract stops being enforced.
//!
//! # What lives here
//!
//! - [`gpu_types`] — [`GpuBladeInstance`], [`GpuFoliageTile`] and [`GpuFoliageType`],
//!   with compile-time size asserts. These are mirrored field-for-field in WGSL.
//! - [`packing`] — the bit-level primitives the packed fields are built from, each with
//!   a documented WGSL equivalent.
//! - [`placement`] — CPU reference implementations of the placement and LOD maths. The
//!   WGSL is a transcription of these, and these are what the determinism and
//!   cull-equality tests compare against.
//! - [`quality`] — the [`FoliageQuality`] presets, the only platform difference in the
//!   foliage stack.
//! - [`bands`] — the bands of resident tiles that carry foliage out to its range.
//! - [`ground`] — what foliage grows on.
//! - [`components`] — the SceneDB rows foliage reads and their buffer contracts.
//!
//! # What does not live here
//!
//! Anything holding a GPU resource, and anything with per-frame state. Buffers, bind
//! groups and residency bookkeeping belong to `helio-pass-foliage-place`. The SceneDB rows
//! ([`components`]) and the shared WGSL ([`COMMON_SNIPPET`], [`WIND_SNIPPET`]) live here
//! too: what a feature that grows foliage on its ground (a voxel terrain) or draws it
//! reads, without depending on the foliage passes.

pub mod bands;
pub mod gpu_types;
pub mod ground;
pub mod packing;
pub mod placement;
pub mod quality;

pub use bands::*;
pub use gpu_types::*;
pub use ground::*;
pub use packing::*;
pub use placement::*;
pub use quality::*;

pub mod components;
#[cfg(feature = "host-rows")]
pub mod host_rows;
pub use components::{
    FoliageInteractorRow, FoliageLayerRow, FoliageTypeRow, FoliageWindRow, FOLIAGE_INTERACTORS,
    FOLIAGE_LAYERS, FOLIAGE_TYPES, FOLIAGE_WIND,
};

/// The buffer the level's wind source rows live in
/// ([`WIND_SOURCE_ROW_BYTES`]): read by foliage and by water.
pub const WIND_SOURCES_BUFFER: &str = "wind_sources";

/// Marker opting a shader into [`WIND`]. Must appear in the source.
pub const WIND_MARKER: &str = "//!use helio_foliage_wind";

/// The three-band foliage wind model and the `Wind` uniform layout, as a
/// `helio-core` shader snippet (see `helio_core::shader::ShaderSnippet`).
pub const WIND: &str = include_str!("shaders/foliage_wind.wgsl");

/// [`helio_core::shader::ShaderSnippet`] for [`WIND`]. Passed explicitly to
/// `helio_core::shader::resolve_with`/`module_with` by any shader opting in
/// via [`WIND_MARKER`] — `helio-core` itself never names this snippet.
pub const WIND_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!(WIND_MARKER, "shaders/foliage_wind.wgsl");

/// Marker opting a shader into [`COMMON`]. Must appear in the source.
pub const COMMON_MARKER: &str = "//!use helio_foliage_common";

/// The records placement, the cull and the draw share (type, tile, frame, blade, bands)
/// and the bands' functions, as a `helio-core` shader snippet. A shader including it
/// declares `foliage_bands: FoliageBands` (a uniform).
pub const COMMON: &str = include_str!("shaders/foliage_common.wgsl");

/// [`helio_core::shader::ShaderSnippet`] for [`COMMON`].
pub const COMMON_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!(COMMON_MARKER, "shaders/foliage_common.wgsl");

/// Cubes in the tallest blocky blade ([`FoliageKind::Block`]): a stack of the type's
/// cube (`width_range[0]`) as tall as its height rounds to, at most this many.
pub const FOLIAGE_STACK_CUBES: u32 = 24;

/// Hard cap on stratified candidates evaluated for one tile.
///
/// Bounds the placement shader's inner loop so a mis-authored density cannot turn one
/// workgroup into a multi-millisecond stall. 16 384 candidates over an 8 m tile is 256
/// per m², six times the reference density, so hitting this ceiling means the density
/// authoring is wrong, not that the ceiling is low.
pub const MAX_CANDIDATES_PER_TILE: u32 = 16_384;

/// Representational ceiling on blades in one tile's arena slab.
///
/// Not a budget. A `visible_blades[]` entry packs the owning tile slot in its high 16 bits
/// and the blade's tile-local index in the low 16, so a slab past 65 536 would alias
/// blades onto each other with no error anywhere. Raising this means widening that
/// encoding on both sides of the producer/consumer contract.
pub const MAX_BLADES_PER_TILE: u32 = 65_536;

/// Edge length of one base foliage tile in metres.
///
/// 8 m is the unit of everything downstream: the near band's tiles, the ground's chart
/// (`GroundGeometry::tile_frame` spans whole base tiles), the probe blocks and the
/// placement dispatch (one workgroup per tile). The far bands' tiles span `2^k` of them
/// ([`crate::plan_bands`]).
///
/// Changing this invalidates every cached tile and shifts the tile grid, which re-rolls
/// every tuft in the world via [`blade_seed`].
pub const FOLIAGE_TILE_SIZE_METERS: f32 = 8.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_placing_a_tile_reproduces_the_same_records() {
        // The determinism contract, exercised through the packing layer rather than just
        // the hash: two independent "dispatches" over the same tile must produce
        // byte-identical arena contents.
        fn place(tile_coord: [i32; 2], generation: u32) -> Vec<GpuBladeInstance> {
            (0..128u32)
                .map(|lane| {
                    let seed = blade_seed(tile_coord, lane, generation);
                    pack_blade(BladeParams {
                        tile_uv: [hash_to_unit(seed), hash_to_unit(seed.rotate_left(11))],
                        height_offset: hash_to_unit(seed.rotate_left(19)) * 0.4,
                        rank: hash_to_unit(seed.rotate_left(23)),
                        height_scale: hash_to_unit(seed.rotate_left(5)),
                        type_id: 2,
                        variant: (seed >> 30) as u8,
                        seed: seed as u16,
                        ..BladeParams::default()
                    })
                })
                .collect()
        }

        let first = place([12, -7], 3);
        let second = place([12, -7], 3);
        assert_eq!(
            first, second,
            "the same tile and generation must produce a byte-identical record list"
        );

        // A generation bump must reshuffle, not merely perturb.
        let bumped = place([12, -7], 4);
        let unchanged = first
            .iter()
            .zip(bumped.iter())
            .filter(|(a, b)| a.packed_pos == b.packed_pos)
            .count();
        assert!(
            unchanged <= 2,
            "{unchanged} of 128 tufts kept their position across a generation bump"
        );

        // And a neighbouring tile must be an independent draw, not an offset copy.
        let neighbour = place([13, -7], 3);
        let shared = first
            .iter()
            .zip(neighbour.iter())
            .filter(|(a, b)| a.packed_pos == b.packed_pos)
            .count();
        assert!(
            shared <= 2,
            "{shared} of 128 tufts are shared with the neighbouring tile"
        );
    }
}

/// The level's wind rows (`GlobalWindSourceRow`): the foliage wind row (12
/// words), marked global. Foliage and water read them in their scene rows.
pub const WIND_SOURCE_ROW_BYTES: u64 = 12 * 4;
/// The wind row's `_pad[0]`: 1 in a global wind's row.
pub const GLOBAL_WIND_MARK_WORD: u32 = 10;
