//! Meshes in the level editor: the Create menu's primitive meshes,
//! registered through the level editor's extension points
//! (`ui_level_editor::extend`). Each add is one undoable command.

use helio_component::StaticMeshComponent;
use ui_level_editor::LevelEditorState;
use ui_level_editor::commands::{CommandResult, TypedComponent};
use ui_level_editor::extend::{CreateEntry, add_object};
use ui_level_editor::scene_edit::{MeshType, ObjectType, Transform};

/// A primitive mesh (`meshes/primitives/<file>`) at the origin.
fn primitive(
    state: &mut LevelEditorState,
    name: &str,
    file: &str,
    kind: MeshType,
) -> CommandResult {
    let mesh = StaticMeshComponent::for_mesh_asset(&format!("meshes/primitives/{file}"));
    add_object(
        state,
        name,
        ObjectType::Mesh(kind),
        Transform::default(),
        vec![TypedComponent::new(mesh)],
    )
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "mesh_cube",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.Cube").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 200,
        create: |state| primitive(state, "Cube", "SM_Cube.fbx", MeshType::Cube),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "mesh_sphere",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.Sphere").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 210,
        create: |state| primitive(state, "Sphere", "SM_Sphere.fbx", MeshType::Sphere),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "mesh_cylinder",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.Cylinder").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 220,
        create: |state| primitive(state, "Cylinder", "SM_Cylinder.fbx", MeshType::Cylinder),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "mesh_plane",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.Plane").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 230,
        create: |state| primitive(state, "Plane", "SM_Plane.fbx", MeshType::Plane),
    }
}
