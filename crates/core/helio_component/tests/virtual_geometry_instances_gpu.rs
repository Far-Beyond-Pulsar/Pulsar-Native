//! GPU run of the virtual-geometry instance derivation against hand-built
//! scene-join-shaped inputs: which static-mesh instances become VG-active,
//! with which flags, and which objects the authored LOD must skip.

use helio_component::components::VgOverrideGpu;
use helio_component::virtual_geometry_instances::{
    VgAssetReadyRow, VgInstanceKeys, VirtualGeometryInstances, LOD_SUPPRESSED_KEY,
    VG_ASSET_READY_FLAG_READY, VG_ASSET_READY_KEY, VG_INSTANCES_KEY, VG_INSTANCE_COUNT_KEY,
};
use helio_core::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
};
use helio_vg_format::{
    GpuVgInstance, TierLock, INST_CONE_CULL, INST_ENABLED, INST_FORCE_ROOT_ONLY,
    INST_LOD_SUPPRESS, INST_SHADOW_CASTER, INST_TIER_SHIFT,
};
use std::sync::Arc;

fn gpu() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("the VG derivation test requires a local adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .expect("device");
    (Arc::new(device), Arc::new(queue))
}

fn upload<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rows: &[T],
    generation: u64,
) -> BufferHandle {
    let bytes = bytemuck::cast_slice(rows);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VG test input"),
        size: bytes.len().max(4) as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytes);
    BufferHandle {
        buffer,
        epoch: 1,
        row_bytes: std::mem::size_of::<T>() as u64,
        content_generation: generation,
    }
}

