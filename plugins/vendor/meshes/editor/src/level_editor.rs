//! Meshes in the level editor: the Create menu's primitive meshes,
//! registered through the level editor's extension points
//! (`ui_level_editor::extend`). Each add is one undoable command.

use meshes_component::StaticMeshComponent;
use ui_level_editor::LevelEditorState;
use ui_level_editor::commands::{CommandResult, TypedComponent};
use ui_level_editor::extend::{CreateEntry, add_object};
use ui_level_editor::scene_edit::{ObjectType, Transform};

// An object holding a mesh shows as one.
inventory::submit! {
    ui_level_editor::extend::ObjectIconRegistration {
        origin: pulsar_plugins::origin!(),
        order: 200,
        icon: |world, object| {
            pulsar_scene_model::attachments::instances(world, object)
                .iter()
                .any(|instance| {
                    world.get::<StaticMeshComponent>(*instance).is_some()
                        || world
                            .get::<meshes_component::components::SkeletalMeshComponent>(*instance)
                            .is_some()
                })
                .then_some((ui::IconName::Box, ui::hierarchical_tree::tree_colors::CODE_PURPLE))
        },
    }
}

/// A primitive mesh (`meshes/primitives/<file>`) at the origin.
fn primitive(state: &mut LevelEditorState, name: &str, file: &str) -> CommandResult {
    let mesh = StaticMeshComponent::for_mesh_asset(&format!("meshes/primitives/{file}"));
    add_object(
        state,
        name,
        ObjectType::Empty,
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
        create: |state| primitive(state, "Cube", "SM_Cube.fbx"),
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
        create: |state| primitive(state, "Sphere", "SM_Sphere.fbx"),
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
        create: |state| primitive(state, "Cylinder", "SM_Cylinder.fbx"),
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
        create: |state| primitive(state, "Plane", "SM_Plane.fbx"),
    }
}
