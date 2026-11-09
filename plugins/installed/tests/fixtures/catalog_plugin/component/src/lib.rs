//! The catalog fixture plugin's runtime code. Registers nothing: its root
//! crate does.

use helio_feature_api::{Feature, FeatureRegistrar, FeatureSet, NodeDescriptor};

/// The fixture's features and the nodes each registers.
pub const FEATURES: [(&str, [&str; 2]); 2] = [
    (
        "test_catalog.a",
        ["test_catalog.a.first", "test_catalog.a.second"],
    ),
    (
        "test_catalog.b",
        ["test_catalog.b.first", "test_catalog.b.second"],
    ),
];

struct Fixture(usize);

impl Feature for Fixture {
    fn id(&self) -> &'static str {
        FEATURES[self.0].0
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        for kind in FEATURES[self.0].1 {
            registrar.node(NodeDescriptor::new(kind, kind, "Test", "", |_| Ok(())));
        }
    }
}

/// The fixture's features.
pub fn render_features() -> FeatureSet {
    FeatureSet::new().with(Fixture(0)).with(Fixture(1))
}

/// Its features take no settings.
pub fn configure(config: helio::RendererConfig) -> helio::RendererConfig {
    config
}

/// What a headless run reports of it.
pub fn report(_: &pulsar_scenedb::World) -> serde_json::Value {
    serde_json::json!({})
}

/// Its settings namespace (none).
pub fn register_settings(_: &'static pulsar_config::ConfigManager) {}
