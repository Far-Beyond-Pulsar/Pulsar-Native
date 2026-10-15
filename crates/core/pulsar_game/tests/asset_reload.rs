//! Asset reload without the editor (SceneDB#84): a world a game loop built
//! reloads a re-imported mesh when its `AssetUpdated` is published, and
//! rebinds every static mesh showing it. Headless, no editor subscription.
//! Its own test binary: the project path the loader resolves against is a
//! process global.

use std::path::Path;
use std::time::Duration;

use helio_component::components::StaticMeshComponent;
use pulsar_game::tick::TickLoop;
use pulsar_game::TickMode;
use pulsar_scenedb::{Entity, World};

fn primitive(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../assets/meshes/primitives")
        .join(name)
}

fn spawn_mesh(world: &mut World, asset: &str) -> Entity {
    let entity = world.spawn();
    world.insert(entity, StaticMeshComponent::for_mesh_asset(asset));
    entity
}

fn vertices(world: &World, entity: Entity) -> usize {
    world
        .get::<StaticMeshComponent>(entity)
        .unwrap()
        .vertices
        .len()
}

/// Load and bind every mesh asset `game`'s world is waiting on, as the
/// game's per-tick pump does.
fn resolve(game: &TickLoop) {
    assert!(game
        .scene_store
        .write()
        .world
        .resolve_assets_blocking(Duration::from_secs(30)));
}

#[test]
fn a_game_world_reloads_a_reimported_mesh_without_the_editor() {
    let project = tempfile::tempdir().unwrap();
    let meshes = project.path().join("meshes");
    std::fs::create_dir_all(&meshes).unwrap();
    std::fs::copy(primitive("SM_Cube.fbx"), meshes.join("prop.fbx")).unwrap();
    std::fs::copy(primitive("SM_Cylinder.fbx"), meshes.join("other.fbx")).unwrap();
    std::fs::copy(primitive("SM_Sphere.fbx"), meshes.join("sphere.fbx")).unwrap();
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());

    let game = TickLoop::new(TickMode::default(), 0);
    let (a, b, other, sphere) = {
        let world = &mut game.scene_store.write().world;
        (
            spawn_mesh(world, "meshes/prop.fbx"),
            spawn_mesh(world, "meshes/prop.fbx"),
            spawn_mesh(world, "meshes/other.fbx"),
            spawn_mesh(world, "meshes/sphere.fbx"),
        )
    };
    resolve(&game);
    let (cube_vertices, cylinder_vertices, sphere_vertices) = {
        let world = &game.scene_store.read().world;
        (
            vertices(world, a),
            vertices(world, other),
            vertices(world, sphere),
        )
    };
    assert_ne!(cube_vertices, sphere_vertices);

    // The asset is rewritten on disk (a re-import), then announced.
    std::fs::copy(primitive("SM_Sphere.fbx"), meshes.join("prop.fbx")).unwrap();
    resolve(&game);
    assert_eq!(
        vertices(&game.scene_store.read().world, a),
        cube_vertices,
        "nothing reloads before the update"
    );
    pulsar_events::publish_asset_updated(
        pulsar_events::AssetUpdated::new(pulsar_events::AssetKind::Mesh)
            .with_path(meshes.join("prop.fbx")),
    );

    resolve(&game);
    let world = &game.scene_store.read().world;
    assert_eq!(vertices(world, a), sphere_vertices, "a reloaded");
    assert_eq!(vertices(world, b), sphere_vertices, "b reloaded");
    assert_eq!(
        vertices(world, other),
        cylinder_vertices,
        "a mesh naming another file is untouched"
    );
}
