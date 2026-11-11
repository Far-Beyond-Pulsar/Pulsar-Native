//! What plugins add to the renderer: the Helio features of the object kinds
//! they own (kinds neither Helio's presets nor this crate know) and those
//! features' settings. Every pipeline the engine runs builds over Helio's
//! default features and these: the default preset, the registered presets
//! and render-graph assets (`pipelines`).

use helio_feature_api::FeatureSet;

/// A plugin's features and how they read the project's settings.
pub struct RenderFeatureRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// The plugin's features.
    pub features: fn() -> FeatureSet,
    /// Adds the plugin's feature settings to a renderer config, from the
    /// project's settings ([`super::project_settings::resolved`] of the
    /// groups the plugin registers in `pulsar_settings`).
    pub configure: fn(helio::RendererConfig) -> helio::RendererConfig,
}

/// The features' ids and, under each, the node kinds it registers (each
/// feature registers into a registry of its own here, so a node shows under
/// the feature that added it: `NodeRegistry::feature_nodes`).
impl pulsar_plugins::Contribution for RenderFeatureRegistration {
    const CATEGORY: &'static str = "render.features";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        (self.features)().ids().join("+")
    }

    fn label(&self) -> String {
        "Render features".into()
    }

    fn children(&self) -> Vec<pulsar_plugins::Summary> {
        let features = (self.features)();
        let mut registry = helio_feature_api::NodeRegistry::new();
        features.register_into(&mut registry);
        let nodes = registry.feature_nodes();
        features
            .ids()
            .into_iter()
            .map(|feature| pulsar_plugins::Summary {
                children: nodes
                    .get(feature)
                    .into_iter()
                    .flatten()
                    .map(|kind| {
                        let title = registry
                            .get(kind)
                            .map_or_else(|| kind.to_string(), |node| node.title.clone());
                        pulsar_plugins::Summary::new("render.node", *kind, title, self.origin)
                    })
                    .collect(),
                ..pulsar_plugins::Summary::new("render.feature", feature, feature, self.origin)
            })
            .collect()
    }
}

pulsar_plugins::registration!(RenderFeatureRegistration);

/// Every registered plugin's features: the aggregate the engine builds its
/// graphs over. It keeps no ownership; the plugin catalog
/// (`plugin_manager::catalog`) reads which plugin and feature brought each
/// node from the registrations themselves.
pub fn plugin_features() -> FeatureSet {
    pulsar_plugins::registered::<RenderFeatureRegistration>()
        .fold(FeatureSet::new(), |set, registration| {
            set.extended(&(registration.features)())
        })
}

/// The node kinds of the render features whose plugin is disabled, each with
/// the plugin's id: what a graph asset can still name that no longer exists.
/// (A registration whose origin is the engine's or no plugin's is never gated,
/// so it is never listed.)
pub fn disabled_feature_nodes() -> Vec<(String, &'static str)> {
    let mut absent = Vec::new();
    for registration in inventory::iter::<RenderFeatureRegistration> {
        if pulsar_plugins::origin_enabled(registration.origin) {
            continue;
        }
        let Some(plugin) = pulsar_plugins::plugin_of(registration.origin) else {
            continue;
        };
        let mut scratch = helio_feature_api::NodeRegistry::new();
        (registration.features)().register_into(&mut scratch);
        absent.extend(
            scratch
                .list()
                .into_iter()
                .map(|node| (node.kind.clone(), plugin.id.as_str())),
        );
    }
    absent
}

/// `config` with every registered plugin's feature settings.
pub fn configure(config: helio::RendererConfig) -> helio::RendererConfig {
    pulsar_plugins::registered::<RenderFeatureRegistration>()
        .fold(config, |config, registration| {
            (registration.configure)(config)
        })
}

/// How much wider than its frame a camera's view must render, as whatever
/// owns the camera's optics says (a fisheye or barrel-distorted lens maps
/// past the frame and crops back). Submitted by that plugin.
pub struct ViewOverscanRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// Stable id of the registration within its plugin (`"lens.overscan"`): what
    /// the plugin catalog lists it by.
    pub name: &'static str,
    /// The overscan of view `view_id` (1: none) in `world`, for a frame of
    /// `aspect` (width over height).
    pub overscan: fn(&pulsar_scenedb::World, u32, f32) -> f32,
}

impl pulsar_plugins::Contribution for ViewOverscanRegistration {
    const CATEGORY: &'static str = "render.view_overscan";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(ViewOverscanRegistration);

/// The largest overscan any registration asks of view `view_id` (1: none).
pub fn view_overscan(world: &pulsar_scenedb::World, view_id: u32, aspect: f32) -> f32 {
    pulsar_plugins::registered::<ViewOverscanRegistration>()
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
/// one watching `world`. Submitted by the plugin's root crate.
pub struct EditorLinesRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    pub name: &'static str,
    pub new: fn(&pulsar_scenedb::World) -> Box<dyn EditorLines>,
}

impl pulsar_plugins::Contribution for EditorLinesRegistration {
    const CATEGORY: &'static str = "render.editor_lines";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(EditorLinesRegistration);

/// Every registered line set, watching `world`.
pub fn editor_lines(world: &pulsar_scenedb::World) -> Vec<(&'static str, Box<dyn EditorLines>)> {
    pulsar_plugins::registered::<EditorLinesRegistration>()
        .map(|registration| (registration.name, (registration.new)(world)))
        .collect()
}
