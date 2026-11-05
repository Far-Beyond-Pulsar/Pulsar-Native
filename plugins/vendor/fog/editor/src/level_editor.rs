//! Fog in the level editor's environment mixer: its global fog and local fog volumes, registered
//! through the level editor's extension points (`ui_level_editor::extend`).
//! Each add is one undoable command; the components are this plugin's.

use ui_level_editor::commands::TypedComponent;
use ui_level_editor::extend::{CreateEntry, EnvironmentListing, add_object};
use ui_level_editor::scene_edit::{ObjectType, Transform};

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_global_fog",
        menu_label: None,
        environment_label: Some("Global Fog"),
        quick_add: None,
        offered: None,
        order: 410,
        create: |state| {
            add_object(
                state,
                "Global Fog",
                ObjectType::Empty,
                Transform::default(),
                vec![TypedComponent::new(fog_component::GlobalFogComponent::default())],
            )
        },
    }
}

inventory::submit! {
    EnvironmentListing {
        origin: pulsar_plugins::origin!(),
        class_name: "GlobalFogComponent",
        label: "Global Fog",
        order: 10,
        create_entry: Some("environment_global_fog"),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_local_fog_volume",
        menu_label: None,
        environment_label: Some("Local Fog Volume"),
        quick_add: None,
        offered: None,
        order: 420,
        create: |state| {
            add_object(
                state,
                "Local Fog Volume",
                ObjectType::Empty,
                Transform::default(),
                vec![TypedComponent::new(fog_component::LocalFogVolumeComponent::default())],
            )
        },
    }
}

inventory::submit! {
    EnvironmentListing {
        origin: pulsar_plugins::origin!(),
        class_name: "LocalFogVolumeComponent",
        label: "Local Fog Volume",
        order: 20,
        create_entry: Some("environment_local_fog_volume"),
    }
}
