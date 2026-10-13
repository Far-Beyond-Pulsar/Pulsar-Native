//! What the level editor's Create menu adds (Pulsar-Native#1012): one object
//! of a kind, with the components that make it what its name says, as one
//! undoable command. The new object is selected.

use crate::commands::{execute_command, CommandResult, SceneCommand, TypedComponent};
use crate::scene_edit::{LightType, MeshType, ObjectType, SceneObjectData, Transform};
use crate::state::LevelEditorState;

/// Every kind [`create`] knows, in menu order, with its locale key.
pub const KINDS: &[(&str, &str)] = &[
    ("empty", "LevelEditor.Create.Empty"),
    ("folder", "LevelEditor.Create.Folder"),
    ("camera", "LevelEditor.Create.Camera"),
    ("light_point", "LevelEditor.Create.PointLight"),
    ("light_spot", "LevelEditor.Create.SpotLight"),
    ("light_directional", "LevelEditor.Create.DirectionalLight"),
    ("mesh_cube", "LevelEditor.Create.Cube"),
    ("mesh_sphere", "LevelEditor.Create.Sphere"),
    ("mesh_cylinder", "LevelEditor.Create.Cylinder"),
    ("mesh_plane", "LevelEditor.Create.Plane"),
    ("sky", "LevelEditor.Create.Sky"),
    ("wind", "LevelEditor.Create.Wind"),
];

/// Add an object of `kind` (one of [`KINDS`]) at the origin and select it.
pub fn create(state: &mut LevelEditorState, kind: &str) -> CommandResult {
    let result = match kind {
        "sky" => super::sky::create_sky(state),
        "wind" => super::wind::create_wind(state),
        _ => {
            let Some((name, object_type, components)) = object_for(kind) else {
                return CommandResult::noop("Unknown object kind");
            };
            execute_command(
                state,
                SceneCommand::AddObjectWithComponents {
                    data: SceneObjectData {
                        id: String::new(),
                        name: name.to_string(),
                        object_type,
                        transform: Transform::default(),
                        visible: true,
                        locked: false,
                        parent: None,
                        children: vec![],
                        scene_path: String::new(),
                        props: Default::default(),
                        component_instances: None,
                    },
                    parent_id: None,
                    components,
                },
            )
        }
    };
    if let Some(id) = result.affected_ids.first() {
        execute_command(
            state,
            SceneCommand::SelectObject {
                id: Some(id.clone()),
            },
        );
    }
    result
}

fn object_for(kind: &str) -> Option<(&'static str, ObjectType, Vec<TypedComponent>)> {
    let light = |name, light_type, kind| {
        let mut light = helio_component::LightComponent::default();
        light.general.enabled = true;
        light.general.light_type = light_type;
        (name, ObjectType::Light(kind), vec![TypedComponent::new(light)])
    };
    let mesh = |name, file: &str, kind| {
        let mesh = helio_component::StaticMeshComponent::for_mesh_asset(&format!(
            "meshes/primitives/{file}"
        ));
        (name, ObjectType::Mesh(kind), vec![TypedComponent::new(mesh)])
    };
    use helio_component::LightType as Helio;
    Some(match kind {
        "empty" => ("Empty", ObjectType::Empty, vec![]),
        "folder" => ("Folder", ObjectType::Folder, vec![]),
        "camera" => ("Camera", ObjectType::Camera, vec![]),
        "light_point" => light("Point Light", Helio::Point, LightType::Point),
        "light_spot" => light("Spot Light", Helio::Spot, LightType::Spot),
        "light_directional" => light(
            "Directional Light",
            Helio::Directional,
            LightType::Directional,
        ),
        "mesh_cube" => mesh("Cube", "SM_Cube.fbx", MeshType::Cube),
        "mesh_sphere" => mesh("Sphere", "SM_Sphere.fbx", MeshType::Sphere),
        "mesh_cylinder" => mesh("Cylinder", "SM_Cylinder.fbx", MeshType::Cylinder),
        "mesh_plane" => mesh("Plane", "SM_Plane.fbx", MeshType::Plane),
        _ => return None,
    })
}
