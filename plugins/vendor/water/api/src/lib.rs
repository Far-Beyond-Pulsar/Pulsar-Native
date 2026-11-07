//! The water feature's public contract.
//!
//! - Water bakes its shoreline from a terrain's elevations
//!   (`helio_pass_water_sim::WaterShoreMap::from_terrain`) and names no
//!   terrain: a terrain feature answers [`WaterTerrainProvider`].
//! - Bodies push simulated water through hitboxes: whatever owns the bodies
//!   (physics) derives one source row per body under
//!   [`WATER_HITBOX_SOURCES_BUFFER`], laid out as
//!   [`WATER_HITBOX_SOURCE_ROW_BYTES`] describes; water's row join bounds
//!   each by its owner's meshes.

/// The buffer bodies' water hitbox source rows live in.
pub const WATER_HITBOX_SOURCES_BUFFER: &str = "water_hitbox_sources";
/// A hitbox source row: `[edge softness (m), strength, interacts, 0]` as
/// `f32`s. A row whose interaction word is zero is skipped (a body that does
/// not push water).
pub const WATER_HITBOX_SOURCE_ROW_BYTES: u64 = 4 * 4;
/// A hitbox source row's interaction word.
pub const WATER_HITBOX_GATE_WORD: u32 = 2;

/// Supplies actual terrain elevations in absolute world coordinates. `None`
/// means the requested location has no terrain data; baking then fails rather
/// than silently treating missing data as ocean floor.
pub trait WaterTerrainProvider: Sync {
    fn elevation_y(&self, world_x: f64, world_z: f64) -> Option<f64>;
}
