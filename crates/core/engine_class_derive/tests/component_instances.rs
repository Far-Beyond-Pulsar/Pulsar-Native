//! Phase 1 exit (Pulsar-Native#1035, D1): a registered component created by
//! its generic factory and attached through erased APIs behaves exactly like
//! a direct typed insert -- value, write hooks, mutation, removal and GPU
//! row -- with no render-specific call; several and disabled instances of
//! one class are separate typed values; JSON records round-trip at the
//! boundary, keeping what this build cannot decode as explicit unresolved
//! payloads; and a mirror attached after population sees every instance.
//!
//! The GPU half needs an adapter (any Vulkan device, including lavapipe)
//! and is skipped without one.

use engine_class_derive::{engine_class, register_world_component};
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};
use pulsar_scenedb::{ComponentChangeKind, World};
use pulsar_world_registry::pulsar_scene_model::attachments::{
    self, NewInstance, UnresolvedComponent,
};
use pulsar_world_registry::pulsar_scene_model::ComponentInstance as Record;
use pulsar_world_registry::{ComponentPayload, GpuMirrored};
use std::sync::Arc;

#[engine_class(
    gpu_rows,
    category = "Test",
    default,
    clone,
    debug,
    serialize,
    deserialize,
    no_register
)]
pub struct InstanceProbe {
    #[property]
    #[gpu]
    pub value: f32,
    #[property]
    pub label: String,
}

#[register_world_component]
impl ComponentRuntimeBehavior for InstanceProbe {
    const CLASS_NAME: &'static str = "InstanceProbe";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component: &Self,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
    }
}

const CLASS: &str = "InstanceProbe";

fn probe(value: f32) -> InstanceProbe {
    InstanceProbe {
        value,
        label: "x".into(),
    }
}

#[test]
fn factory_decode_and_typed_inserts_are_equivalent() {
    let mut world = World::new();
    let object = world.spawn();

    // Typed reference: a plain insert on its own entity.
    let typed = world.spawn();
    world.insert(typed, probe(0.0));

    // The generic factory, attached through the erased path.
    let from_factory = pulsar_world_registry::attach_component(
        &mut world,
        object,
        NewInstance::new(CLASS),
        ComponentPayload::Default,
    )
    .unwrap();
    // A boundary decode.
    let from_json = pulsar_world_registry::attach_component(
        &mut world,
        object,
        NewInstance::new(CLASS),
        ComponentPayload::Json(serde_json::json!({ "value": 0.0, "label": "x" })),
    )
    .unwrap();

    let fields = |e| {
        world
            .get::<InstanceProbe>(e)
            .map(|p| (p.value, p.label.clone()))
    };
    let default_probe = InstanceProbe::default();
    assert_eq!(
        fields(from_factory),
        Some((default_probe.value, default_probe.label))
    );
    assert_eq!(fields(from_json), fields(typed));

    // A reflected property write on the erased instance records the same
    // journal entries a typed guarded write does.
    let mut cursor = world.open_change_cursor::<InstanceProbe>();
    pulsar_world_registry::set_world_component_property(
        CLASS,
        &mut world,
        from_json,
        "value",
        Box::new(5.0_f32),
    )
    .unwrap();
    world.get_mut::<InstanceProbe>(typed).unwrap().value = 5.0;
    assert_eq!(
        world.get::<InstanceProbe>(from_json).map(|p| p.value),
        Some(5.0)
    );
    let mut changes = Vec::new();
    let _ = world.read_changes(&mut cursor, &mut changes);
    assert_eq!(changes.len(), 2);
    assert!(changes
        .iter()
        .all(|c| c.kind == ComponentChangeKind::Mutated));

    // Detaching removes the value with the instance; the object stays.
    assert!(attachments::detach(&mut world, from_json));
    assert!(world.get::<InstanceProbe>(from_json).is_none());
    assert_eq!(attachments::instances(&world, object), vec![from_factory]);
    assert!(world.is_alive(object));
}

