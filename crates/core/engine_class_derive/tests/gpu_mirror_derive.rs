//! Proves `#[gpu]` on a `#[property]` field (Pulsar-Native#561's
//! auto-derived GPU mirroring, `gpu_mirror_codegen` in `src/lib.rs`) works
//! end to end, in isolation, on throwaway test types -- before any real
//! primitive (reflection capture, water volume, ...) is pointed at it.
//!
//! Covers the three packing rules (`Direct` for numeric primitives/fixed-
//! size arrays, `AsU32` for `bool` and a plain enum), `#[sub_props]`
//! composition (a containing struct's mirror embeds its sub-props groups'
//! own independently-generated mirrors), the `NoGpuMirror` case for a
//! struct with no `#[gpu]` fields at all, and that the real GPU buffer ends
//! up holding the translated (not raw) bytes -- the same "byte-identical to
//! what a shader/consumer wants" proof `LightGpuData`
//! (`helio_component`, hand-written) and `scene_store_delegation.rs`
//! (`#[engine_class(scene_store, ...)]`, a different mechanism entirely)
//! each make for their own mechanism.

use engine_class_derive::{engine_class, register_runtime_behavior, register_world_component};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, EngineClass as _, RuntimeComponentOwner,
};
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};
use pulsar_scenedb::World;
use pulsar_world_registry::{GpuListMirrored, GpuMirrored, NoGpuMirror};
use std::sync::Arc;

fn test_context() -> EngineGpuContext {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter — GPU tests need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("engine-class-gpu-mirror-derive-test"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
}

fn readback(ctx: &EngineGpuContext, buf: &wgpu::Buffer, src_offset: u64, bytes: u64) -> Vec<u8> {
    let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buf, src_offset, &staging, 0, bytes);
    ctx.queue().submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    ctx.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = slice.get_mapped_range().expect("mapped range").to_vec();
    staging.unmap();
    data
}

fn scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 1 }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    }
}

/// A plain, fieldless enum -- the `AsU32` packing case. `#[repr(u32)]` isn't
/// required for `self.field as u32` to compile (any fieldless enum casts to
/// an integer), but pins the discriminant values this test asserts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u32)]
pub enum ThrowawayKind {
    #[default]
    Alpha = 0,
    Beta = 1,
    Gamma = 2,
}

/// A `#[sub_props]` group with a mix of `#[gpu]` and plain fields --
/// exercises `Direct` ([f32; 4]), `AsU32` (bool, enum), and confirms a
/// non-`#[gpu]` field (including a `String`, which `#[gpu]` could never
/// support) is simply excluded from the mirror, not an error, since it was
/// never marked `#[gpu]` in the first place.
#[engine_class(category = "Test", default, clone, debug, serialize, deserialize, no_register)]
pub struct ThrowawaySubProps {
    #[property]
    #[gpu]
    pub enabled: bool,
    // Deliberately NOT `#[property]` -- `pulsar_reflection::Reflectable`
    // (needed for #[property]'s editor-facing type metadata) isn't
    // implemented for this throwaway enum, and registering it isn't this
    // test's concern. `#[gpu]` doesn't require `#[property]` at all (they're
    // independent field-level opt-ins -- see `derive_engine_class`'s field
    // loop): this field is GPU-mirrored but not properties-panel-visible.
    #[gpu]
    pub kind: ThrowawayKind,
    #[property]
    #[gpu]
    pub color: [f32; 4],
    #[property]
    pub label: String,
}

/// The containing struct: one direct `#[gpu]` leaf field PLUS a
/// `#[sub_props]` field -- proves composition (the sub-props group's own
/// mirror is embedded, not re-derived here).
#[engine_class(category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayMirroredComponent {
    #[sub_props]
    pub sub: ThrowawaySubProps,
    #[property]
    #[gpu]
    pub intensity: f32,
}

/// No `#[gpu]` fields anywhere -- must get `GpuMirror = NoGpuMirror`, not a
/// generated (empty) struct of its own.
#[engine_class(category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayUnmirroredComponent {
    #[property]
    pub label: f32,
}

