//! The lights for standalone Helio hosts (the plugins' examples, tests and
//! demos): what `helio-components` is for the kinds still in Helio.
//! Engines register their own components (Pulsar's `LightComponent`) and
//! install the lights' join (`helio_pass_light_cull::scene_rows`).
//!
//! - [`features`]: the lights' Helio features, for a host's preset
//!   (`helio_presets::Preset::with_features`);
//! - [`LightComponent`], [`BillboardComponent`], [`ReflectionCaptureComponent`]:
//!   the rows the lights' passes read, written by the host directly (a
//!   standalone host's light rows are the light list itself);
//!   [`register_columns`] registers the light list up front.

pub use helio_light_api::host_rows::LightComponent;
pub use helio_pass_billboard::host_rows::BillboardComponent;
pub use helio_pass_deferred_light::host_rows::ReflectionCaptureComponent;

/// The lights' Helio features: the shadow views, the light binning, the
/// lighting models (deferred, light-field) and the billboards.
pub fn features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_shadow_matrix::ShadowViewsFeature)
        .with(helio_pass_light_cull::LightCullFeature)
        .with(helio_pass_deferred_light::DeferredLightingFeature)
        .with(helio_pass_hlfs::HlfsFeature)
        .with(helio_pass_billboard::BillboardFeature)
}

/// Registers the light list's column with a store before its mirror is
/// attached: registering up front, not on first insert, so its first rows
/// are never lost. (Billboard and reflection-capture columns are a host's
/// choice: a registered capture column makes the lighting walk its rows.)
pub fn register_columns(
    store: &mut pulsar_scenedb::gpu::SceneGpuStore,
    device: &std::sync::Arc<wgpu::Device>,
) {
    LightComponent::register_gpu_columns_growable(store, helio_light_api::MAX_LIGHTS, device);
}
