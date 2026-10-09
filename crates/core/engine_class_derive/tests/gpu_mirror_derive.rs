//! Proves `#[gpu]` on a `#[property]` field (Pulsar-Native#561's
//! auto-derived GPU mirroring, `gpu_mirror_codegen` in `src/lib.rs`) works
//! end to end, in isolation, on throwaway test types -- `LightComponent`
//! (`helio_component`) is the real primitive now pointed at this exact
//! mechanism (`LightComponentGpuMirror`, no hand-written companion left).
//!
//! Covers the universal `pulsar_world_registry::GpuRepr<T>` wrapping (ANY
//! `Copy` type mirrors as its own exact bytes -- no classification, no
//! bool/enum-to-u32 conversion, see that type's own doc for why), `GpuHeavy
//! <T>`'s separate handle/heavy-element split, `#[sub_props]` composition (a
//! containing struct's mirror embeds its sub-props groups' own
//! independently-generated mirrors), the `NoGpuMirror` case for a struct
//! with no `#[gpu]` fields at all, and that the real GPU buffer ends up
//! holding the actual bytes -- the same "byte-identical, no translation-
//! layer duplication" proof `LightComponentGpuMirror`'s own mirror test
//! (`helio_component`) and `scene_store_delegation.rs` (`#[engine_class(
//! scene_store, ...)]`, a different mechanism entirely) each make for their
//! own mechanism.

use engine_class_derive::{engine_class, register_world_component};
use pulsar_reflection::{EngineClass as _, Reflectable};
use pulsar_scenedb::World;
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuColumnSet, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig,
    SceneGpuStore,
};
use pulsar_world_registry::{GpuMirrored, GpuRepr, NoGpuMirror};
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
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().expect("mapped range").to_vec();
    staging.unmap();
    data
}

fn scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig {
            capacity: 64,
            max_resident_cells: 1,
        }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    }
}

/// A plain, fieldless enum. `#[repr(u32)]` isn't required for `GpuRepr<T>`
/// to work (it only needs `T: Copy`), but pins this enum's own byte size/
/// layout to something this test can assert on deterministically.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, Reflectable,
)]
#[repr(u32)]
pub enum ThrowawayKind {
    #[default]
    Alpha = 0,
    Beta = 1,
    Gamma = 2,
}

/// A `#[sub_props]` group with a mix of `#[gpu]` and plain fields --
/// exercises a `bool`, a plain enum, and a `[f32; 4]` all mirroring via the
/// SAME `GpuRepr<T>` wrapping (no per-shape conversion happening anywhere),
/// and confirms a non-`#[gpu]` field (`label`, a `String` -- never `Copy`,
/// could never work here) is simply excluded from the mirror, not an
/// error, since it was never marked `#[gpu]` in the first place.
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
pub struct ThrowawaySubProps {
    #[property]
    #[gpu]
    pub enabled: bool,
    // This field is GPU-mirrored but not properties-panel-visible. The enum
    // still implements Reflectable because SceneDB requires it for GPU rows.
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
#[engine_class(gpu_rows, category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayMirroredComponent {
    #[sub_props]
    pub sub: ThrowawaySubProps,
    #[property]
    #[gpu]
    pub intensity: f32,
}

/// No `#[gpu]` fields anywhere -- must get `GpuMirror = NoGpuMirror`, not a
/// generated (empty) struct of its own.
#[engine_class(gpu_rows, category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayUnmirroredComponent {
    #[property]
    pub label: f32,
}

#[test]
fn bool_and_enum_and_array_fields_all_mirror_as_their_own_exact_bytes() {
    let sub = ThrowawaySubProps {
        enabled: true,
        kind: ThrowawayKind::Beta,
        color: [0.1, 0.2, 0.3, 0.4],
        label: "unused, not #[gpu]".to_string(),
    };
    let mirror = sub.to_gpu_mirror();
    // No conversion anywhere: `enabled` stays a `bool` (1 byte, whatever
    // `true`'s own bit pattern is), `kind` stays the enum itself (whatever
    // bytes ITS OWN #[repr] gives it -- GpuRepr never inspects or casts).
    assert_eq!(
        mirror.enabled,
        GpuRepr(true),
        "bool must mirror as itself, not a u32 cast"
    );
    assert_eq!(
        mirror.kind,
        GpuRepr(ThrowawayKind::Beta),
        "enum must mirror as itself, not its discriminant cast to u32"
    );
    assert_eq!(
        mirror.color,
        GpuRepr([0.1, 0.2, 0.3, 0.4]),
        "a Pod array field must pack unchanged"
    );

    let sub_off = ThrowawaySubProps {
        enabled: false,
        ..sub
    };
    assert_eq!(sub_off.to_gpu_mirror().enabled, GpuRepr(false));
}

