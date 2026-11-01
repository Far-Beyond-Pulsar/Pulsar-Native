use plugin_editor_api::*;

#[derive(Default)]
struct FixturePlugin;

impl Plugin for FixturePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("fixture.base", "Fixture base", "0.1.3", "tests")
    }

    fn build(&self, _p: &mut PluginBuilder) {}
}

export_plugin_v2!(FixturePlugin);
