//! Decals' scene rows: the decal components' rows, packed into
//! [`crate::MAX_DECALS`] rows, each with a non-zero box whose transform maps
//! world space into the owner-placed box (a row join, see
//! `helio_scene_join::RowJoin`).
use helio_scene_join::{placement::*, BufferKey, RowJoin, RowJoinKeys, RowTable};

/// `DecalSourceRow`: local size vec4, then the `DecalRow` row (32 words)
/// with its transform left to the join.
pub const DECAL_SOURCE_ROW_BYTES: u64 = 36 * 4;

pub const DECALS_KEY: BufferKey = BufferKey::of("decals");

/// Decals' row join, from the decal rows at `decals`.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, decals: BufferKey) -> RowJoin {
    RowJoin::new(
        device,
        "DecalRows",
        keys,
        vec![RowTable::new(
            "Decal Rows",
            DECALS_KEY,
            decals,
            DECAL_SOURCE_ROW_BYTES,
            32,
            GATE_HIDDEN | OWNER_BOX,
        )
        .packed(crate::MAX_DECALS)],
    )
}
