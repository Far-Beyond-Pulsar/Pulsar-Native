use plugin_editor_api::*;

#[derive(Default)]
struct FixturePlugin;

impl Plugin for FixturePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("fixture.top", "Fixture top", "0.1.0", "tests").requires(PluginDependency::new("fixture.base", "^0.1"))
    }

    fn build(&self, _p: &mut PluginBuilder) {}
}

export_plugin_v2!(FixturePlugin);
