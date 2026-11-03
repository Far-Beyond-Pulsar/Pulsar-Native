//! Particles (`pulsar.particles`): the component crate of the plugin that
//! owns GPU particle emitters.
//!
//! - The component ([`component`]): `ParticleEmitterComponent`, GPU
//!   particles spawned at its owner (the Corona pass).
//! - Its row ([`CoronaEmitterSourceRow`]): the Corona pass's emitter row
//!   SceneDB derives from the authored value; the plugin's join places it at
//!   the owner and gives it its range of the shared particle pool
//!   (`helio_pass_corona::scene_rows`).
//! - The registrations of the plugin's Helio feature
//!   (`helio_pass_corona::CoronaFeature`) and its scene rows with the engine.
//!
//! Everything registers through `inventory` once the crate is linked: hosts
//! link it (the game: `extern crate particles_component`; the editor through
//! the plugin's editor crate, `particles_plugin`) and know no particle type.

pub mod component;

pub use component::*;

use pulsar_scenedb::gpu::{GpuMirrorHandle, SceneGpuStore};
use pulsar_scenedb_derive::SceneStore;

/// Where the emitters' source rows live.
pub const CORONA_EMITTER_SOURCES_BUFFER: &str = "corona_emitter_sources";

/// A particle emitter: the Corona pass row
/// (`helio_pass_corona::GpuCoronaEmitter`, bit for bit) with a zero
/// transform, which the join replaces with the owner's, and the requested
/// pool range in `particle_count` (word 46), which the join replaces with
/// the allocated range (`particle_offset`, word 45). A disabled emitter's
/// row is zero, which the join skips.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "corona_emitter_sources")]
pub struct CoronaEmitterSourceRow {
    /// The pass row's transform: zero here, the join's to write.
    #[gpu]
    pub transform: [f32; 16],
    /// Emission (rate, lifetime and its variation, gravity), start and end
    /// size, start and end colour.
    #[gpu]
    pub params: [f32; 16],
    /// Velocity, its variation, shape (type, radius, -, active).
    #[gpu]
    pub motion: [f32; 12],
    /// Sprite, particle offset (the join's), requested particles, spawn
    /// cursor (unused: the pass keeps its own).
    #[gpu]
    pub range: [u32; 4],
    #[gpu]
    pub reserved: [f32; 12],
}

impl CoronaEmitterSourceRow {
    pub fn of(emitter: &ParticleEmitterComponent) -> Self {
        bytemuck::pod_read_unaligned(bytemuck::bytes_of(&emitter.to_row()))
    }
}

fn emitter_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
    // SAFETY: SceneDB reaches this only through `ParticleEmitterComponent`'s
    // own `ComponentId`, with a pointer to a live value.
    let emitter = unsafe { &*(data as *const ParticleEmitterComponent) };
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &CoronaEmitterSourceRow::of(emitter),
        is_new_insert,
    );
}

fn emitter_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<CoronaEmitterSourceRow>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<ParticleEmitterComponent>,
        dispatch: emitter_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<ParticleEmitterComponent>,
        clear: emitter_clear,
    }
}

fn register_columns(store: &mut SceneGpuStore, device: &std::sync::Arc<wgpu::Device>) {
    CoronaEmitterSourceRow::register_gpu_columns_growable(store, 16, device);
}

fn derivation(
    device: &wgpu::Device,
    keys: helio_scene_join::RowJoinKeys,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    Box::new(helio_pass_corona::scene_rows::scene_rows(
        device,
        keys,
        helio_scene_join::BufferKey::of(CORONA_EMITTER_SOURCES_BUFFER),
    ))
}

inventory::submit! {
    engine_backend::scene::helio_bridge::SceneRowsRegistration {
        register_columns,
        derivation,
    }
}

/// Particles move on their own: a realtime viewport keeps drawing while an
/// enabled emitter is in the world.
fn particles_live(world: &pulsar_scenedb::World) -> bool {
    world
        .query::<&ParticleEmitterComponent>()
        .any(|(instance, emitter)| {
            emitter.enabled && pulsar_scene_model::attachments::is_enabled(world, instance)
        })
}

fn watch_particles(world: &pulsar_scenedb::World) -> pulsar_scenedb::ChangeCursor {
    world.open_change_cursor::<ParticleEmitterComponent>()
}

inventory::submit! {
    engine_backend::subsystems::render::AnimatedContentRegistration {
        watch: watch_particles,
        animated: particles_live,
    }
}

/// The particles' feature: Corona's simulation and its overlay drawer.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_corona::CoronaFeature)
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        features: render_features,
        configure: |config| config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_row_is_the_pass_row() {
        assert_eq!(std::mem::size_of::<CoronaEmitterSourceRow>(), 60 * 4);
        assert_eq!(
            std::mem::size_of::<CoronaEmitterSourceRow>() as u64,
            helio_pass_corona::scene_rows::CORONA_EMITTER_SOURCE_ROW_BYTES
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_corona::GpuCoronaEmitter>(),
            60 * 4
        );
    }

    #[test]
    fn a_disabled_emitter_derives_an_inert_row() {
        let emitter = ParticleEmitterComponent {
            max_particles: 300,
            ..Default::default()
        };
        // The join's gate word (the requested range) and the range itself.
        assert_eq!(CoronaEmitterSourceRow::of(&emitter).range[2], 300);
        let row = CoronaEmitterSourceRow::of(&ParticleEmitterComponent {
            enabled: false,
            ..emitter
        });
        assert!(bytemuck::bytes_of(&row).iter().all(|byte| *byte == 0));
    }

    #[test]
    fn registers_its_feature() {
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        assert!(
            features.ids().contains(&"helio.corona"),
            "features: {:?}",
            features.ids()
        );
    }
}
