//! Water's scene rows (a row join, see `helio_scene_join::RowJoin`):
//!
//! - Water volumes, packed into [`MAX_WATER_VOLUMES`] rows: bounds from the
//!   owner-oriented box, the surface height following the owner's Y, the
//!   sun the lights' directional light (`helio_light_api::
//!   DIRECTIONAL_LIGHT`, which this join names; `w` 0 without one), the
//!   level's global wind (the foliage api's wind rows) unless the volume opts
//!   out (`wind_params.w`), and the frame's phase in the water's world tiling.
//! - Water hitboxes, packed into [`MAX_WATER_HITBOXES`] rows: bodies that push
//!   simulated water, bounded by the world AABB of the bounding spheres of
//!   the meshes their owner holds; `params` is the source row's edge softness
//!   and strength with the body's identity (its source row + 1) in `z`.
use helio_scene_join::{placement::*, BufferKey, RowJoin, RowJoinKeys, RowTable};

use helio_foliage_api::{GLOBAL_WIND_MARK_WORD, WIND_SOURCE_ROW_BYTES};

/// `WaterVolumeSourceRow`: local size (`w`: surface height above the
/// owner), then the water volume row after its bounds (56 words).
pub const WATER_VOLUME_SOURCE_ROW_BYTES: u64 = 60 * 4;
/// Rows the water passes read: placed water volumes beyond these are not
/// drawn.
pub const MAX_WATER_VOLUMES: u32 = crate::MAX_SIM_VOLUMES;
/// A water row's wind strength per m/s of the global wind's speed
/// (`row_join.wgsl`'s `WATER_WIND_PER_SPEED`): a 2 m/s breeze makes gentle
/// ripples (1), 10 m/s a choppy surface (5).
pub const WATER_WIND_STRENGTH_PER_SPEED: f32 = 0.5;
pub use helio_water_api::{WATER_HITBOX_GATE_WORD, WATER_HITBOX_SOURCE_ROW_BYTES};

pub const WATER_VOLUMES_KEY: BufferKey = BufferKey::of("water_volumes");
pub const WATER_HITBOXES_KEY: BufferKey = BufferKey::of("water_hitboxes");

/// Where water's source rows live.
#[derive(Clone, Copy, Debug)]
pub struct WaterRowSources {
    pub water_volumes: BufferKey,
    /// The level's wind rows (the foliage api's `WIND_SOURCE_ROW_BYTES`).
    pub wind: BufferKey,
    pub water_hitboxes: BufferKey,
}

/// Water's row join. Its sun is the lights' directional light, whatever
/// the host's keys say.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, sources: WaterRowSources) -> RowJoin {
    let keys = RowJoinKeys {
        directional_light: Some(BufferKey::of(helio_light_api::DIRECTIONAL_LIGHT)),
        ..keys
    };
    RowJoin::new(
        device,
        "WaterRows",
        keys,
        vec![
            RowTable::new(
                "Water Rows Volumes",
                WATER_VOLUMES_KEY,
                sources.water_volumes,
                WATER_VOLUME_SOURCE_ROW_BYTES,
                64,
                OWNER_AABB | GATE_HIDDEN | SURFACE_HEIGHT | SUN,
            )
            .packed(MAX_WATER_VOLUMES)
            .phase_period(crate::WATER_WORLD_PERIOD)
            .over(
                RowTable::new(
                    "Water Rows Global Wind",
                    WATER_VOLUMES_KEY,
                    sources.wind,
                    WIND_SOURCE_ROW_BYTES,
                    64,
                    WIND,
                )
                .gated_on(GLOBAL_WIND_MARK_WORD),
            ),
            RowTable::new(
                "Water Rows Hitboxes",
                WATER_HITBOXES_KEY,
                sources.water_hitboxes,
                WATER_HITBOX_SOURCE_ROW_BYTES,
                20,
                MESH_BOUNDS,
            )
            .packed(crate::MAX_WATER_HITBOXES)
            .gated_on(WATER_HITBOX_GATE_WORD),
        ],
    )
}