/// A plain function (not a closure) -- `#[gpu(with = ...)]` takes a path,
/// same "no wrapper, used directly as the fn item" convention every other
/// `path`-taking macro argument in this crate already uses.
fn throwaway_kind_to_u32(kind: ThrowawayKind) -> u32 {
    match kind {
        // Deliberately NOT the discriminant order -- proves this is a real
        // semantic mapping, not just a disguised `as u32` cast.
        ThrowawayKind::Alpha => 100,
        ThrowawayKind::Beta => 200,
        ThrowawayKind::Gamma => 300,
    }
}

#[engine_class(gpu_rows, category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayOverrideComponent {
    // Upload-time unit conversion -- degrees in the properties panel,
    // radians in the mirror. `f32::to_radians` used directly as the `with`
    // path, no wrapper closure.
    #[property]
    #[gpu(as = f32, with = f32::to_radians)]
    pub angle_degrees: f32,
    // Upload-time semantic remap -- a business-logic enum with no bit-
    // pattern relationship to the u32 the GPU consumer wants.
    #[gpu(as = u32, with = throwaway_kind_to_u32)]
    pub kind: ThrowawayKind,
}

#[test]
fn gpu_as_with_computes_the_override_once_at_mirror_build_time() {
    let value = ThrowawayOverrideComponent {
        angle_degrees: 180.0,
        kind: ThrowawayKind::Gamma,
    };
    let mirror = value.to_gpu_mirror();

    // Field TYPE changed too, not just the value -- `angle_degrees` mirrors
    // as an f32 (matches `as = f32`), `kind` mirrors as a u32 (matches
    // `as = u32`), neither as their own source type.
    let angle_radians: GpuRepr<f32> = mirror.angle_degrees;
    assert!(
        (angle_radians.0 - std::f32::consts::PI).abs() < 1e-6,
        "180 degrees must mirror as pi radians, computed by `with`, not stored as 180.0"
    );

    let kind_u32: GpuRepr<u32> = mirror.kind;
    assert_eq!(
        kind_u32,
        GpuRepr(300),
        "must go through throwaway_kind_to_u32, not a raw discriminant cast"
    );
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
    assert_eq!(mirror.intensity, GpuRepr(99.5));
    assert_eq!(mirror.sub.enabled, GpuRepr(true));
    assert_eq!(mirror.sub.kind, GpuRepr(ThrowawayKind::Gamma));
    assert_eq!(mirror.sub.color, GpuRepr([1.0, 2.0, 3.0, 4.0]));
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
    // type #[gpu] could never support, it isn't Copy) still shows up
    // normally.
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
fn gpu_mirror_row_follows_a_plain_insert_and_removal() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));

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
    // The only call: an ordinary insert of the authored component. Its own
    // generated SceneDB dispatch derives and writes the companion row.
    world.insert(entity, value);
    world
        .flush_gpu_mirror(ctx.queue())
        .expect("mirror attached");

    // Decoded at MANUALLY computed offsets, not via a whole-struct
    // readback_row::<Mirror>() reinterpret -- the packed buffer's
    // per-#[gpu]-field byte offsets are assigned by the derive (in
    // generated-field declaration order: `intensity` leaf field first, then
    // the composed `sub` field).
    type Mirror = <ThrowawayMirroredComponent as GpuMirrored>::GpuMirror;
    const INTENSITY_OFFSET: u64 = 0;
    const SUB_ENABLED_OFFSET: u64 = 4;
    const SUB_KIND_OFFSET: u64 = 8;
    const SUB_COLOR_OFFSET: u64 = 12;
    const PACKED_ROW_BYTES: u64 = 4 + (4 + 4 + 16); // intensity + sub(enabled, kind, color)

    let id = Mirror::packed_gpu_component_id();
    let handle = store
        .resolve_buffer_handle(store.buffer_key_for(id).expect("the insert auto-registers it"))
        .expect("resolvable");
    let row_start = (entity.index() as u64) * PACKED_ROW_BYTES;
    let bytes = readback(&ctx, &handle.buffer, row_start, PACKED_ROW_BYTES);

    let f32_at =
        |bytes: &[u8], off: u64| f32::from_ne_bytes(bytes[off as usize..off as usize + 4].try_into().unwrap());
    // `enabled` (bool, 1 byte, itself) still occupies a 4-byte-aligned slot
    // in the packed struct -- only byte 0 of that slot is meaningful.
    let bool_at = |bytes: &[u8], off: u64| bytes[off as usize] != 0;
    let u32_at =
        |bytes: &[u8], off: u64| u32::from_ne_bytes(bytes[off as usize..off as usize + 4].try_into().unwrap());

    assert_eq!(f32_at(&bytes, INTENSITY_OFFSET), 42.0);
    assert!(bool_at(&bytes, SUB_ENABLED_OFFSET));
    assert_eq!(
        u32_at(&bytes, SUB_KIND_OFFSET),
        ThrowawayKind::Beta as u32,
        "the enum's own #[repr(u32)] byte layout, read raw -- not a semantic cast"
    );
    let color: [f32; 4] = std::array::from_fn(|i| f32_at(&bytes, SUB_COLOR_OFFSET + i as u64 * 4));
    assert_eq!(color, [5.0, 6.0, 7.0, 8.0]);
    assert!(
        world.get::<Mirror>(entity).is_none(),
        "the companion is GPU-only; nothing inserts it as a component"
    );

    // A write through the guard reaches the row with no refresh call.
    world.get_mut::<ThrowawayMirroredComponent>(entity).unwrap().intensity = 3.0;
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");
    let bytes = readback(&ctx, &handle.buffer, row_start, PACKED_ROW_BYTES);
    assert_eq!(f32_at(&bytes, INTENSITY_OFFSET), 3.0);

    // Removal clears it.
    world.remove::<ThrowawayMirroredComponent>(entity);
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");
    let bytes = readback(&ctx, &handle.buffer, row_start, PACKED_ROW_BYTES);
    assert!(bytes.iter().all(|b| *b == 0), "a removed component's row must be cleared");
}

