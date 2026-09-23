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
    assert!(properties
        .iter()
        .all(|property| property.name != "source_revision"));
}

#[test]
fn live_payload_state_is_scene_owned_but_script_exfiltrated() {
    use std::sync::Arc;

    let terrain = VoxelTerrainComponent::default();
    let properties = terrain.get_properties();
    assert!(properties
        .iter()
        .all(|property| property.name != "payloads"));

    let mut serialized = serde_json::to_value(&terrain).expect("serialize authored config");
    assert!(serialized.get("payloads").is_none());
    {
        let store_handle = terrain.payload_store();
        let mut store = store_handle.write().unwrap();
        store.1.insert([u64::MAX, 0, 7, 2], Arc::from([1, 2, 3]));
        store.0 = 1;
    }

    // Scripts opt in to exfiltration by retaining the component's runtime
    // store handle; default component serialization omits live payload bytes.
    let export = terrain.payload_store();
    let snapshot = export.read().unwrap();
    assert_eq!(snapshot.0, 1);
    assert_eq!(
        snapshot.1.get(&[u64::MAX, 0, 7, 2]).unwrap().as_ref(),
        &[1, 2, 3]
    );
    drop(snapshot);

    // SceneDB/value clones retain the canonical payload value while owning
    // an independent mutable index, so cloning an entry cannot erase data or
    // make future writes leak between entities.
    let cloned = terrain.clone();
    assert_eq!(
        cloned.payload_store().read().unwrap().1[&[u64::MAX, 0, 7, 2]].as_ref(),
        &[1, 2, 3]
    );
    cloned
        .payload_store()
        .write()
        .unwrap()
        .1
        .remove(&[u64::MAX, 0, 7, 2]);
    assert_eq!(terrain.payload_store().read().unwrap().1.len(), 1);

    serialized = serde_json::to_value(&terrain).expect("payload remains caller-owned");
    assert!(serialized.get("payloads").is_none());
}

#[test]
fn voxel_components_hydrate_as_typed_scenedb_world_rows() {
    let mut world = pulsar_scenedb::World::new();
    let entity = world.spawn();
    let mut terrain = VoxelTerrainComponent::default();
    terrain.generator_id = "test.generator".into();
    terrain.seed = 1234;
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

#[test]
fn live_batch_publish_snapshot_and_import_round_trip_through_scenedb_rows() {
    use helio_pass_voxel_mesh::{
        BoundedVoxelInbox, VoxelBatchRevision, VoxelChunkBatch, VoxelChunkKey, VoxelChunkOp,
        VoxelChunkPayload, VoxelChunkUpdate, VoxelDomain, VoxelInboxDrainBudget, VoxelInboxLimits,
        VoxelSourceId, VoxelSourceWriter, VoxelTerrainId, VOXEL_CHUNK_ENCODING_RAW,
        VOXEL_CHUNK_SCHEMA_VERSION,
    };
    use std::sync::Arc;

    let mut world = pulsar_scenedb::World::new();
    let source_entity = world.spawn();
    assert!(pulsar_world_registry::hydrate_world_component_for_class(
        "VoxelTerrainComponent",
        &mut world,
        source_entity,
        &serde_json::to_value(VoxelTerrainComponent::default()).unwrap(),
    )
    .unwrap());

    let terrain_id = VoxelTerrainId(77);
    let source_id = VoxelSourceId(12);
    let source_store = world
        .get::<VoxelTerrainComponent>(source_entity)
        .expect("hydrated SceneDB terrain row")
        .payload_store();
    let writer = VoxelSourceWriter::new(terrain_id, source_id, source_store);

    let bytes: Arc<[u8]> = Arc::from([5_u8, 8, 13, 21]);
    let key = VoxelChunkKey::new(-4, 2, 19, 0);
    let ops = [VoxelChunkOp::Upsert(VoxelChunkUpdate {
        key,
        payload: VoxelChunkPayload {
            encoding: VOXEL_CHUNK_ENCODING_RAW,
            schema_version: VOXEL_CHUNK_SCHEMA_VERSION,
            bytes: &bytes,
        },
    })];
    let batch = VoxelChunkBatch {
        terrain: terrain_id,
        source: source_id,
        revision: VoxelBatchRevision {
            expected: 0,
            publish: 1,
        },
        domain: VoxelDomain::Unbounded { max_lod: 8 },
        ops: &ops,
    };
    let inbox = BoundedVoxelInbox::new(VoxelInboxLimits::default()).unwrap();
    inbox.try_submit(&batch, &[Arc::clone(&bytes)]).unwrap();
    let mut drained = inbox
        .try_drain(VoxelInboxDrainBudget {
            max_ops: 8,
            max_payload_bytes: 1024,
        })
        .unwrap();
    drained
        .batches
        .pop()
        .unwrap()
        .publish_into(&writer)
        .unwrap();

    let live = world
        .get::<VoxelTerrainComponent>(source_entity)
        .unwrap()
        .payload_store();
    let published = writer.snapshot().unwrap();
    assert_eq!(published.revision(), 1);
    assert_eq!(published.get(key), Some(&[5, 8, 13, 21][..]));
    assert_eq!(live.read().unwrap().0, published.revision());

    // The snapshot can be handed to another SceneDB component; the receiving
    // script still chooses whether/how to serialize it durably.
    let target_entity = world.spawn();
    assert!(pulsar_world_registry::hydrate_world_component_for_class(
        "VoxelTerrainComponent",
        &mut world,
        target_entity,
        &serde_json::to_value(VoxelTerrainComponent::default()).unwrap(),
    )
    .unwrap());
    let target_store = world
        .get::<VoxelTerrainComponent>(target_entity)
        .unwrap()
        .payload_store();
    let target_writer = VoxelSourceWriter::new(VoxelTerrainId(78), source_id, target_store);
    target_writer
        .replace_from_snapshot(&published, VoxelDomain::Unbounded { max_lod: 8 })
        .unwrap();
    let imported = target_writer.snapshot().unwrap();
    assert_eq!(imported.get(key), Some(&[5, 8, 13, 21][..]));
    assert_eq!(imported.revision(), 1);

    // Stale work is rejected without changing the component-owned row.
    assert!(matches!(
        writer.publish_batch(&batch),
        Err(helio_pass_voxel_mesh::VoxelUpdateError::StaleRevision {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(writer.snapshot().unwrap().revision(), 1);
}
