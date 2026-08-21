//! Proves `#[gpu]` on a `#[property]` field (Pulsar-Native#561's
//! auto-derived GPU mirroring, `gpu_mirror_codegen` in `src/lib.rs`) works
//! end to end, in isolation, on throwaway test types -- before any real
//! primitive (reflection capture, water volume, ...) is pointed at it.
//!
//! Covers the universal `pulsar_world_registry::GpuRepr<T>` wrapping (ANY
//! `Copy` type mirrors as its own exact bytes -- no classification, no
//! bool/enum-to-u32 conversion, see that type's own doc for why), `#[sub_
//! props]` composition (a containing struct's mirror embeds its sub-props
//! groups' own independently-generated mirrors), the `NoGpuMirror` case for
//! a struct with no `#[gpu]` fields at all, and that the real GPU buffer
//! ends up holding the actual bytes -- the same "byte-identical, no
//! translation-layer duplication" proof `LightGpuData` (`helio_component`,
//! hand-written) and `scene_store_delegation.rs` (`#[engine_class(
//! scene_store, ...)]`, a different mechanism entirely) each make for their
//! own mechanism.

use engine_class_derive::{engine_class, register_runtime_behavior, register_world_component};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, EngineClass as _, Reflectable, RuntimeComponentOwner,
};
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuColumnSet, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};
use pulsar_scenedb::World;
use pulsar_world_registry::{GpuHeavy, GpuHeavyMirrored, GpuListMirrored, GpuMirrored, GpuRepr, NoGpuMirror};
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

/// A plain, fieldless enum. `#[repr(u32)]` isn't required for `GpuRepr<T>`
/// to work (it only needs `T: Copy`), but pins this enum's own byte size/
/// layout to something this test can assert on deterministically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    assert_eq!(mirror.enabled, GpuRepr(true), "bool must mirror as itself, not a u32 cast");
    assert_eq!(mirror.kind, GpuRepr(ThrowawayKind::Beta), "enum must mirror as itself, not its discriminant cast to u32");
    assert_eq!(mirror.color, GpuRepr([0.1, 0.2, 0.3, 0.4]), "a Pod array field must pack unchanged");

    let sub_off = ThrowawaySubProps { enabled: false, ..sub };
    assert_eq!(sub_off.to_gpu_mirror().enabled, GpuRepr(false));
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
    // `enabled` (bool, 1 byte, itself) still occupies a 4-byte-aligned slot
    // in the packed struct (repr(C) pads a lone leading bool up to the next
    // field's 4-byte alignment) -- only byte 0 of that slot is meaningful.
    let bool_at = |off: u64| bytes[off as usize] != 0;
    let u32_at = |off: u64| u32::from_ne_bytes(bytes[off as usize..off as usize + 4].try_into().unwrap());

    assert_eq!(f32_at(INTENSITY_OFFSET), 42.0);
    assert!(bool_at(SUB_ENABLED_OFFSET));
    assert_eq!(u32_at(SUB_KIND_OFFSET), ThrowawayKind::Beta as u32, "the enum's own #[repr(u32)] byte layout, read raw -- not a semantic cast");
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
        Some(GpuRepr(13.0)),
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
/// doesn't need a real GPU-mesh dependency. Proves a `Vec<T>` of an
/// arbitrary multi-field `Copy` struct works exactly the same way a
/// `Vec<bool>` does -- one wrapping rule, no special-casing either shape.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThrowawayVertex {
    pub position: [f32; 3],
    pub id: u32,
}

#[engine_class(category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayListComponent {
    #[gpu]
    pub flags: Vec<bool>,
    #[gpu]
    pub vertices: Vec<ThrowawayVertex>,
}

#[test]
fn vec_fields_mirror_element_by_element_as_their_own_exact_bytes() {
    let value = ThrowawayListComponent {
        flags: vec![true, false, true],
        vertices: vec![
            ThrowawayVertex { position: [1.0, 2.0, 3.0], id: 7 },
            ThrowawayVertex { position: [4.0, 5.0, 6.0], id: 8 },
        ],
    };
    let mirror = value.to_gpu_list_mirror();
    assert_eq!(mirror.flags, vec![GpuRepr(true), GpuRepr(false), GpuRepr(true)]);
    assert_eq!(
        mirror.vertices,
        value.vertices.iter().copied().map(GpuRepr).collect::<Vec<_>>(),
        "an arbitrary Copy element type must pass through element-by-element unchanged"
    );
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
    assert_eq!(stored.flags, vec![GpuRepr(true), GpuRepr(true), GpuRepr(false)]);
    assert_eq!(stored.vertices, vec![GpuRepr(ThrowawayVertex { position: [9.0, 9.0, 9.0], id: 42 })]);

    // The `vertices` field's var-len pool is real GPU storage (same
    // zero-manual-registration guarantee `LightGpuData`'s own mirror test
    // makes for the packed/fixed half) -- same derive-assigned buffer-key
    // naming convention (`"{Struct}::{field}"`) `static_mesh_component_
    // gpu_mirror.rs` reads back through for its own var-len fields. The
    // pool's element type is `GpuRepr<ThrowawayVertex>` (the wrapper this
    // whole mechanism mirrors through), not `ThrowawayVertex` directly.
    let pool = store
        .var_len_pool::<GpuRepr<ThrowawayVertex>>(pulsar_scenedb::gpu::BufferKey::of("ThrowawayListComponentGpuListMirror::vertices"))
        .expect("vertices pool must be registered by the var-len path -- proves this is real GPU storage, not just a CPU-side Vec");
    assert!(pool.capacity() > 0);

    ThrowawayListComponent::remove_gpu_list_mirror(&mut world, entity);
    assert!(world.get::<ListMirror>(entity).is_none());
}