/// A registered class with no hand-written storage code at all: proves the
/// generated factory, decoder and erased insert land the same GPU row a
/// typed insert does, and that a reflected property write reaches it.
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
pub struct ThrowawayRegisteredComponent {
    #[property]
    #[gpu]
    pub value: f32,
}

#[register_world_component]
impl ThrowawayRegisteredComponent {}

fn single_f32_row(ctx: &EngineGpuContext, store: &SceneGpuStore, id: pulsar_scenedb::ComponentId, row: u32) -> f32 {
    let handle = store
        .resolve_buffer_handle(store.buffer_key_for(id).expect("registered"))
        .expect("resolvable");
    let bytes = readback(ctx, &handle.buffer, row as u64 * 4, 4);
    f32::from_ne_bytes(bytes[..4].try_into().unwrap())
}

#[test]
fn erased_factory_decode_and_property_writes_reach_the_gpu_row() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));
    type Mirror = <ThrowawayRegisteredComponent as GpuMirrored>::GpuMirror;
    let id = Mirror::packed_gpu_component_id();

    // Typed reference.
    let typed = world.spawn();
    world.insert(typed, ThrowawayRegisteredComponent { value: 13.0 });

    // The same value through the boundary decoder + erased insert.
    let decoded = world.spawn();
    assert!(pulsar_world_registry::hydrate_world_component_for_class(
        "ThrowawayRegisteredComponent",
        &mut world,
        decoded,
        &serde_json::json!({ "value": 13.0 }),
    )
    .unwrap());

    // The generic factory, then a reflected property write.
    let factory = world.spawn();
    let value = pulsar_world_registry::new_world_component_value("ThrowawayRegisteredComponent")
        .expect("registered class has a factory");
    pulsar_world_registry::insert_world_component_value(
        "ThrowawayRegisteredComponent",
        &mut world,
        factory,
        value,
    )
    .unwrap();
    pulsar_world_registry::set_world_component_property(
        "ThrowawayRegisteredComponent",
        &mut world,
        factory,
        "value",
        Box::new(13.0_f32),
    )
    .unwrap();
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");

    for entity in [typed, decoded, factory] {
        assert_eq!(
            world.get::<ThrowawayRegisteredComponent>(entity).map(|c| c.value),
            Some(13.0)
        );
        assert_eq!(single_f32_row(&ctx, &store, id, entity.index()), 13.0);
    }

    // An erased clone is an independent value of the same class.
    let clone = pulsar_world_registry::clone_world_component_value(
        "ThrowawayRegisteredComponent",
        &world,
        typed,
    )
    .expect("live value");
    assert_eq!(
        clone.downcast_ref::<ThrowawayRegisteredComponent>().map(|c| c.value),
        Some(13.0)
    );

    // A value of another class is refused, untouched.
    let refused = pulsar_world_registry::insert_world_component_value(
        "ThrowawayRegisteredComponent",
        &mut world,
        typed,
        Box::new(ThrowawayNormalizedComponent::default()),
    );
    assert!(matches!(
        refused,
        Err(pulsar_world_registry::ComponentValueError::TypeMismatch { .. })
    ));

    // Removal through the registry clears the row.
    assert!(pulsar_world_registry::remove_world_component_for_class(
        "ThrowawayRegisteredComponent",
        &mut world,
        factory,
    ));
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");
    assert_eq!(single_f32_row(&ctx, &store, id, factory.index()), 0.0);
}

