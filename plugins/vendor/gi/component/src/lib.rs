//! GI (`pulsar.gi`): the component crate of the plugin that owns world-space
//! diffuse global illumination.
//!
//! GI has no SceneDB component: its probe clipmap follows the camera, and
//! the scene's tracers feed it (the meshes' ray queries, from the feature;
//! a voxel terrain through the GI api's `TerrainTracer`). What a project
//! sets is the feature's settings ([`helio_pass_gi::GI`], a settings group
//! of its own).
//!
//! The crate registers the plugin's Helio feature (`helio_pass_gi::GiFeature`:
//! the probes' update, the meshes' rays and the resolve that publishes the
//! frame's indirect diffuse) and its settings with the engine. Everything
//! registers through `inventory` once the crate is linked: hosts link it
//! without naming it (a game built for a project links the runtime modules
//! of the plugins its `Pulsar.toml` enables, the editor every installed
//! plugin through `installed_plugins`; `pulsar_plugins`) and know no GI type.

// The plugin's descriptor (`../pulsar-plugin.toml`): linking this crate
// installs the plugin; a project enables it or not (`pulsar_plugins`).
pulsar_plugins::installed!("../pulsar-plugin.toml");

/// GI's feature: the probe clipmap, its tracers and its resolve.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_gi::GiFeature)
}

/// The feature's settings from the project's GI settings.
fn configure_renderer(config: helio::RendererConfig) -> helio::RendererConfig {
    use engine_backend::subsystems::render::helio_renderer::project_settings;
    config.with_feature(helio_pass_gi::GiSettings::from_settings(
        &project_settings::resolved(&helio_pass_gi::GI),
    ))
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        origin: pulsar_plugins::origin!(),
        features: render_features,
        configure: configure_renderer,
    }
}

inventory::submit! {
    pulsar_settings::project::rendering::SettingsGroupRegistration {
        origin: pulsar_plugins::origin!(),
        group: &helio_pass_gi::GI,
    }
}

#[cfg(test)]
mod tests {
    use engine_backend::subsystems::render::helio_renderer::render_features;

    #[test]
    fn registers_its_feature() {
        let features = render_features::plugin_features();
        assert!(
            features.ids().contains(&"helio.gi"),
            "features: {:?}",
            features.ids()
        );
    }

    #[test]
    fn configures_its_settings() {
        let config = render_features::configure(helio::RendererConfig::new(
            8,
            8,
            wgpu::TextureFormat::Rgba8Unorm,
        ));
        assert!(config.features.get::<helio_pass_gi::GiSettings>().is_some());
    }
}
