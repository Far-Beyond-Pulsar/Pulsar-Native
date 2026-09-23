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