#[test]
fn several_and_disabled_instances_are_separate_typed_values() {
    let mut world = World::new();
    let object = world.spawn();
    let mut disabled = NewInstance::new(CLASS);
    disabled.enabled = false;
    let a = pulsar_world_registry::attach_component(
        &mut world,
        object,
        NewInstance::new(CLASS),
        ComponentPayload::Value(Box::new(probe(1.0))),
    )
    .unwrap();
    let b = pulsar_world_registry::attach_component(
        &mut world,
        object,
        NewInstance::new(CLASS),
        ComponentPayload::Value(Box::new(probe(2.0))),
    )
    .unwrap();
    let c = pulsar_world_registry::attach_component(
        &mut world,
        object,
        disabled,
        ComponentPayload::Value(Box::new(probe(3.0))),
    )
    .unwrap();

    let values: Vec<f32> = [a, b, c]
        .iter()
        .map(|e| world.get::<InstanceProbe>(*e).unwrap().value)
        .collect();
    assert_eq!(
        values,
        vec![1.0, 2.0, 3.0],
        "each instance keeps its own value, disabled included"
    );
    let enabled: Vec<f32> = attachments::enabled_components_of::<InstanceProbe>(&world, object)
        .into_iter()
        .map(|(_, p)| p.value)
        .collect();
    assert_eq!(enabled, vec![1.0, 2.0]);

    // Addressing by owner + class-local ordinal reaches the second instance
    // itself, not the first.
    let second =
        pulsar_world_registry::instances::resolve_instance(&world, object, CLASS, 1).unwrap();
    assert_eq!(second, b);
    pulsar_world_registry::set_world_component_property(
        CLASS,
        &mut world,
        second,
        "value",
        Box::new(9.0_f32),
    )
    .unwrap();
    assert_eq!(world.get::<InstanceProbe>(b).unwrap().value, 9.0);
    assert_eq!(world.get::<InstanceProbe>(a).unwrap().value, 1.0);

    // Duplicating clones the value under a fresh id.
    let copy = pulsar_world_registry::duplicate_instance(&mut world, b, object, None).unwrap();
    assert_eq!(world.get::<InstanceProbe>(copy).unwrap().value, 9.0);
    assert_ne!(
        attachments::meta(&world, copy).unwrap().id,
        attachments::meta(&world, b).unwrap().id
    );

    // Despawning the object despawns every instance with it.
    use pulsar_world_registry::pulsar_scene_model::SceneWorldExt;
    world.despawn_tree(object);
    assert!([a, b, c, copy].iter().all(|e| !world.is_alive(*e)));
}

#[test]
fn records_round_trip_and_keep_undecodable_payloads() {
    let mut world = World::new();
    let object = world.spawn();
    let parent = pulsar_world_registry::attach_component(
        &mut world,
        object,
        NewInstance::new(CLASS),
        ComponentPayload::Value(Box::new(probe(1.0))),
    )
    .unwrap();
    let mut child_spec = NewInstance::new(CLASS);
    child_spec.parent = Some(attachments::meta(&world, parent).unwrap().id);
    child_spec.enabled = false;
    pulsar_world_registry::attach_component(
        &mut world,
        object,
        child_spec,
        ComponentPayload::Value(Box::new(probe(2.0))),
    )
    .unwrap();

    let original_parent_id = attachments::meta(&world, parent).unwrap().id;
    let mut records = pulsar_world_registry::component_records(&world, object);
    // A class this build does not know, and known data that does not decode.
    records.push(Record {
        class_name: "FromAPlugin".into(),
        enabled: true,
        data: serde_json::json!({ "anything": [1, 2] }),
    });
    records.push(Record {
        class_name: CLASS.into(),
        enabled: true,
        data: serde_json::json!({ "value": "not a number" }),
    });

    // Load into another world, as a level load would.
    let mut loaded = World::new();
    let other = loaded.spawn();
    let attached = pulsar_world_registry::attach_records(&mut loaded, other, &records).unwrap();
    let world = loaded;
    assert_eq!(attached.len(), 4);
    assert_eq!(world.get::<InstanceProbe>(attached[0]).unwrap().value, 1.0);
    assert!(!attachments::is_enabled(&world, attached[1]));
    assert_eq!(
        attachments::meta(&world, attached[1]).unwrap().parent,
        Some(attachments::meta(&world, attached[0]).unwrap().id),
        "the parent link survives the round trip"
    );
    for unresolved in &attached[2..] {
        assert!(
            world.get::<InstanceProbe>(*unresolved).is_none(),
            "nothing live is claimed"
        );
        assert!(world.get::<UnresolvedComponent>(*unresolved).is_some());
    }
    // Stable ids are preserved by the round trip.
    assert_eq!(
        attachments::meta(&world, attached[0]).unwrap().id,
        original_parent_id
    );
    // Attaching the same ids twice into one world is refused, not duplicated.
    let mut again = world;
    let dup = again.spawn();
    assert!(pulsar_world_registry::attach_records(&mut again, dup, &records[..1]).is_err());
    let world = again;
    // Saving again writes the unresolved payloads back unchanged.
    let saved = pulsar_world_registry::component_records(&world, other);
    assert_eq!(saved[2].data["anything"], serde_json::json!([1, 2]));
    assert_eq!(saved[3].data["value"], serde_json::json!("not a number"));
}