fn read<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    handle: &BufferHandle,
    rows: usize,
) -> Vec<T> {
    let bytes = (rows * std::mem::size_of::<T>()) as u64;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("VG test readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&handle.buffer, 0, &staging, 0, bytes);
    queue.submit([encoder.finish()]);
    staging.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = staging.slice(..).get_mapped_range().expect("range");
    let out = mapped
        .chunks_exact(std::mem::size_of::<T>())
        .map(bytemuck::pod_read_unaligned::<T>)
        .collect();
    drop(mapped);
    staging.unmap();
    out
}

/// A `static_objects` row (63 words): mesh slot, generation, and a model
/// matrix with the given axis scales.
fn object_row(slot: u32, mesh_generation: u32, scale: [f32; 3]) -> [u32; 63] {
    let mut row = [0u32; 63];
    row[0] = slot;
    row[1] = mesh_generation;
    let model = [
        [scale[0], 0.0, 0.0, 0.0],
        [0.0, scale[1], 0.0, 0.0],
        [0.0, 0.0, scale[2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    for (column, values) in model.iter().enumerate() {
        for (component, value) in values.iter().enumerate() {
            row[4 + column * 4 + component] = value.to_bits();
        }
    }
    row
}

struct Scene {
    projection: SceneBufferProjection,
    keys: VgInstanceKeys,
}

/// Eight mesh instances (rows 0..8 own objects 10..18), VG components at
/// entity rows 20 (object 14, disabled) and 21 (object 11, SW tier, root only,
/// error scale 2).
fn scene(device: &wgpu::Device, queue: &wgpu::Queue, asset_generation: u64) -> Scene {
    let keys = VgInstanceKeys::standard();
    let owner = |object: u32| [object, 7u32, 1u32];
    let mut owners = vec![[0u32; 3]; 24];
    for i in 0..8u32 {
        owners[i as usize] = owner(10 + i);
    }
    owners[20] = owner(14);
    owners[21] = owner(11);

    let section_handles: Vec<[u32; 2]> = (0..8).map(|i| [i, 1]).collect();
    // Instance vertex ranges: 100 and 200 are ready assets, 300 is known but
    // not ready, 999 is unknown.
    let vertex_offsets = [100u32, 200, 300, 100, 100, 999, 100, 100];
    let vertex_handles: Vec<[u32; 2]> = vertex_offsets.iter().map(|&o| [o, 24]).collect();

    let mut objects: Vec<[u32; 63]> = (0..8).map(|i| object_row(i, 8, [1.0; 3])).collect();
    objects[3] = object_row(3, 8, [-1.0, 1.0, 1.0]); // mirrored
    objects[6] = object_row(6, 8, [1.0, 2.0, 1.0]); // non-uniform
    objects[7] = object_row(7, 0, [1.0; 3]); // not placed by the join

    let mut override_handles = vec![[0u32; 2]; 24];
    override_handles[20] = [0, 1];
    override_handles[21] = [1, 1];
    let overrides = [
        VgOverrideGpu { flags: 0, lod_error_scale: 1.0 },
        VgOverrideGpu {
            flags: INST_ENABLED
                | INST_FORCE_ROOT_ONLY
                | ((TierLock::SwRaster as u32) << INST_TIER_SHIFT),
            lod_error_scale: 2.0,
        },
    ];
    let ready = [
        VgAssetReadyRow {
            vertex_offset: 100,
            vertex_count: 24,
            asset_slot: 5,
            flags: VG_ASSET_READY_FLAG_READY,
        },
        VgAssetReadyRow {
            vertex_offset: 200,
            vertex_count: 24,
            asset_slot: 6,
            flags: VG_ASSET_READY_FLAG_READY,
        },
        VgAssetReadyRow { vertex_offset: 300, vertex_count: 24, asset_slot: 7, flags: 0 },
        VgAssetReadyRow::PADDING,
    ];

    let projection = SceneBufferProjection::from_handles([
        (keys.owners, upload(device, queue, &owners, 1)),
        (keys.generations, upload(device, queue, &[7u32; 32], 1)),
        (keys.override_handles, upload(device, queue, &override_handles, 1)),
        (keys.overrides, upload(device, queue, &overrides, 1)),
        (keys.section_handles, upload(device, queue, &section_handles, 1)),
        (keys.vertex_handles, upload(device, queue, &vertex_handles, 1)),
        (BufferKey::of("static_objects"), upload(device, queue, &objects, 1)),
        (VG_ASSET_READY_KEY, upload(device, queue, &ready, asset_generation)),
    ]);
    Scene { projection, keys }
}

fn derive(
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    derivation: &mut VirtualGeometryInstances,
    projection: &SceneBufferProjection,
) -> (Vec<GpuVgInstance>, Vec<u32>, bool) {
    let mut encoder = device.create_command_encoder(&Default::default());
    let output = derivation.derive(
        &SceneDerivationContext { device, queue, inputs: projection, world_origin: glam::DVec3::ZERO },
        &mut encoder,
    );
    queue.submit([encoder.finish()]);
    let get = |key: BufferKey| {
        output
            .buffers
            .iter()
            .find(|(k, _)| *k == key)
            .unwrap_or_else(|| panic!("{key:?} not published"))
            .1
            .clone()
    };
    let count = read::<u32>(device, queue, &get(VG_INSTANCE_COUNT_KEY), 1)[0] as usize;
    let rows = read::<GpuVgInstance>(device, queue, &get(VG_INSTANCES_KEY), count.max(1));
    let suppressed_handle = get(LOD_SUPPRESSED_KEY);
    let suppressed_rows = (suppressed_handle.buffer.size() / 4) as usize;
    let suppressed = read::<u32>(device, queue, &suppressed_handle, suppressed_rows);
    (rows[..count].to_vec(), suppressed, output.recorded)
}

#[test]
fn ready_placed_enabled_instances_become_active_with_derived_flags() {
    let (device, queue) = gpu();
    let mut derivation = VirtualGeometryInstances::new(&device, VgInstanceKeys::standard());
    let scene = scene(&device, &queue, 1);
    let (rows, suppressed, recorded) = derive(&device, &queue, &mut derivation, &scene.projection);
    assert!(recorded);

    // Instances 0, 1, 3 and 6 are active, in instance order. 2: asset not
    // ready, 4: component disabled, 5: asset unknown, 7: not placed.
    assert_eq!(rows.len(), 4, "{rows:?}");

    let defaults = INST_ENABLED | INST_SHADOW_CASTER | INST_LOD_SUPPRESS | INST_CONE_CULL;
    assert_eq!(rows[0], GpuVgInstance { asset_slot: 5, flags: defaults, lod_error_scale: 1.0, object_row: 0 });

    // Authored overrides replace the defaults; derived bits are still set.
    let authored = INST_ENABLED
        | INST_FORCE_ROOT_ONLY
        | INST_LOD_SUPPRESS
        | INST_CONE_CULL
        | ((TierLock::SwRaster as u32) << INST_TIER_SHIFT);
    assert_eq!(rows[1], GpuVgInstance { asset_slot: 6, flags: authored, lod_error_scale: 2.0, object_row: 1 });
    assert_eq!(rows[1].tier_lock(), TierLock::SwRaster);

    // Mirrored and non-uniformly scaled instances are active but never
    // cone-culled.
    assert_eq!(rows[2].object_row, 3);
    assert_eq!(rows[2].flags, defaults & !INST_CONE_CULL);
    assert_eq!(rows[3].object_row, 6);
    assert_eq!(rows[3].flags, defaults & !INST_CONE_CULL);

    // The authored LOD must skip exactly the objects of active instances
    // (objects 10, 11, 13, 16); never those of inactive ones.
    for object in 0..suppressed.len() {
        let expected = matches!(object, 10 | 11 | 13 | 16);
        assert_eq!(suppressed[object] != 0, expected, "object {object}");
    }
}

#[test]
fn the_derivation_follows_readiness_and_the_global_switch() {
    let (device, queue) = gpu();
    let mut derivation = VirtualGeometryInstances::new(&device, VgInstanceKeys::standard());
    let control = derivation.control();
    let mut scene = scene(&device, &queue, 1);
    let (rows, _, _) = derive(&device, &queue, &mut derivation, &scene.projection);
    assert_eq!(rows.len(), 4);

    // Unchanged inputs do not record work again.
    let (_, _, recorded) = derive(&device, &queue, &mut derivation, &scene.projection);
    assert!(!recorded);

    // The streamer finishing asset 300 makes instance 2 active.
    let ready = [
        VgAssetReadyRow { vertex_offset: 100, vertex_count: 24, asset_slot: 5, flags: 1 },
        VgAssetReadyRow { vertex_offset: 200, vertex_count: 24, asset_slot: 6, flags: 1 },
        VgAssetReadyRow { vertex_offset: 300, vertex_count: 24, asset_slot: 7, flags: 1 },
        VgAssetReadyRow::PADDING,
    ];
    scene.projection.insert(VG_ASSET_READY_KEY, upload(&device, &queue, &ready, 2));
    let (rows, suppressed, recorded) = derive(&device, &queue, &mut derivation, &scene.projection);
    assert!(recorded);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[2].asset_slot, 7);
    assert_ne!(suppressed[12], 0);

    // Turning virtual geometry off empties everything, flags included.
    control.set_enabled(false);
    let (rows, suppressed, recorded) = derive(&device, &queue, &mut derivation, &scene.projection);
    assert!(recorded);
    assert!(rows.is_empty());
    assert!(suppressed.iter().all(|&flag| flag == 0));
}

#[test]
fn missing_inputs_publish_empty_outputs() {
    let (device, queue) = gpu();
    let mut derivation = VirtualGeometryInstances::new(&device, VgInstanceKeys::standard());
    let projection = SceneBufferProjection::from_handles([]);
    let (rows, suppressed, _) = derive(&device, &queue, &mut derivation, &projection);
    assert!(rows.is_empty());
    assert!(suppressed.iter().all(|&flag| flag == 0));
}
