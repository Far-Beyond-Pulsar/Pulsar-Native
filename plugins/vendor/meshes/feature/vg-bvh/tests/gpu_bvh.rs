//! Headless GPU test: the GPU build and refit against the CPU reference.
//! Skips (passes) when no adapter can be created.

mod common;

use common::*;
use helio_pass_gbuffer::StaticObjectRow;
use helio_pass_vg_bvh::reference::{build, header_of, refit, validate, RefTree};
use helio_pass_vg_bvh::{BvhConfig, BvhMode, GpuBvhHeader, GpuBvhNode, VgBvhPass};
use helio_vg_format::GpuVgInstance;
use wgpu::util::DeviceExt;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let mut adapter = None;
        for force_fallback_adapter in [false, true] {
            if let Ok(a) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter,
                    apply_limit_buckets: false,
                })
                .await
            {
                adapter = Some(a);
                break;
            }
        }
        let adapter = adapter?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("VgBvh Test Device"),
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .ok()?;
        device.on_uncaptured_error(std::sync::Arc::new(|error| {
            panic!("vg bvh GPU validation error: {error:?}");
        }));
        Some((device, queue))
    })
}

fn upload<T: bytemuck::Pod>(device: &wgpu::Device, label: &str, data: &[T]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

fn read<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buf: &wgpu::Buffer,
    count: usize,
) -> Vec<T> {
    let bytes = (count * std::mem::size_of::<T>()) as u64;
    if bytes == 0 {
        return Vec::new();
    }
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buf, 0, &staging, 0, bytes);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let view = slice.get_mapped_range().expect("mapped range");
    let out = bytemuck::cast_slice(&view).to_vec();
    drop(view);
    out
}

struct Gpu {
    header: GpuBvhHeader,
    nodes: Vec<GpuBvhNode>,
    items: Vec<u32>,
}

fn fetch(device: &wgpu::Device, queue: &wgpu::Queue, pass: &VgBvhPass) -> Gpu {
    let header: GpuBvhHeader = read::<GpuBvhHeader>(device, queue, pass.header_buffer(), 1)[0];
    let nodes = read::<GpuBvhNode>(
        device,
        queue,
        pass.nodes_buffer(),
        header.node_count as usize,
    );
    let items = read::<u32>(
        device,
        queue,
        pass.items_buffer(),
        header.instance_count as usize,
    );
    Gpu {
        header,
        nodes,
        items,
    }
}

/// Compares the GPU's output with the reference tree and the validity rules.
fn assert_matches(
    label: &str,
    gpu: &Gpu,
    reference: &RefTree,
    instances: &[GpuVgInstance],
    objects: &[StaticObjectRow],
) {
    validate(&gpu.nodes, &gpu.items, &gpu.header, instances, objects)
        .unwrap_or_else(|e| panic!("{label}: GPU tree invalid: {e}"));
    let want = header_of(reference, reference.items.len() as u32);
    assert_eq!(
        gpu.header.instance_count, want.instance_count,
        "{label}: instance_count"
    );
    assert_eq!(
        gpu.header.leaf_count, want.leaf_count,
        "{label}: leaf_count"
    );
    assert_eq!(
        gpu.header.node_count, want.node_count,
        "{label}: node_count"
    );
    assert_eq!(
        gpu.header.batch_leaf_count, want.batch_leaf_count,
        "{label}: batch_leaf_count"
    );
    assert_eq!(gpu.header.root_node, want.root_node, "{label}: root_node");
    assert_eq!(
        gpu.header.bounds_min, want.bounds_min,
        "{label}: bounds_min"
    );
    assert_eq!(
        gpu.header.bounds_max, want.bounds_max,
        "{label}: bounds_max"
    );
    let m = gpu.header.leaf_count as u32;
    assert_eq!(
        gpu.header.dispatch_leaves,
        [m.div_ceil(64), 1, 1],
        "{label}: dispatch_leaves"
    );
    assert_eq!(
        gpu.header.dispatch_instances,
        [gpu.header.instance_count.div_ceil(64), 1, 1],
        "{label}: dispatch_instances"
    );
    assert_eq!(gpu.items, reference.items, "{label}: Morton order");
    assert_eq!(
        gpu.nodes.len(),
        reference.nodes.len(),
        "{label}: node count"
    );
    for (i, (g, r)) in gpu.nodes.iter().zip(&reference.nodes).enumerate() {
        assert_eq!(g, r, "{label}: node {i}");
    }
}

