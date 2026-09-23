use helio_component::{VoxelComponent, VoxelTerrainComponent};
use pulsar_reflection::EngineClass;

#[test]
fn voxel_components_default_and_round_trip_as_scene_component_data() {
    let voxel = VoxelComponent::default();
    assert_eq!(voxel.dimensions, [16; 3]);
    assert!(!voxel.smooth_surface);
    let voxel_json = serde_json::to_value(&voxel).expect("serialize voxel component");
    let voxel_restored: VoxelComponent =
        serde_json::from_value(voxel_json).expect("restore voxel component");
    assert_eq!(voxel_restored.material_ids, voxel.material_ids);
    assert_eq!(voxel_restored.dimensions, voxel.dimensions);

    let terrain = VoxelTerrainComponent::default();
    assert_eq!(terrain.domain_mode, 1);
    assert_eq!(terrain.shape_mode, 0);
    let terrain_json = serde_json::to_value(&terrain).expect("serialize terrain component");
    let terrain_restored: VoxelTerrainComponent =
        serde_json::from_value(terrain_json).expect("restore terrain component");
    assert_eq!(terrain_restored.generator_id, terrain.generator_id);
    assert_eq!(terrain_restored.source_revision, terrain.source_revision);
}

#[test]
fn service_revision_is_persisted_but_not_exposed_as_an_inspector_property() {
    let properties = VoxelTerrainComponent::default().get_properties();
    assert_eq!(properties.len(), 20);
    assert!(properties.iter().all(|property| property.name != "source_revision"));
}

#[test]
fn voxel_components_hydrate_as_typed_scenedb_world_rows() {
    let mut world = pulsar_scenedb::World::new();
    let entity = world.spawn();
    let terrain = VoxelTerrainComponent {
        generator_id: "test.generator".into(),
        seed: 1234,
        ..VoxelTerrainComponent::default()
    };
    let terrain_json = serde_json::to_value(&terrain).unwrap();
    assert!(pulsar_world_registry::hydrate_world_component_for_class(
        "VoxelTerrainComponent",
        &mut world,
        entity,
        &terrain_json,
    )
    .unwrap());
    let hydrated = world
        .get::<VoxelTerrainComponent>(entity)
        .expect("typed SceneDB component");
    assert_eq!(hydrated.generator_id, "test.generator");
    assert_eq!(hydrated.seed, 1234);

    let voxel = VoxelComponent::default();
    let voxel_json = serde_json::to_value(&voxel).unwrap();
    assert!(pulsar_world_registry::hydrate_world_component_for_class(
        "VoxelComponent",
        &mut world,
        entity,
        &voxel_json,
    )
    .unwrap());
    assert!(world.get::<VoxelComponent>(entity).is_some());
}
