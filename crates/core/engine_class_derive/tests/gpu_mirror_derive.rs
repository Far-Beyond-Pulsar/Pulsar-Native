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

use engine_class_derive::{engine_class, register_runtime_behavior, register_world_component};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, EngineClass as _, Reflectable, RuntimeComponentOwner,
};
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuColumnSet, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};
use pulsar_scenedb::World;
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

#[engine_class(category = "Test", default, clone, debug, no_register)]
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
    let value = ThrowawayOverrideComponent { angle_degrees: 180.0, kind: ThrowawayKind::Gamma };
    let mirror = value.to_gpu_mirror();

    // Field TYPE changed too, not just the value -- `angle_degrees` mirrors
    // as an f32 (matches `as = f32`), `kind` mirrors as a u32 (matches
    // `as = u32`), neither as their own source type.
    let angle_radians: GpuRepr<f32> = mirror.angle_degrees;
    assert!((angle_radians.0 - std::f32::consts::PI).abs() < 1e-6, "180 degrees must mirror as pi radians, computed by `with`, not stored as 180.0");

    let kind_u32: GpuRepr<u32> = mirror.kind;
    assert_eq!(kind_u32, GpuRepr(300), "must go through throwaway_kind_to_u32, not a raw discriminant cast");
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
    // happen to agree, which isn't guaranteed here the way it trivially is
    // for a single-field wrapper.
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

/// Pulsar-Native#561 (properties-panel live-edit bug): `hydrate` only ever
/// runs once, at JSON-hydrate time -- a live edit through `get_mut`/
/// `get_world_component_as_engine_class_mut` (the properties panel's real
/// write path, no re-hydrate involved) must still reach the mirror when
/// `refresh_world_component_gpu_mirror_for_class` is called, with zero
/// hand-written sync code anywhere in `ThrowawayRegisteredComponent`'s own
/// definition -- the bare `gpu_mirror` flag's default `refresh_gpu_mirror`
/// body is what's under test here.
#[test]
fn refresh_gpu_mirror_for_class_picks_up_a_live_edit_hydrate_never_saw() {
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
    assert_eq!(world.get::<Mirror>(entity).map(|m| m.value), Some(GpuRepr(13.0)));

    // The live-edit path: mutate the real `World`-resident value directly,
    // no JSON, no re-hydrate -- exactly what `update_live_component_property`
    // (the actual properties-panel write path, `ui_level_editor`) does.
    world.get_mut::<ThrowawayRegisteredComponent>(entity).unwrap().value = 42.0;

    // Before the refresh call: this is the bug as originally reported --
    // the live value changed, but the mirror is still whatever hydrate saw.
    assert_eq!(
        world.get::<Mirror>(entity).map(|m| m.value),
        Some(GpuRepr(13.0)),
        "sanity: a plain live edit must NOT auto-propagate to the mirror by itself"
    );

    assert!(pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(
        "ThrowawayRegisteredComponent",
        &mut world,
        entity,
    ));
    assert_eq!(
        world.get::<Mirror>(entity).map(|m| m.value),
        Some(GpuRepr(42.0)),
        "refresh_world_component_gpu_mirror_for_class must re-derive the mirror from the CURRENT live value"
    );
}

/// A class whose mirror's presence is conditional on its own data (the
/// `LightComponent` "disabled means absent" shape) -- proves `refresh_
/// gpu_mirror = path` overrides the bare flag's unconditional default, and
/// that the override sees live edits the same way the default does.
#[engine_class(category = "Test", default, clone, debug, serialize, deserialize, no_register)]
pub struct ThrowawayConditionalMirrorComponent {
    #[property]
    pub enabled: bool,
    #[property]
    #[gpu]
    pub value: f32,
}

fn throwaway_conditional_refresh(
    world: &mut pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
) {
    let Some(enabled) = world.get::<ThrowawayConditionalMirrorComponent>(entity).map(|c| c.enabled)
    else {
        return;
    };
    if enabled {
        let mirror = world
            .get::<ThrowawayConditionalMirrorComponent>(entity)
            .map(GpuMirrored::to_gpu_mirror);
        if let Some(mirror) = mirror {
            world.insert(entity, mirror);
        }
    } else {
        ThrowawayConditionalMirrorComponent::remove_gpu_mirror(world, entity);
    }
}

#[register_world_component(refresh_gpu_mirror = throwaway_conditional_refresh)]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for ThrowawayConditionalMirrorComponent {
    const CLASS_NAME: &'static str = "ThrowawayConditionalMirrorComponent";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component: &Self,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
    }
}

#[test]
fn refresh_gpu_mirror_override_replaces_the_default_and_still_sees_live_edits() {
    let ctx = test_context();
    let store = Arc::new(SceneGpuStore::new(&ctx, scene_cfg()));

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(Arc::clone(&store), Arc::clone(ctx.queue())));
    let entity = world.spawn();
    world.insert(entity, ThrowawayConditionalMirrorComponent { enabled: true, value: 7.0 });

    type Mirror = <ThrowawayConditionalMirrorComponent as GpuMirrored>::GpuMirror;
    assert!(
        world.get::<Mirror>(entity).is_none(),
        "a plain World::insert (no hydrate) must not have a mirror yet"
    );

    assert!(pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(
        "ThrowawayConditionalMirrorComponent",
        &mut world,
        entity,
    ));
    assert_eq!(world.get::<Mirror>(entity).map(|m| m.value), Some(GpuRepr(7.0)));

    // Live edit, then refresh again -- the override must see it, same as the default.
    world.get_mut::<ThrowawayConditionalMirrorComponent>(entity).unwrap().value = 99.0;
    pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(
        "ThrowawayConditionalMirrorComponent",
        &mut world,
        entity,
    );
    assert_eq!(world.get::<Mirror>(entity).map(|m| m.value), Some(GpuRepr(99.0)));

    // Disabling and refreshing must remove the mirror, not leave a stale one --
    // this is exactly the behavior the bare `gpu_mirror` flag's unconditional
    // default CANNOT express, which is why this override exists.
    world.get_mut::<ThrowawayConditionalMirrorComponent>(entity).unwrap().enabled = false;
    pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(
        "ThrowawayConditionalMirrorComponent",
        &mut world,
        entity,
    );
    assert!(
        world.get::<Mirror>(entity).is_none(),
        "disabling must remove the mirror, not leave the last-synced value behind"
    );
}