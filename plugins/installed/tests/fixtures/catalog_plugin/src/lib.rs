//! The catalog fixture plugin's root crate: everything the plugin
//! contributes, registered here from its component and editor crates.

pulsar_plugins::installed!("pulsar-plugin.toml", build);

use catalog_fixture_component as component;

/// Everything the plugin registers (runtime always, editor with the `editor`
/// feature).
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    // Two features, two nodes each.
    p.add(
        engine_backend::subsystems::render::RenderFeatureRegistration {
            origin: p.origin(),
            features: component::render_features,
            configure: component::configure,
        },
    );

    p.add(
        engine_backend::scene::game_hooks::ContentReportRegistration {
            origin: p.origin(),
            key: "test_catalog",
            report: component::report,
        },
    );

    p.add(pulsar_settings::project::ProjectSettingsRegistration {
        origin: p.origin(),
        name: "test_catalog.settings",
        register: component::register_settings,
    });

    #[cfg(feature = "editor")]
    editor::build(p);
}

#[cfg(feature = "editor")]
mod editor {
    pub(super) fn build(p: &mut pulsar_plugins::PluginBuilder) {
        p.add(ui_common::asset_thumbnails::ThumbnailRendererRegistration {
            origin: p.origin(),
            extension: "testcatalog",
            render: catalog_fixture_editor::render_thumbnail,
        });

        p.add(ui_level_editor::extend::AiToolNamespace {
            origin: p.origin(),
            namespace: catalog_fixture_editor::TOOLS,
        });
    }
}