/// A class that derives one field from another: `property_written` keeps
/// `doubled` in step under the same write guard as the edit.
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
pub struct ThrowawayNormalizedComponent {
    #[property]
    pub value: f32,
    #[property]
    #[gpu]
    pub doubled: f32,
}

fn throwaway_property_written(component: &mut ThrowawayNormalizedComponent, property: Option<&str>) {
    if matches!(property, None | Some("value")) {
        component.doubled = component.value * 2.0;
    }
}

#[register_world_component(property_written = throwaway_property_written)]
impl ThrowawayNormalizedComponent {}

#[test]
fn property_written_runs_under_the_same_write_and_reaches_the_gpu_row() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));
    let entity = world.spawn();
    world.insert(entity, ThrowawayNormalizedComponent::default());
    let mut cursor = world.open_change_cursor::<ThrowawayNormalizedComponent>();

    pulsar_world_registry::set_world_component_property(
        "ThrowawayNormalizedComponent",
        &mut world,
        entity,
        "value",
        Box::new(21.0_f32),
    )
    .unwrap();
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");

    assert_eq!(
        world.get::<ThrowawayNormalizedComponent>(entity).map(|c| c.doubled),
        Some(42.0)
    );
    let mut changes = Vec::new();
    let _ = world.read_changes(&mut cursor, &mut changes);
    assert_eq!(
        changes.len(),
        1,
        "the edit and its normalization are one write"
    );
    type Mirror = <ThrowawayNormalizedComponent as GpuMirrored>::GpuMirror;
    assert_eq!(
        single_f32_row(&ctx, &store, Mirror::packed_gpu_component_id(), entity.index()),
        42.0
    );
}

/// Without `gpu_rows`, the generated companion is a CPU mapping only:
/// inserting the authored value uploads no row (Pulsar-Native#1035, Phase 4).
#[engine_class(category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayCpuOnlyComponent {
    #[property]
    #[gpu]
    pub intensity: f32,
}

#[test]
fn without_gpu_rows_the_companion_uploads_nothing() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));
    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));
    let entity = world.spawn();
    world.insert(entity, ThrowawayCpuOnlyComponent { intensity: 3.0 });
    world
        .flush_gpu_mirror(ctx.queue())
        .expect("mirror attached");

    type Mirror = <ThrowawayCpuOnlyComponent as GpuMirrored>::GpuMirror;
    assert!(store.buffer_key_for(Mirror::packed_gpu_component_id()).is_none());
    // The mapping itself is still there.
    let mirror = ThrowawayCpuOnlyComponent { intensity: 3.0 }.to_gpu_mirror();
    assert_eq!(mirror.intensity.0, 3.0);
}

/// A registered class edited through every authoring path: a nested
/// `#[sub_props]` field, a collection, and a reflected method.
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
pub struct ThrowawayEquivalenceComponent {
    #[sub_props]
    pub sub: ThrowawaySubProps,
    #[property]
    pub weights: Vec<f32>,
    #[property]
    #[gpu]
    pub total: f32,
}