#[test]
fn bool_and_enum_fields_pack_as_u32_arrays_pack_as_is() {
    let sub = ThrowawaySubProps {
        enabled: true,
        kind: ThrowawayKind::Beta,
        color: [0.1, 0.2, 0.3, 0.4],
        label: "unused, not #[gpu]".to_string(),
    };
    let mirror = sub.to_gpu_mirror();
    assert_eq!(mirror.enabled, 1, "bool true must pack as u32 1");
    assert_eq!(mirror.kind, ThrowawayKind::Beta as u32, "enum must pack as its u32 discriminant");
    assert_eq!(mirror.color, [0.1, 0.2, 0.3, 0.4], "a Pod array field must pack unchanged");

    let sub_off = ThrowawaySubProps { enabled: false, ..sub };
    assert_eq!(sub_off.to_gpu_mirror().enabled, 0, "bool false must pack as u32 0");
}

#[test]
fn sub_props_composition_embeds_the_nested_mirror() {
    let value = ThrowawayMirroredComponent {
        sub: ThrowawaySubProps {
            enabled: true,
            kind: ThrowawayKind::Gamma,
            color: [1.0, 2.0, 3.0, 4.0],
            label: String::new(),
        },
        intensity: 99.5,
    };
    let mirror = value.to_gpu_mirror();
    assert_eq!(mirror.intensity, 99.5);
    assert_eq!(mirror.sub.enabled, 1);
    assert_eq!(mirror.sub.kind, ThrowawayKind::Gamma as u32);
    assert_eq!(mirror.sub.color, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn a_struct_with_no_gpu_fields_gets_no_gpu_mirror() {
    let value = ThrowawayUnmirroredComponent { label: 1.0 };
    // Type-level: if this didn't hold, the line below wouldn't compile.
    let _mirror: NoGpuMirror = value.to_gpu_mirror();
    assert_eq!(value.to_gpu_mirror(), NoGpuMirror);
}

#[test]
fn reflection_properties_are_unaffected_by_gpu_mirroring() {
    // The whole point, same as scene_store_delegation.rs's equivalent test
    // for the OTHER mechanism: the properties panel and the GPU mirror read
    // the SAME struct, and a non-#[gpu] property (`label`, a String -- a
    // type #[gpu] could never support) still shows up normally.
    let value = ThrowawaySubProps {
        enabled: true,
        kind: ThrowawayKind::Alpha,
        color: [0.0; 4],
        label: "hello".to_string(),
    };
    let props = value.get_properties();
    assert!(props.iter().any(|p| p.name == "label"));
    assert!(props.iter().any(|p| p.name == "enabled"));
}

#[test]
fn gpu_mirror_lands_on_the_real_gpu_through_sync_gpu_mirror() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(Arc::clone(&store), Arc::clone(ctx.queue())));

    let entity = world.spawn();
    let value = ThrowawayMirroredComponent {
        sub: ThrowawaySubProps {
            enabled: true,
            kind: ThrowawayKind::Beta,
            color: [5.0, 6.0, 7.0, 8.0],
            label: String::new(),
        },
        intensity: 42.0,
    };
    // `sync_gpu_mirror` is `GpuMirrored`'s own trait-default -- not
    // something `#[engine_class]` had to generate a per-type override of
    // (see that trait's doc). This is the exact call `#[register_world_
    // component(gpu_mirror)]`'s generated hydrate makes.
    value.sync_gpu_mirror(&mut world, entity);
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");

    // Decoded at MANUALLY computed offsets, not via a whole-struct
    // readback_row::<Mirror>() reinterpret -- same reasoning
    // `gpu_packed_layout.rs`'s own upstream test uses: the packed buffer's
    // per-#[gpu]-field byte offsets are assigned by the derive (in
    // generated-field declaration order: `intensity` leaf field first, then
    // the composed `sub` field), which Rust's own (unspecified, no
    // `#[repr(C)]`) struct layout for `Mirror` has no obligation to match
    // byte-for-byte -- a raw reinterpret cast is only safe when the two
    // happen to agree, which isn't guaranteed here the way it trivially was
    // for `LightGpuData`'s single-field case.
    type Mirror = <ThrowawayMirroredComponent as GpuMirrored>::GpuMirror;
    const INTENSITY_OFFSET: u64 = 0;
    const SUB_ENABLED_OFFSET: u64 = 4;
    const SUB_KIND_OFFSET: u64 = 8;
    const SUB_COLOR_OFFSET: u64 = 12;
    const PACKED_ROW_BYTES: u64 = 4 + (4 + 4 + 16); // intensity + sub(enabled, kind, color)

    let id = Mirror::packed_gpu_component_id();
    let handle = store.resolve_buffer_handle(store.buffer_key_for(id).expect("registered")).expect("resolvable");
    let row_start = (entity.index() as u64) * PACKED_ROW_BYTES;
    let bytes = readback(&ctx, &handle.buffer, row_start, PACKED_ROW_BYTES);

    let f32_at = |off: u64| f32::from_ne_bytes(bytes[off as usize..off as usize + 4].try_into().unwrap());
    let u32_at = |off: u64| u32::from_ne_bytes(bytes[off as usize..off as usize + 4].try_into().unwrap());

    assert_eq!(f32_at(INTENSITY_OFFSET), 42.0);
    assert_eq!(u32_at(SUB_ENABLED_OFFSET), 1);
    assert_eq!(u32_at(SUB_KIND_OFFSET), ThrowawayKind::Beta as u32);
    let color: [f32; 4] = std::array::from_fn(|i| f32_at(SUB_COLOR_OFFSET + i as u64 * 4));
    assert_eq!(color, [5.0, 6.0, 7.0, 8.0]);

    // Removal must drop it (GpuMirrored::remove_gpu_mirror -- the other
    // trait default, and what #[register_world_component(gpu_mirror)]'s
    // generated `remove` calls).
    ThrowawayMirroredComponent::remove_gpu_mirror(&mut world, entity);
    assert!(world.get::<Mirror>(entity).is_none());
}

