//! Materials (`pulsar.materials`): the plugin's registrations.
//!
//! The materials' Helio feature (`helio-pass-materials`: the material
//! texture table every kind's drawers bind) joins every pipeline the engine
//! runs, and its sampler comes from the project's texture streaming
//! settings ([`sampler`]).
//!
//! The material assets themselves (`helio_component::material_assets`, the
//! material graph and cooked textures) stay with the meshes' components in
//! `helio_component`: core crates (the importer, the cooker, the class and
//! prefab records) read them by type, as batch 4 found for the meshes.

// The plugin's descriptor (`../pulsar-plugin.toml`): linking this crate
// installs the plugin; a project enables it or not (`pulsar_plugins`).
pulsar_plugins::installed!("../pulsar-plugin.toml");

pub mod sampler;

/// The materials' features: the texture table.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_materials::MaterialsFeature)
}

/// The material sampler from the project's `streaming.texture_*` settings:
/// scene materials read through trilinear, anisotropic, repeating filtering,
/// not wgpu's nearest/clamp default.
fn configure_renderer(config: helio::RendererConfig) -> helio::RendererConfig {
    config.with_feature(helio_mats::MaterialSampler(
        sampler::SamplerSettings::from_project().descriptor(),
    ))
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        origin: pulsar_plugins::origin!(),
        features: render_features,
        configure: configure_renderer,
    }
}

#[cfg(test)]
mod tests {
    use engine_backend::subsystems::render::helio_renderer::render_features;

    #[test]
    fn the_engine_runs_the_material_texture_table() {
        let features = render_features::plugin_features();
        assert!(
            features.ids().contains(&"pulsar.materials"),
            "the materials' feature is registered"
        );
    }
}