fn throwaway_equivalence_written(
    component: &mut ThrowawayEquivalenceComponent,
    property: Option<&str>,
) {
    if matches!(property, None | Some("weights")) {
        component.total = component.weights.iter().sum();
    }
}

#[register_world_component(property_written = throwaway_equivalence_written)]
impl ThrowawayEquivalenceComponent {}

#[pulsar_scenedb::component_methods]
impl ThrowawayEquivalenceComponent {
    /// Multiply every weight by `factor`.
    #[reflect_method]
    fn scale_weights(&mut self, factor: f32) {
        for weight in &mut self.weights {
            *weight *= factor;
        }
        self.total = self.weights.iter().sum();
    }
}

/// Mutation equivalence (Pulsar-Native#1035 acceptance, #1081): a nested
/// sub-props edit, a collection edit and a reflected method call leave the
/// same authoritative value and the same GPU row as a typed insert of it,
/// with no refresh call.
#[test]
fn nested_collection_and_method_writes_match_a_typed_insert() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));
    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));
    const CLASS: &str = "ThrowawayEquivalenceComponent";

    let typed = world.spawn();
    world.insert(
        typed,
        ThrowawayEquivalenceComponent {
            sub: ThrowawaySubProps {
                enabled: true,
                kind: ThrowawayKind::Alpha,
                color: [1.0, 2.0, 3.0, 4.0],
                label: String::new(),
            },
            weights: vec![2.0, 4.0, 6.0],
            total: 12.0,
        },
    );

    let edited = world.spawn();
    world.insert(edited, ThrowawayEquivalenceComponent::default());
    // Nested: `sub.color` and `sub.enabled` by their flattened names.
    pulsar_world_registry::set_world_component_property(
        CLASS,
        &mut world,
        edited,
        "color",
        Box::new([1.0_f32, 2.0, 3.0, 4.0]),
    )
    .unwrap();
    pulsar_world_registry::set_world_component_property(
        CLASS,
        &mut world,
        edited,
        "enabled",
        Box::new(true),
    )
    .unwrap();
    // Collection: the whole list, normalized under the same write.
    pulsar_world_registry::set_world_component_property(
        CLASS,
        &mut world,
        edited,
        "weights",
        Box::new(vec![1.0_f32, 2.0, 3.0]),
    )
    .unwrap();
    assert_eq!(
        world.get::<ThrowawayEquivalenceComponent>(edited).map(|c| c.total),
        Some(6.0)
    );
    // Method: the script/Blueprint path for reflected methods.
    world
        .call_component_method(
            edited,
            pulsar_scenedb::component_id::<ThrowawayEquivalenceComponent>(),
            "scale_weights",
            &mut [Box::new(2.0_f32)],
        )
        .unwrap();
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");

    let (a, b) = (
        world.get::<ThrowawayEquivalenceComponent>(typed).unwrap(),
        world.get::<ThrowawayEquivalenceComponent>(edited).unwrap(),
    );
    assert_eq!(a.weights, b.weights);
    assert_eq!(a.total, b.total);
    assert_eq!(a.sub.color, b.sub.color);
    assert_eq!(a.sub.enabled, b.sub.enabled);

    // `total` leaf first, then `sub` (enabled, kind, color).
    const ROW_BYTES: u64 = 4 + (4 + 4 + 16);
    type Mirror = <ThrowawayEquivalenceComponent as GpuMirrored>::GpuMirror;
    let handle = store
        .resolve_buffer_handle(
            store
                .buffer_key_for(Mirror::packed_gpu_component_id())
                .expect("registered"),
        )
        .expect("resolvable");
    let row = |entity: pulsar_scenedb::Entity| {
        readback(&ctx, &handle.buffer, entity.index() as u64 * ROW_BYTES, ROW_BYTES)
    };
    // Bytes 5..8 are the padding after `enabled`'s one byte: `GpuRepr<bool>`
    // copies the value's own representation, so they carry no data.
    let meaningful = |bytes: Vec<u8>| [&bytes[..5], &bytes[8..]].concat();
    let (typed_row, edited_row) = (meaningful(row(typed)), meaningful(row(edited)));
    assert_eq!(typed_row, edited_row, "the GPU rows carry the same values");
    assert_eq!(f32::from_ne_bytes(edited_row[..4].try_into().unwrap()), 12.0);
    assert_eq!(edited_row[4], 1, "sub.enabled");
}