/// A real `#[register_world_component(gpu_mirror)]` class -- unlike the
/// tests above (which call `GpuMirrored`'s methods directly), this proves
/// the full pipeline the flag actually wires up: `hydrate_world_component_
/// for_class` (JSON in) auto-inserts the mirror, `remove_world_component_
/// for_class` auto-drops it, with zero hand-written hydrate/remove
/// anywhere in this component's own definition.
#[engine_class(category = "Test", default, clone, debug, serialize, deserialize, no_register)]
pub struct ThrowawayRegisteredComponent {
    #[property]
    #[gpu]
    pub value: f32,
}

#[register_world_component(gpu_mirror)]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for ThrowawayRegisteredComponent {
    const CLASS_NAME: &'static str = "ThrowawayRegisteredComponent";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component: &Self,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
    }
}

#[test]
fn register_world_component_gpu_mirror_flag_auto_syncs_through_hydrate_and_remove() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(Arc::clone(&store), Arc::clone(ctx.queue())));
    let entity = world.spawn();

    pulsar_world_registry::hydrate_world_component_for_class(
        "ThrowawayRegisteredComponent",
        &mut world,
        entity,
        &serde_json::json!({ "value": 13.0 }),
    )
    .unwrap();

    type Mirror = <ThrowawayRegisteredComponent as GpuMirrored>::GpuMirror;
    assert_eq!(
        world.get::<Mirror>(entity).map(|m| m.value),
        Some(13.0),
        "hydrate must have auto-inserted the mirror with no hand-written hydrate fn"
    );

    assert!(pulsar_world_registry::remove_world_component_for_class(
        "ThrowawayRegisteredComponent",
        &mut world,
        entity,
    ));
    assert!(
        world.get::<Mirror>(entity).is_none(),
        "remove must have auto-dropped the mirror too, with no hand-written remove fn"
    );
}