fn scenario(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    instances: &[GpuVgInstance],
    objects: &mut Vec<StaticObjectRow>,
    cfg: BvhConfig,
) {
    let inst_buf = upload(device, "instances", instances);
    let obj_buf = upload(device, "objects", objects);
    let mut pass = VgBvhPass::with_config(device, cfg);

    pass.run_for_testing(device, queue, &inst_buf, &obj_buf, BvhMode::Rebuild);
    let mut reference = build(instances, objects, &cfg);
    let gpu = fetch(device, queue, &pass);
    assert_matches(
        &format!("{label} rebuild"),
        &gpu,
        &reference,
        instances,
        objects,
    );
    let build_gen = gpu.header.build_generation;
    let refit_gen = gpu.header.refit_generation;

    // Move a third of the objects, upload, refit only.
    let mut rng = Rng(99);
    for o in objects.iter_mut() {
        if rng.next_u32() % 3 == 0 {
            for k in 0..3 {
                o.bounds[k] += rng.unit() * 6.0 - 3.0;
            }
        }
    }
    queue.write_buffer(&obj_buf, 0, bytemuck::cast_slice(objects));
    pass.run_for_testing(device, queue, &inst_buf, &obj_buf, BvhMode::Refit);
    refit(&mut reference, instances, objects);
    let gpu = fetch(device, queue, &pass);
    assert_matches(
        &format!("{label} refit"),
        &gpu,
        &reference,
        instances,
        objects,
    );
    assert_eq!(
        gpu.header.build_generation, build_gen,
        "{label}: refit must not rebuild"
    );
    assert_eq!(
        gpu.header.refit_generation,
        refit_gen + 1,
        "{label}: refit generation"
    );

    // A full rebuild over the moved scene matches a fresh reference build.
    pass.run_for_testing(device, queue, &inst_buf, &obj_buf, BvhMode::Rebuild);
    let fresh = build(instances, objects, &cfg);
    let gpu = fetch(device, queue, &pass);
    assert_matches(
        &format!("{label} rebuild 2"),
        &gpu,
        &fresh,
        instances,
        objects,
    );
    assert_eq!(gpu.header.build_generation, build_gen + 1);
}

#[test]
fn gpu_matches_reference() {
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: vg bvh");
        return;
    };

    // Holes (disabled / dead rows), several workgroups' worth.
    for (n, seed) in [(1usize, 1u64), (2, 2), (3, 3), (50, 4), (700, 5), (6000, 6)] {
        let (instances, mut objects) = random_scene(seed, n, 200.0, 0.2, 3.0);
        scenario(
            &device,
            &queue,
            &format!("random {n}"),
            &instances,
            &mut objects,
            BvhConfig::default(),
        );
    }

    // Dense tiny instances: batch leaves.
    let (instances, mut objects) = city_scene(11, 12, 300);
    scenario(
        &device,
        &queue,
        "city",
        &instances,
        &mut objects,
        BvhConfig::default(),
    );
    scenario(
        &device,
        &queue,
        "city, batching off",
        &instances,
        &mut objects.clone(),
        BvhConfig {
            batch_max_instances: 1,
            ..Default::default()
        },
    );
    scenario(
        &device,
        &queue,
        "city, batch 16",
        &instances,
        &mut objects.clone(),
        BvhConfig {
            batch_max_instances: 16,
            ..Default::default()
        },
    );
}

#[test]
fn gpu_handles_empty_and_all_equal_keys() {
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: vg bvh");
        return;
    };
    // No live rows.
    let instances = vec![helio_pass_vg_bvh_test_disabled(); 40];
    let mut objects = vec![object([0.0; 3], 1.0); 40];
    scenario_empty(&device, &queue, &instances, &mut objects);

    // Every key equal: the leaf-index tie-break.
    let objects: Vec<_> = (0..500).map(|_| object([1.0, 2.0, 3.0], 1.0)).collect();
    let instances: Vec<_> = (0..500).map(instance).collect();
    scenario(
        &device,
        &queue,
        "equal keys",
        &instances,
        &mut objects.clone(),
        BvhConfig::default(),
    );
}

fn helio_pass_vg_bvh_test_disabled() -> GpuVgInstance {
    GpuVgInstance {
        flags: 0,
        ..instance(0)
    }
}

fn scenario_empty(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    instances: &[GpuVgInstance],
    objects: &mut Vec<StaticObjectRow>,
) {
    let inst_buf = upload(device, "instances", instances);
    let obj_buf = upload(device, "objects", objects);
    let mut pass = VgBvhPass::new(device);
    pass.run_for_testing(device, queue, &inst_buf, &obj_buf, BvhMode::Rebuild);
    let gpu = fetch(device, queue, &pass);
    assert_eq!(gpu.header.instance_count, 0);
    assert_eq!(gpu.header.node_count, 0);
    assert_eq!(gpu.header.root_node, helio_pass_vg_bvh::NODE_INVALID);
    // Refit of an empty tree is harmless.
    pass.run_for_testing(device, queue, &inst_buf, &obj_buf, BvhMode::Refit);
    let gpu = fetch(device, queue, &pass);
    assert_eq!(gpu.header.node_count, 0);
}
