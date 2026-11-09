//! The catalog fixture plugin's root crate: everything the plugin
//! contributes, registered here from its component and editor crates.

pulsar_plugins::installed!("pulsar-plugin.toml");

use catalog_fixture_component as component;

// Two features, two nodes each.
inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        origin: pulsar_plugins::origin!(),
        features: component::render_features,
        configure: component::configure,
    }
}

inventory::submit! {
    engine_backend::scene::game_hooks::ContentReportRegistration {
        origin: pulsar_plugins::origin!(),
        key: "test_catalog",
        report: component::report,
    }
}

inventory::submit! {
    pulsar_settings::project::ProjectSettingsRegistration {
        origin: pulsar_plugins::origin!(),
        name: "test_catalog.settings",
        register: component::register_settings,
    }
}

#[cfg(feature = "editor")]
mod editor {
    inventory::submit! {
        ui_common::asset_thumbnails::ThumbnailRendererRegistration {
            origin: pulsar_plugins::origin!(),
            extension: "testcatalog",
            render: catalog_fixture_editor::render_thumbnail,
        }
    }

    inventory::submit! {
        ui_level_editor::extend::AiToolNamespace {
            origin: pulsar_plugins::origin!(),
            namespace: catalog_fixture_editor::TOOLS,
        }
    }
}
