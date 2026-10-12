//! Asset reload (Pulsar-Native#1035 acceptance, #1081, SceneDB#84):
//! re-importing a mesh publishes a mesh `AssetUpdated`, and the level
//! editor's world loads that file again and rebinds every static mesh
//! showing it; a mesh naming another file is left alone. Its own test binary: the project path the loader resolves
//! against is a process global.

use std::path::Path;
use std::sync::Arc;

use helio_component::components::StaticMeshComponent;
use parking_lot::RwLock;
use serde_json::json;
use ui_level_editor::core::asset_updates;
use ui_level_editor::scene_edit::{components, objects, ObjectType};
use ui_level_editor::{LevelEditorState, SceneObjectData};

fn primitive(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../assets/meshes/primitives")
        .join(name)
}

fn mesh_object(state: &LevelEditorState, name: &str, asset: &str) -> String {
    let mut world = state.scene.world_mut();
    let id = objects::add_object(
        &mut world,
        SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type: ObjectType::Empty,
            transform: Default::default(),
            visible: true,
            locked: false,
            parent: None,
            children: Vec::new(),
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        },
        None,
    );
    components::add_component(
        &mut world,
        &id,
        "StaticMeshComponent".into(),
        json!({ "mesh_asset": asset }),
    );
    id
}

/// Load and bind every mesh asset the editor's world is waiting on, as the
/// renderer's per-frame pump would.
fn resolve(state: &LevelEditorState) {
    assert!(state
        .scene
        .world_mut()
        .resolve_assets_blocking(std::time::Duration::from_secs(30)));
}

/// How many vertices the mesh at `path` loads to.
fn loaded_vertices(path: &str) -> usize {
    let mut world = pulsar_scenedb::World::new();
    let entity = world.spawn();
    world.insert(entity, StaticMeshComponent::for_mesh_asset(path));
    assert!(world.resolve_assets_blocking(std::time::Duration::from_secs(30)));
    world.get::<StaticMeshComponent>(entity).unwrap().vertices.len()
}

fn vertices(state: &LevelEditorState, id: &str) -> usize {
    let world = state.scene.world();
    let instance = components::instance_at(&world, id, 0).unwrap();
    world
        .get::<StaticMeshComponent>(instance)
        .unwrap()
        .vertices
        .len()
}

#[test]
fn a_reimported_mesh_reloads_every_mesh_naming_it() {
    let project = tempfile::tempdir().unwrap();
    let meshes = project.path().join("meshes");
    std::fs::create_dir_all(&meshes).unwrap();
    std::fs::copy(primitive("SM_Cube.fbx"), meshes.join("prop.fbx")).unwrap();
    std::fs::copy(primitive("SM_Cylinder.fbx"), meshes.join("other.fbx")).unwrap();
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());
    let cylinder = loaded_vertices("meshes/other.fbx");
    std::fs::copy(primitive("SM_Sphere.fbx"), meshes.join("sphere.fbx")).unwrap();
    let sphere_vertices = loaded_vertices("meshes/sphere.fbx");
    let cube_vertices = loaded_vertices("meshes/prop.fbx");
    assert_ne!(cube_vertices, sphere_vertices);

    let state = Arc::new(RwLock::new(LevelEditorState::new()));
    let (a, b, other) = {
        let state = state.read();
        (
            mesh_object(&state, "a", "meshes/prop.fbx"),
            mesh_object(&state, "b", "meshes/prop.fbx"),
            mesh_object(&state, "other", "meshes/other.fbx"),
        )
    };
    resolve(&state.read());
    let _subscription = asset_updates::subscribe_mesh_updates(state.clone());

    // The asset is rewritten on disk (a re-import), then announced.
    std::fs::copy(primitive("SM_Sphere.fbx"), meshes.join("prop.fbx")).unwrap();
    {
        let state = state.read();
        assert_eq!(
            vertices(&state, &a),
            cube_vertices,
            "nothing reloads before the update"
        );
    }
    plugin_editor_api::publish_asset_updated(
        plugin_editor_api::AssetUpdated::new(plugin_editor_api::AssetKind::Mesh)
            .with_path(meshes.join("prop.fbx")),
    );

    let state = state.read();
    resolve(&state);
    assert_eq!(vertices(&state, &a), sphere_vertices, "a reloaded");
    assert_eq!(vertices(&state, &b), sphere_vertices, "b reloaded");
    assert_eq!(
        vertices(&state, &other),
        cylinder,
        "a mesh naming another file is untouched"
    );
}
