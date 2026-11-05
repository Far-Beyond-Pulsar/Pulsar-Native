//! What the level editor's Create menu adds (Pulsar-Native#1012): one object
//! of a kind, with the components that make it what its name says, as one
//! undoable command. The new object is selected.
//!
//! The menu lists the registered [`crate::extend::CreateEntry`]s, grouped by
//! hundreds of their `order` (objects 0.., lights 100.., meshes 200..,
//! environment 300..). The shell adds the three kinds it knows itself (an
//! empty object, a folder, a camera); every other entry is a plugin's.

use crate::commands::CommandResult;
use crate::extend::{add_object, CreateEntry};
use crate::scene_edit::{ObjectType, Transform};
use crate::state::LevelEditorState;

/// Add an object of the registered create entry `id` and select it.
pub fn create(state: &mut LevelEditorState, id: &str) -> CommandResult {
    crate::extend::create(state, id)
}

fn shell_object(
    state: &mut LevelEditorState,
    name: &str,
    object_type: ObjectType,
) -> CommandResult {
    add_object(state, name, object_type, Transform::default(), vec![])
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "empty",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.Empty").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 0,
        create: |state| shell_object(state, "Empty", ObjectType::Empty),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "folder",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.Folder").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 10,
        create: |state| shell_object(state, "Folder", ObjectType::Folder),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "camera",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.Camera").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 20,
        create: |state| shell_object(state, "Camera", ObjectType::Camera),
    }
}
