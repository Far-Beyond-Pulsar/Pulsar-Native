//! Fog's scene rows: the global fog and local fog volume components' rows
//! placed into the media rows the volumetric fog reads (a row join, see
//! `helio_scene_join::RowJoin`). A local volume's bounds are its
//! owner-oriented box's world AABB.
use helio_scene_join::{placement::*, BufferKey, RowJoin, RowJoinKeys, RowTable};

/// `GlobalFogSourceRow`: the `GlobalFogRow` pass row (20 words).
pub const GLOBAL_FOG_SOURCE_ROW_BYTES: u64 = 20 * 4;
/// `LocalFogSourceRow`: local size vec4, medium (20), edge fade vec4.
pub const LOCAL_FOG_SOURCE_ROW_BYTES: u64 = 28 * 4;

pub const GLOBAL_FOG_KEY: BufferKey = BufferKey::of("global_fog_media");
pub const LOCAL_FOG_KEY: BufferKey = BufferKey::of("local_fog_media");

/// Where the fog components' source rows live.
#[derive(Clone, Copy, Debug)]
pub struct FogRowSources {
    pub global_fog: BufferKey,
    pub local_fog: BufferKey,
}

/// Fog's row join.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, sources: FogRowSources) -> RowJoin {
    RowJoin::new(
        device,
        "FogRows",
        keys,
        vec![
            RowTable::new(
                "Fog Rows Global",
                GLOBAL_FOG_KEY,
                sources.global_fog,
                GLOBAL_FOG_SOURCE_ROW_BYTES,
                20,
                GATE_HIDDEN,
            ),
            RowTable::new(
                "Fog Rows Local",
                LOCAL_FOG_KEY,
                sources.local_fog,
                LOCAL_FOG_SOURCE_ROW_BYTES,
                32,
                OWNER_AABB | GATE_HIDDEN,
            ),
        ],
    )
}
