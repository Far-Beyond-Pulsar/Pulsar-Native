//! A body's interaction with simulated water (Pulsar-Native#1080).
//!
//! The physics world is SceneDB: a body is its `PhysicsComponent` and its
//! owner object, and nothing is synced to or from a separate physics world.
//! A body on the `WaterSim` collision channel pushes the water it moves
//! through. SceneDB derives a [`WaterHitboxSourceRow`] from the component
//! on every insert, write, removal and replay, keyed by the component
//! instance; the renderer's environment join (Helio's
//! `helio-default-graphs::environment_join`) bounds the body by its owner's
//! meshes at the owner's transform, on the GPU, into the water
//! simulation's hitbox rows, and the simulation keeps each body's previous
//! bounds itself, so a body displaces water only as it moves.

use pulsar_scenedb::gpu::GpuMirrorHandle;
use pulsar_scenedb_derive::SceneStore;

use super::{CollisionChannel, PhysicsComponent};

/// The buffer [`WaterHitboxSourceRow`]s live in.
pub const WATER_HITBOX_SOURCES_BUFFER: &str = "water_hitbox_sources";

/// How far, in metres, a body's push fades out past its bounds.
pub const WATER_HITBOX_EDGE_SOFTNESS: f32 = 0.25;
/// How strongly a body pushes the water it moves through.
pub const WATER_HITBOX_STRENGTH: f32 = 1.0;

/// A body's water interaction: `[edge softness (m), strength, interacts,
/// 0]`, the layout `helio_default_graphs::environment_join` reads
/// (`WATER_HITBOX_SOURCE_ROW_BYTES`). Zero for a body that does not
/// interact, which the join skips.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "water_hitbox_sources")]
pub struct WaterHitboxSourceRow {
    #[gpu]
    pub params: [f32; 4],
}

impl WaterHitboxSourceRow {
    pub fn of(physics: &PhysicsComponent) -> Self {
        if !physics.interacts_with_water() {
            return bytemuck::Zeroable::zeroed();
        }
        Self {
            params: [WATER_HITBOX_EDGE_SOFTNESS, WATER_HITBOX_STRENGTH, 1.0, 0.0],
        }
    }
}

impl PhysicsComponent {
    /// Whether this body disturbs simulated water: enabled, colliding, and
    /// on the `WaterSim` channel.
    pub fn interacts_with_water(&self) -> bool {
        self.general.enabled
            && self.general.collision_enabled
            && self.collision.collision_channel & u64::from(CollisionChannel::WaterSim) != 0
    }
}

fn water_hitbox_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
    // SAFETY: SceneDB reaches this only through `PhysicsComponent`'s own
    // `ComponentId`, with a pointer to a live value.
    let physics = unsafe { &*(data as *const PhysicsComponent) };
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &WaterHitboxSourceRow::of(physics),
        is_new_insert,
    );
}

fn water_hitbox_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<WaterHitboxSourceRow>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<PhysicsComponent>,
        dispatch: water_hitbox_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<PhysicsComponent>,
        clear: water_hitbox_clear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bodies_on_the_water_sim_channel_interact() {
        let mut physics = PhysicsComponent::default();
        assert_eq!(WaterHitboxSourceRow::of(&physics).params, [0.0; 4]);
        physics.collision.collision_channel |= u64::from(CollisionChannel::WaterSim);
        assert_eq!(
            WaterHitboxSourceRow::of(&physics).params,
            [WATER_HITBOX_EDGE_SOFTNESS, WATER_HITBOX_STRENGTH, 1.0, 0.0]
        );
        physics.general.collision_enabled = false;
        assert_eq!(WaterHitboxSourceRow::of(&physics).params, [0.0; 4]);
        physics.general.collision_enabled = true;
        physics.general.enabled = false;
        assert_eq!(WaterHitboxSourceRow::of(&physics).params, [0.0; 4]);
        assert_eq!(std::mem::size_of::<WaterHitboxSourceRow>(), 16);
    }

    /// Saved channels are their index; WaterSim follows Custom (8), so
    /// channels saved before it keep their meaning.
    #[test]
    fn the_water_sim_channel_is_saved_after_the_existing_ones() {
        assert_eq!(CollisionChannel::Custom as u64, 8);
        assert_eq!(CollisionChannel::WaterSim as u64, 9);
        assert_eq!(u64::from(CollisionChannel::WaterSim), 1 << 8);
    }
}
