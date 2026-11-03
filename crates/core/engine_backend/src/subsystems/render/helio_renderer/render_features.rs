//! What plugins add to the renderer: the Helio features of the object kinds
//! they own (kinds neither Helio's presets nor this crate know) and those
//! features' settings. Every pipeline the engine runs builds over Helio's
//! default features and these: the default preset, the registered presets
//! and render-graph assets (`pipelines`).

use helio_feature_api::FeatureSet;

/// A plugin's features and how they read the project's settings.
pub struct RenderFeatureRegistration {
    /// The plugin's features.
    pub features: fn() -> FeatureSet,
    /// Adds the plugin's feature settings to a renderer config, from the
    /// project's settings ([`super::project_settings::resolved`] of the
    /// groups the plugin registers in `pulsar_settings`).
    pub configure: fn(helio::RendererConfig) -> helio::RendererConfig,
}

inventory::collect!(RenderFeatureRegistration);

/// Every registered plugin's features.
pub fn plugin_features() -> FeatureSet {
    inventory::iter::<RenderFeatureRegistration>
        .into_iter()
        .fold(FeatureSet::new(), |set, registration| {
            set.extended(&(registration.features)())
        })
}

/// `config` with every registered plugin's feature settings.
pub fn configure(config: helio::RendererConfig) -> helio::RendererConfig {
    inventory::iter::<RenderFeatureRegistration>
        .into_iter()
        .fold(config, |config, registration| {
            (registration.configure)(config)
        })
}
