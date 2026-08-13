//! Phase A verification (see `.claude/plans/eager-plotting-lecun.md`,
//! "Phase A — Retarget `engine_class_derive`'s SceneDB codegen to
//! `World`/`Entity`"): proves `#[engine_class(scene_store, ...)]` delegates
//! to `pulsar_scenedb::SceneStore` correctly on a throwaway struct -- both
//! that the SceneDB storage half (`World`/GPU-mirror) works, AND that the
//! reflection half (`#[property]`/`EngineClass::get_properties`) still
//! works on the very same struct, side by side. The two were never in
//! tension, but this is the first struct that actually exercises both
//! derived facets at once, proving the delegation didn't silently break
//! either.
//!
//! Runs with `pulsar_scenedb`'s `gpu` feature ON (see this crate's
//! Cargo.toml `[dev-dependencies]` comment: this workspace's
//! Pulsar-Reflection pin was bumped to close a version skew that
//! previously blocked this), so the `#[gpu(...)]` field <-> GPU buffer half
//! of the delegation is proven here directly, not just inferred from
//! `pulsar_scenedb`'s own `tests/world_gpu_mirror.rs` (which proves the
//! same mechanism for a plain `#[derive(SceneStore)]` struct, not reached
//! through `#[engine_class]`'s delegation path specifically).

use engine_class_derive::engine_class;
use pulsar_reflection::EngineClass as _;
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuColumnSet, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig,
    SceneGpuStore,
};
use pulsar_scenedb::{Entity, World};
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
        label: Some("engine-class-scene-store-delegation-test"),
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
    // World-mirrored buffers don't use cells/regions at all -- this only
    // needs to be valid enough for `SceneGpuStore::new` to build its own
    // built-ins. Mirrors `pulsar_scenedb`'s own `world_gpu_mirror.rs` test.
    SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 1 }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    }
}

const WORLD_GPU_CAPACITY: u32 = 1024;

/// The struct under test: a real `#[engine_class]` component (reflection-
/// visible `label` field, exactly like every other component in
/// `pulsar_rendering`) that ALSO opts into SceneDB storage via the new
/// `scene_store` flag, with one `#[gpu]` field. `no_register` keeps this
/// hermetic -- no global `inventory` registration from a throwaway test
/// type.
#[engine_class(category = "Test", default, clone, debug, no_register, scene_store)]
pub struct ThrowawayComponent {
    #[property]
    pub label: f32,
    #[gpu]
    pub mesh: u32,
}

#[test]
fn engine_class_scene_store_field_lands_in_its_gpu_buffer_at_entity_index() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    ThrowawayComponent::register_gpu_columns(&mut store, WORLD_GPU_CAPACITY, ctx.device());
    let store = Arc::new(store);

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(Arc::clone(&store), Arc::clone(ctx.queue())));

    // Non-trivial row, same reasoning as the SceneDB-side test this mirrors:
    // row 0 succeeding wouldn't prove `entity.index()` actually drives the
    // write, just that row 0 happens to work.
    for _ in 0..5 {
        world.spawn();
    }
    let entity: Entity = world.spawn();
    let row = entity.index();
    assert_eq!(row, 5, "sanity: depends on a non-zero row");

    world.insert(entity, ThrowawayComponent { label: 7.0, mesh: 0xBEEF });

    let columns = ThrowawayComponent::gpu_columns();
    assert_eq!(columns.len(), 1, "ThrowawayComponent has exactly one #[gpu] field");
    let mesh_field_id = columns[0].field_token.id();
    let mesh_buf = store.buffer_for_id(mesh_field_id).expect("mesh buffer registered");

    let bytes = readback(&ctx, &mesh_buf, (row as u64) * 4, 4);
    let got = u32::from_ne_bytes(bytes.try_into().unwrap());
    assert_eq!(got, 0xBEEF, "engine_class's #[gpu] field must be readable at row = entity.index()");

    // In-place update (re-insert) must re-mirror too, not just first insert.
    world.insert(entity, ThrowawayComponent { label: 7.0, mesh: 0x1234 });
    let bytes = readback(&ctx, &mesh_buf, (row as u64) * 4, 4);
    let got = u32::from_ne_bytes(bytes.try_into().unwrap());
    assert_eq!(got, 0x1234, "re-insert (update) must re-mirror the new value");

    // And the plain CPU-only field is readable back through World normally,
    // proving engine_class's own storage (not just its GPU column) is intact.
    assert_eq!(world.get::<ThrowawayComponent>(entity).unwrap().label, 7.0);
}

#[test]
fn engine_class_scene_store_struct_is_still_reflection_visible() {
    // The whole point: SceneDB storage and the reflection/properties-panel
    // system read the SAME struct, not two parallel representations.
    let value = ThrowawayComponent { label: 42.0, mesh: 0 };
    let props = value.get_properties();
    assert!(
        props.iter().any(|p| p.name == "label"),
        "engine_class's #[property] metadata must survive scene_store delegation, got: {:?}",
        props.iter().map(|p| p.name).collect::<Vec<_>>()
    );
}

#[test]
fn without_an_attached_mirror_insert_never_touches_the_gpu() {
    // No `attach_gpu_mirror` at all -- must behave exactly as before this
    // feature existed: no panic, no silent GPU write.
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, ThrowawayComponent { label: 1.0, mesh: 99 });
    assert_eq!(world.get::<ThrowawayComponent>(entity).unwrap().mesh, 99);
    assert!(!world.has_gpu_mirror());
}
