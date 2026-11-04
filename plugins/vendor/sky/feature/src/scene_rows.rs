//! The sky's scene rows: the atmosphere components' rows (a row join, see
//! `helio_scene_join::RowJoin`); a planet placed at its owner is centred on
//! the owner's position.
use helio_scene_join::{placement::*, BufferKey, RowJoin, RowJoinKeys, RowTable};

/// Words of the atmosphere row ([`crate::AtmosphereRow`]).
const ATMOSPHERE_WORDS: u32 = (std::mem::size_of::<crate::AtmosphereRow>() / 4) as u32;
/// `AtmosphereSourceRow`: the [`crate::AtmosphereRow`] pass row, its centre in
/// the owner's space.
pub const ATMOSPHERE_SOURCE_ROW_BYTES: u64 = ATMOSPHERE_WORDS as u64 * 4;

pub const ATMOSPHERES_KEY: BufferKey = BufferKey::of("atmospheres");

/// The sky's row join, from the atmosphere rows at `atmospheres`.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, atmospheres: BufferKey) -> RowJoin {
    RowJoin::new(
        device,
        "SkyRows",
        keys,
        vec![RowTable::new(
            "Sky Rows Atmospheres",
            ATMOSPHERES_KEY,
            atmospheres,
            ATMOSPHERE_SOURCE_ROW_BYTES,
            ATMOSPHERE_WORDS,
            CENTRED,
        )],
    )
}