// ── Deliberately awkward element shapes ─────────────────────────────────
//
// A real `Vec<Vec<T>>` `#[gpu]` field can never compile: `Vec` isn't `Copy`,
// so it fails `GpuRepr<T>`'s `T: Copy` bound before any codegen even runs --
// that's Rust's own rule (heap-recursive data has no fixed byte layout to
// mirror), not a gap in this mechanism. The honest GPU-representable
// equivalent of "nested/jagged data" is a `Vec<T>` where `T` is itself a
// fixed, possibly array-of-arrays-shaped `Copy` record. `ThrowawayNastyElement`
// below deliberately combines several awkward properties at once: a leading
// `bool`, a nested 2D array field (`[[u8; 3]; 2]`), a `u16` tail, and a
// TOTAL size (10 bytes) that is neither a multiple of 4 nor evenly divides
// it -- exactly the shape `elem_align`/`write_padded` (SceneDB's
// `dynamic_buffer.rs`) exist to survive, reached here through the real
// derive-generated pipeline rather than SceneDB's own unit tests.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThrowawayNastyElement {
    pub tag: bool,
    pub rows: [[u8; 3]; 2],
    pub id: u16,
}

#[engine_class(category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayNastyListComponent {
    #[gpu]
    pub elements: Vec<ThrowawayNastyElement>,
}

#[test]
fn a_nested_array_of_arrays_element_type_mirrors_correctly_at_every_odd_count() {
    assert_eq!(
        std::mem::size_of::<ThrowawayNastyElement>(),
        10,
        "this test's whole point depends on a 10-byte element -- neither a multiple of 4 nor a divisor of it"
    );

    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));
    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(Arc::clone(&store), Arc::clone(ctx.queue())));

    let make = |n: u16| -> Vec<ThrowawayNastyElement> {
        (0..n)
            .map(|i| ThrowawayNastyElement {
                tag: i % 2 == 0,
                rows: [[i as u8, i as u8 + 1, i as u8 + 2], [i as u8 + 3, i as u8 + 4, i as u8 + 5]],
                id: i,
            })
            .collect()
    };

    // Several entities, each resized across several rounds with COUNTS that
    // don't share a common factor with each other or with 4 -- if the
    // GPU-write padding this depends on ever escaped its own reserved span,
    // one entity's data would corrupt a neighbor's.
    let entities: Vec<_> = (0..3).map(|_| world.spawn()).collect();
    for round in 0..5u16 {
        for (i, &e) in entities.iter().enumerate() {
            let count = 1 + (round * 3 + i as u16 * 5) % 7; // walks 1..=7, non-monotonically
            let value = ThrowawayNastyListComponent { elements: make(count) };
            value.sync_gpu_list_mirror(&mut world, e);
        }
        world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");

        type ListMirror = <ThrowawayNastyListComponent as GpuListMirrored>::GpuListMirror;
        for (i, &e) in entities.iter().enumerate() {
            let count = 1 + (round * 3 + i as u16 * 5) % 7;
            let expected: Vec<GpuRepr<ThrowawayNastyElement>> = make(count).into_iter().map(GpuRepr).collect();
            let stored = world.get::<ListMirror>(e).expect("list mirror must exist every round");
            assert_eq!(stored.elements, expected, "round {round}, entity {i} (count {count}) — a neighbor's write likely clobbered this one");
        }
    }
}

// ── GpuHeavyMirrored: the handle/heavy-element split, reached via a type ──
// ── in the field's own signature instead of #[gpu(heavy)] arguments ──────

/// The heavy, GPU-resident payload a `ThrowawayMeshHandle` maps to --
/// deliberately shaped nothing like the handle (bigger, different fields)
/// to make the split obvious in the test's own assertions.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThrowawayMeshGpuData {
    pub triangle_count: u32,
    pub bounds: [f32; 4],
}
unsafe impl pulsar_scenedb::page::Pod for ThrowawayMeshGpuData {}

/// A lightweight CPU-side handle (an asset ID, in spirit) -- `Reflectable`
/// so it can still be a normal `#[property]` (delegated through, via
/// `GpuHeavy<T>`'s own impl), and `Pod` so it can live in the generated
/// heavy-mirror's fixed GPU column alongside every other handle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThrowawayMeshHandle {
    pub id: f32,
}
unsafe impl pulsar_scenedb::page::Pod for ThrowawayMeshHandle {}