#[test]
fn a_refused_attach_writes_nothing() {
    let mut world = World::new();
    let object = world.spawn();
    let refused = pulsar_world_registry::attach_component(
        &mut world,
        object,
        NewInstance::new(CLASS),
        ComponentPayload::Json(serde_json::json!({ "value": "nope" })),
    );
    assert!(matches!(
        refused,
        Err(pulsar_world_registry::AttachError::Decode { .. })
    ));
    let wrong_type = pulsar_world_registry::attach_component(
        &mut world,
        object,
        NewInstance::new(CLASS),
        ComponentPayload::Value(Box::new(17_u32)),
    );
    assert!(matches!(
        wrong_type,
        Err(pulsar_world_registry::AttachError::Value(_))
    ));
    assert!(attachments::instances(&world, object).is_empty());
    assert_eq!(world.query::<&attachments::ComponentMeta>().count(), 0);
}

#[test]
fn a_mirror_attached_after_population_sees_every_instance() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let ctx = EngineGpuContext::new(Arc::new(device), Arc::new(queue));

    let mut world = World::new();
    let object = world.spawn();
    let mut disabled = NewInstance::new(CLASS);
    disabled.enabled = false;
    let a = pulsar_world_registry::attach_component(
        &mut world,
        object,
        NewInstance::new(CLASS),
        ComponentPayload::Value(Box::new(probe(4.0))),
    )
    .unwrap();
    let b = pulsar_world_registry::attach_component(
        &mut world,
        object,
        disabled,
        ComponentPayload::Value(Box::new(probe(6.0))),
    )
    .unwrap();

    let store = Arc::new(SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![],
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    ));
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));
    world.flush_gpu_mirror(ctx.queue()).unwrap();

    type Mirror = <InstanceProbe as GpuMirrored>::GpuMirror;
    let read_f32 = |key_id: pulsar_scenedb::ComponentId, row: u32, words: u64| -> Vec<u32> {
        let handle = store
            .resolve_buffer_handle(store.buffer_key_for(key_id).expect("registered"))
            .unwrap();
        let bytes = words * 4;
        let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device().create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&handle.buffer, row as u64 * bytes, &staging, 0, bytes);
        ctx.queue().submit([encoder.finish()]);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let out = staging
            .slice(..)
            .get_mapped_range()
            .unwrap()
            .chunks_exact(4)
            .map(|w| u32::from_ne_bytes(w.try_into().unwrap()))
            .collect();
        staging.unmap();
        out
    };

    // Each instance's own value row, keyed by the instance entity.
    assert_eq!(
        f32::from_bits(read_f32(Mirror::packed_gpu_component_id(), a.index(), 1)[0]),
        4.0
    );
    assert_eq!(
        f32::from_bits(read_f32(Mirror::packed_gpu_component_id(), b.index(), 1)[0]),
        6.0
    );
    // And its owner-key row: owner index, owner generation, enabled.
    let owner_id = attachments::ComponentOwner::packed_gpu_component_id();
    assert_eq!(
        read_f32(owner_id, a.index(), 3),
        vec![object.index(), object.generation(), 1]
    );
    assert_eq!(
        read_f32(owner_id, b.index(), 3),
        vec![object.index(), object.generation(), 0]
    );
}
