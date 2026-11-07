//! Post-process volumes in the level editor's environment mixer: its post-process volumes, registered
//! through the level editor's extension points (`ui_level_editor::extend`).
//! Each add is one undoable command; the components are this plugin's.

use ui_level_editor::commands::TypedComponent;
use ui_level_editor::extend::{CreateEntry, EnvironmentListing, add_object};
use ui_level_editor::scene_edit::{ObjectType, Transform};

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_post_process_volume",
        menu_label: None,
        environment_label: Some("Post Process"),
        quick_add: None,
        offered: None,
        order: 430,
        create: |state| {
            add_object(
                state,
                "Post Process Volume",
                ObjectType::Empty,
                Transform::default(),
                vec![TypedComponent::new(post_component::PostProcessVolumeComponent::default())],
            )
        },
    }
}

inventory::submit! {
    EnvironmentListing {
        origin: pulsar_plugins::origin!(),
        class_name: "PostProcessVolumeComponent",
        label: "Post Process Volume",
        order: 30,
        create_entry: Some("environment_post_process_volume"),
    }
}