// Hand-written rather than `#[derive(Reflectable)]`: the derive builds each
// type's `RuntimeTypeInfo` as a `static`, which requires every field's own
// `type_info()` call to be const-evaluable -- not yet true of `Reflectable`
// on stable Rust (`Reflectable::type_info` isn't a `const fn`, so even a
// single bare `f32` field hits this). Delegating to `f32`'s own info is a
// fine stand-in for this throwaway handle -- exactly the same "reflect as
// if you were T" delegation `GpuHeavy<T>`'s own impl does one level up.
impl Reflectable for ThrowawayMeshHandle {
    fn type_info() -> &'static pulsar_reflection::RuntimeTypeInfo
    where
        Self: Sized,
    {
        f32::type_info()
    }
    fn serialize(&self, serializer: &mut dyn pulsar_reflection::TypeSerializer) -> pulsar_reflection::ReflectResult<()> {
        self.id.serialize(serializer)
    }
    fn deserialize(deserializer: &mut dyn pulsar_reflection::TypeDeserializer) -> pulsar_reflection::ReflectResult<Self>
    where
        Self: Sized,
    {
        Ok(Self { id: f32::deserialize(deserializer)? })
    }
    fn clone_any(&self) -> Box<dyn std::any::Any> {
        Box::new(*self)
    }
}

impl pulsar_scenedb::gpu::GpuUploadSource for ThrowawayMeshHandle {
    type Element = ThrowawayMeshGpuData;
    fn upload_element(&self) -> Self::Element {
        // A handle-derived, deterministic "mesh" -- stands in for a real
        // asset-registry lookup, which this mechanism doesn't care about
        // the shape of.
        ThrowawayMeshGpuData { triangle_count: self.id as u32 * 3, bounds: [self.id; 4] }
    }
}

#[engine_class(category = "Test", default, clone, debug, no_register)]
pub struct ThrowawayHeavyComponent {
    #[property]
    #[gpu]
    pub mesh: GpuHeavy<ThrowawayMeshHandle>,
}

#[test]
fn gpu_heavy_field_reflects_and_mirrors_the_handle_not_the_element() {
    // The properties panel/reflection side: GpuHeavy<T> delegates straight
    // to T (this test's confirmed choice for how GpuHeavy<T> reflection
    // should behave) -- the field is visible, cloneable, and its type_info
    // is T's own.
    let value = ThrowawayHeavyComponent { mesh: GpuHeavy(ThrowawayMeshHandle { id: 7.0 }) };
    let props = value.get_properties();
    let mesh_prop = props.iter().find(|p| p.name == "mesh").expect("mesh must still be a property");
    assert_eq!(
        mesh_prop.type_info as *const _,
        ThrowawayMeshHandle::type_info() as *const _,
        "GpuHeavy<T>::type_info() must delegate to T's own registered info"
    );

    // The handle mirror holds the HANDLE, unwrapped -- not the (much
    // larger, upload-computed) Element.
    let mirror = value.to_gpu_heavy_mirror();
    assert_eq!(mirror.mesh, ThrowawayMeshHandle { id: 7.0 });
}

#[test]
fn gpu_heavy_field_lands_the_handle_and_the_uploaded_element_in_separate_real_gpu_buffers() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));
    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(Arc::clone(&store), Arc::clone(ctx.queue())));

    let entity = world.spawn();
    let value = ThrowawayHeavyComponent { mesh: GpuHeavy(ThrowawayMeshHandle { id: 5.0 }) };
    value.sync_gpu_heavy_mirror(&mut world, entity);
    world.flush_gpu_mirror(ctx.queue()).expect("mirror attached");

    type Mirror = <ThrowawayHeavyComponent as GpuHeavyMirrored>::GpuHeavyMirror;

    // CPU side: World holds the handle, instantly, no VRAM round trip.
    let stored = world.get::<Mirror>(entity).expect("heavy mirror must exist");
    assert_eq!(stored.mesh, ThrowawayMeshHandle { id: 5.0 });

    // GPU side: the buffer is real, and sized to the ELEMENT (16 bytes),
    // never the 4-byte handle -- proving `write_gpu_columns_at_row`'s
    // handle -> Element upload mapper actually ran, not just a raw copy of
    // the handle's own bytes.
    let column = Mirror::gpu_columns().into_iter().next().expect("exactly one #[gpu] column");
    let id = column.field_token.id();
    let handle = store.resolve_buffer_handle(store.buffer_key_for(id).expect("registered")).expect("resolvable");
    let bytes = readback(&ctx, &handle.buffer, (entity.index() as u64) * 20, 20);

    let triangle_count = u32::from_ne_bytes(bytes[0..4].try_into().unwrap());
    let bounds: [f32; 4] = std::array::from_fn(|i| f32::from_ne_bytes(bytes[4 + i * 4..8 + i * 4].try_into().unwrap()));
    assert_eq!(triangle_count, 15, "handle.id (5) * 3, from GpuUploadSource::upload_element -- not the raw handle bytes");
    assert_eq!(bounds, [5.0; 4]);

    ThrowawayHeavyComponent::remove_gpu_heavy_mirror(&mut world, entity);
    assert!(world.get::<Mirror>(entity).is_none());
}
