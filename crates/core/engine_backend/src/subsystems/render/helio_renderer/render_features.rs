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

/// How much wider than its frame a camera's view must render, as whatever
/// owns the camera's optics says (a fisheye or barrel-distorted lens maps
/// past the frame and crops back). Submitted by that plugin.
pub struct ViewOverscanRegistration {
    /// The overscan of view `view_id` (1: none) in `world`, for a frame of
    /// `aspect` (width over height).
    pub overscan: fn(&pulsar_scenedb::World, u32, f32) -> f32,
}

inventory::collect!(ViewOverscanRegistration);

/// The largest overscan any registration asks of view `view_id` (1: none).
pub fn view_overscan(world: &pulsar_scenedb::World, view_id: u32, aspect: f32) -> f32 {
    inventory::iter::<ViewOverscanRegistration>
        .into_iter()
        .map(|registration| (registration.overscan)(world, view_id, aspect))
        .fold(1.0, f32::max)
}

/// A set of world-space lines an editor viewport draws for a plugin's kind
/// (a spline's curve), through the renderer's editor debug lines.
pub trait EditorLines: Send {
    /// The lines to draw now, when they changed since the last call
    /// (`None`: keep what is drawn).
    fn poll(&mut self, world: &pulsar_scenedb::World) -> Option<Vec<helio::WorldVertex>>;
}

/// A plugin's editor line set: `name` is its debug-line group, `new` makes
/// one watching `world`. Submitted by the plugin's component crate.
pub struct EditorLinesRegistration {
    pub name: &'static str,
    pub new: fn(&pulsar_scenedb::World) -> Box<dyn EditorLines>,
}

inventory::collect!(EditorLinesRegistration);

/// Every registered line set, watching `world`.
pub fn editor_lines(world: &pulsar_scenedb::World) -> Vec<(&'static str, Box<dyn EditorLines>)> {
    inventory::iter::<EditorLinesRegistration>
        .into_iter()
        .map(|registration| (registration.name, (registration.new)(world)))
        .collect()
}
