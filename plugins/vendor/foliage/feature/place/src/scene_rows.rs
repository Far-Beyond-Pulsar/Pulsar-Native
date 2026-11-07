//! Foliage's scene rows (a row join, see `helio_scene_join::RowJoin`): one
//! foliage source row feeds three tables, each taking the rows whose type
//! has a density (word 0): the types, packed; the layers, packed (a
//! world-aligned square centred on the owner); and the one wind row the
//! foliage passes read: the first foliage component's own wind, under the
//! level's global wind, under the first foliage component that opts out of
//! the global wind (its wind's `_pad[0]`).
use helio_scene_join::{placement::*, BufferKey, Placement, RowJoin, RowJoinKeys, RowTable};

use helio_foliage_api::{GLOBAL_WIND_MARK_WORD, WIND_SOURCE_ROW_BYTES};

/// `FoliageSourceRow`: the foliage type row (32 words; its density first),
/// the layer (half extent, infinite flag, altitude min and max) and the
/// wind row (12 words).
pub const FOLIAGE_SOURCE_ROW_BYTES: u64 = 48 * 4;
/// Words of a foliage type row ([`crate::GpuFoliageType`]).
const FOLIAGE_TYPE_WORDS: u32 = (std::mem::size_of::<crate::GpuFoliageType>() / 4) as u32;
/// A foliage source row's wind `_pad[0]` (36 + 10): 1 when the component
/// opts out of the global wind.
const FOLIAGE_OWN_WIND_WORD: u32 = FOLIAGE_TYPE_WORDS + 4 + 10;
/// Foliage types the join publishes. Placement draws each candidate's type
/// from the live leading rows, so the table's capacity costs only a short
/// per-workgroup scan.
pub const MAX_FOLIAGE_TYPES: u32 = 64;

/// A layer: a world-aligned square centred on its owner (`rows_layers.wgsl`).
const LAYERS_PLACEMENT: Placement = Placement::new(
    "foliage_layers",
    include_str!("../shaders/rows_layers.wgsl"),
)
.from_owner();

pub const FOLIAGE_TYPES_KEY: BufferKey = BufferKey::of("foliage_types");
pub const FOLIAGE_LAYERS_KEY: BufferKey = BufferKey::of("foliage_layers");
pub const FOLIAGE_WIND_KEY: BufferKey = BufferKey::of("foliage_wind");

/// Where foliage's source rows live.
#[derive(Clone, Copy, Debug)]
pub struct FoliageRowSources {
    pub foliage: BufferKey,
    /// The level's wind rows (`helio_foliage_api::WIND_SOURCE_ROW_BYTES`).
    pub wind: BufferKey,
}

/// Foliage's row join.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, sources: FoliageRowSources) -> RowJoin {
    RowJoin::new(
        device,
        "FoliageRows",
        keys,
        vec![
            RowTable::new(
                "Foliage Rows Types",
                FOLIAGE_TYPES_KEY,
                sources.foliage,
                FOLIAGE_SOURCE_ROW_BYTES,
                FOLIAGE_TYPE_WORDS,
                GATE_HIDDEN,
            )
            .packed(MAX_FOLIAGE_TYPES)
            .slice(0, FOLIAGE_TYPE_WORDS)
            .gated_on(0),
            RowTable::new(
                "Foliage Rows Layers",
                FOLIAGE_LAYERS_KEY,
                sources.foliage,
                FOLIAGE_SOURCE_ROW_BYTES,
                8,
                GATE_HIDDEN,
            )
            .placed_by(LAYERS_PLACEMENT)
            .packed(crate::MAX_FOLIAGE_LAYERS)
            .slice(FOLIAGE_TYPE_WORDS, 0)
            .gated_on(0),
            RowTable::new(
                "Foliage Rows Wind",
                FOLIAGE_WIND_KEY,
                sources.foliage,
                FOLIAGE_SOURCE_ROW_BYTES,
                12,
                GATE_HIDDEN,
            )
            .packed(1)
            .slice(FOLIAGE_TYPE_WORDS + 4, 12)
            .gated_on(0)
            .over(
                RowTable::new(
                    "Foliage Rows Global Wind",
                    FOLIAGE_WIND_KEY,
                    sources.wind,
                    WIND_SOURCE_ROW_BYTES,
                    12,
                    0,
                )
                .gated_on(GLOBAL_WIND_MARK_WORD),
            )
            .over(
                RowTable::new(
                    "Foliage Rows Own Wind",
                    FOLIAGE_WIND_KEY,
                    sources.foliage,
                    FOLIAGE_SOURCE_ROW_BYTES,
                    12,
                    GATE_HIDDEN,
                )
                .slice(FOLIAGE_TYPE_WORDS + 4, 12)
                .gated_on(FOLIAGE_OWN_WIND_WORD),
            ),
        ],
    )
}
