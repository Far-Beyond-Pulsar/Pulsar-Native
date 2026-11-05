//! Water in the level editor's environment mixer: its water volumes, registered
//! through the level editor's extension points (`ui_level_editor::extend`).
//! Each add is one undoable command; the components are this plugin's.

use ui_level_editor::commands::TypedComponent;
use ui_level_editor::extend::{CreateEntry, EnvironmentListing, add_object};
use ui_level_editor::scene_edit::{ObjectType, Transform};

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_water_volume",
        menu_label: None,
        environment_label: Some("Water Volume"),
        quick_add: None,
        offered: None,
        order: 450,
        create: |state| {
            add_object(
                state,
                "Water Volume",
                ObjectType::Empty,
                Transform::default(),
                vec![TypedComponent::new(water_component::WaterVolumeComponent::default())],
            )
        },
    }
}

inventory::submit! {
    EnvironmentListing {
        origin: pulsar_plugins::origin!(),
        class_name: "WaterVolumeComponent",
        label: "Water Volume",
        order: 50,
        create_entry: Some("environment_water_volume"),
    }
}