// ── GpuListMirrored: the SEPARATE var-len companion for #[gpu] Vec<T> ──────

/// An already-Pod custom record type -- the same shape `StaticMeshComponent
/// ::vertices`'s `PackedVertex` is, standing in for it here so this test
/// doesn't need a real GPU-mesh dependency. Exercises `classify_gpu_list_
/// elem_type`'s "unrecognized element type -> Direct pass-through" rule
/// (NOT the scalar case's "-> AsU32" one -- casting a multi-field struct
/// `as u32` would be a compile error, which is exactly the regression this
/// type's presence in the test guards against).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThrowawayVertex {
    pub position: [f32; 3],
    pub id: u32,
}
unsafe impl pulsar_scenedb::Pod for ThrowawayVertex {}

/// One `Vec<bool>` (the `AsU32` element case) and one `Vec<ThrowawayVertex>`
/// (the `Direct`/pass-through element case) -- both `#[gpu]`, neither
/// `#[property]` (properties-panel visibility isn't this mechanism's
/// concern, same as `ThrowawayMirroredComponent::kind` above).
#[engine_class(category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayListComponent {
    #[gpu]
    pub flags: Vec<bool>,
    #[gpu]
    pub vertices: Vec<ThrowawayVertex>,
}

#[test]
fn vec_bool_converts_to_vec_u32_vec_of_pod_struct_passes_through() {
    let value = ThrowawayListComponent {
        flags: vec![true, false, true],
        vertices: vec![
            ThrowawayVertex { position: [1.0, 2.0, 3.0], id: 7 },
            ThrowawayVertex { position: [4.0, 5.0, 6.0], id: 8 },
        ],
    };
    let mirror = value.to_gpu_list_mirror();
    assert_eq!(mirror.flags, vec![1u32, 0, 1]);
    assert_eq!(mirror.vertices, value.vertices, "an already-Pod element type must pass through unchanged");
}

#[test]
fn a_struct_with_no_gpu_vec_fields_gets_no_gpu_mirror_for_its_list_half() {
    // ThrowawayUnmirroredComponent (defined above) has zero #[gpu] fields
    // of any kind.
    let value = ThrowawayUnmirroredComponent { label: 1.0 };
    let _mirror: NoGpuMirror = value.to_gpu_list_mirror();
}

#[test]
fn gpu_list_mirror_lands_on_the_real_gpu_as_a_var_len_pool() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(Arc::clone(&store), Arc::clone(ctx.queue())));

    let entity = world.spawn();
    let value = ThrowawayListComponent {
        flags: vec![true, true, false],
        vertices: vec![ThrowawayVertex { position: [9.0, 9.0, 9.0], id: 42 }],
    };
    value.sync_gpu_list_mirror(&mut world, entity);
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");

    type ListMirror = <ThrowawayListComponent as GpuListMirrored>::GpuListMirror;
    let stored = world.get::<ListMirror>(entity).expect("list mirror must be readable back from World");
    assert_eq!(stored.flags, vec![1, 1, 0]);
    assert_eq!(stored.vertices, vec![ThrowawayVertex { position: [9.0, 9.0, 9.0], id: 42 }]);

    // The `vertices` field's var-len pool is real GPU storage (same
    // zero-manual-registration guarantee `LightGpuData`'s own mirror test
    // makes for the packed/fixed half) -- same derive-assigned buffer-key
    // naming convention (`"{Struct}::{field}"`) `static_mesh_component_
    // gpu_mirror.rs` reads back through for its own var-len fields.
    let pool = store
        .var_len_pool::<ThrowawayVertex>(pulsar_scenedb::gpu::BufferKey::of("ThrowawayListComponentGpuListMirror::vertices"))
        .expect("vertices pool must be registered by the var-len path -- proves this is real GPU storage, not just a CPU-side Vec");
    assert!(pool.capacity() > 0);

    ThrowawayListComponent::remove_gpu_list_mirror(&mut world, entity);
    assert!(world.get::<ListMirror>(entity).is_none());
}
