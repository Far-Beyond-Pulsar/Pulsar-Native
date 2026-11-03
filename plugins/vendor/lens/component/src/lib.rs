//! Camera lenses (`pulsar.lens`): the component crate of the plugin that
//! owns a camera's optics.
//!
//! - The component ([`component`]): `CameraLensComponent`, Helio's
//!   physically based lens as authored (optics, focus, tilt, an ordered
//!   stack of optical elements, presets), its rows and its script methods.
//! - The registrations of the plugin's Helio features (the lens,
//!   `helio_pass_lens::LensFeature`, and the flare it drives,
//!   `helio_pass_flare::LensFlareFeature`), its scene rows (the lens source
//!   rows the plugin's join publishes as `camera_lenses`, and the element
//!   rows) and how much wider than its frame a lensed view renders
//!   ([`camera_lens_view_overscan`]), with the engine.
//!
//! Everything registers through `inventory` once the crate is linked: hosts
//! link it (the game: `extern crate lens_component`; the editor through the
//! plugin's editor crate, `lens_plugin`) and know no lens type.

pub mod component;

pub use component::*;

fn register_columns(
    store: &mut pulsar_scenedb::gpu::SceneGpuStore,
    device: &std::sync::Arc<wgpu::Device>,
) {
    register_lens_gpu_columns(store, device);
}

fn derivation(
    device: &wgpu::Device,
    keys: helio_scene_join::RowJoinKeys,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    Box::new(helio_pass_lens::scene_rows::scene_rows(
        device,
        keys,
        helio_scene_join::BufferKey::of(CAMERA_LENS_SOURCES_BUFFER),
    ))
}

inventory::submit! {
    engine_backend::scene::helio_bridge::SceneRowsRegistration {
        register_columns,
        derivation,
    }
}

inventory::submit! {
    engine_backend::subsystems::render::ViewOverscanRegistration {
        overscan: camera_lens_view_overscan,
    }
}

/// The lens and its flare: the lens resolves the scene colour through the
/// camera's optics after anti-aliasing, the flare adds the lights' ghosts.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_lens::LensFeature)
        .with(helio_pass_flare::LensFlareFeature)
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
        assert_eq!(
            std::mem::size_of::<CameraLensSourceRow>() as u64,
            helio_pass_lens::scene_rows::CAMERA_LENS_SOURCE_ROW_BYTES
        );
    }

    #[test]
    fn registers_its_features() {
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        for id in ["helio.lens", "helio.lens_flare"] {
            assert!(
                features.ids().contains(&id),
                "{id}: features {:?}",
                features.ids()
            );
        }
    }
}
