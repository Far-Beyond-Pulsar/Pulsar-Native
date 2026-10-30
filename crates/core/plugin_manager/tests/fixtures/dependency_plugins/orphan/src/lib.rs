use plugin_editor_api::*;

#[derive(Default)]
struct FixturePlugin;

impl Plugin for FixturePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("fixture.orphan", "Fixture orphan", "0.1.0", "tests").requires(PluginDependency::any("fixture.absent"))
    }

    fn build(&self, _p: &mut PluginBuilder) {}
}

export_plugin_v2!(FixturePlugin);
