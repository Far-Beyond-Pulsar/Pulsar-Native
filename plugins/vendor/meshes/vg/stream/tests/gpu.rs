//! Headless wgpu test of the store: pool uploads, residency table, shader
//! request append, readback decode, and streaming of requested pages.
//! Skips (passes) when no adapter is available.

use helio_vg_format::*;
use helio_vg_stream::testing::FakeSource;
use helio_vg_stream::*;
use std::sync::Arc;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let limits = adapter.limits();
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits {
            max_storage_buffers_per_shader_stage:
                limits.max_storage_buffers_per_shader_stage.min(16),
            max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
            max_buffer_size: limits.max_buffer_size,
            ..wgpu::Limits::default()
        },
        ..Default::default()
    }))
    .ok()
}

fn read_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::Buffer,
    offset: u64,
    len: u64,
) -> Vec<u8> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: len,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(src, offset, &staging, 0, len);
    queue.submit(Some(enc.finish()));
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let out = staging
        .slice(..)
        .get_mapped_range()
        .expect("range")
        .to_vec();
    staging.unmap();
    out
}

const PAGES: u32 = 12;
const ROOTS: u32 = 2;

fn shader() -> String {
    format!(
        "{}\n@group(1) @binding(0) var<storage, read_write> out_slots: array<u32>;\n\
         @compute @workgroup_size(16) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
            if (id.x < {PAGES}u) {{ out_slots[id.x] = vg_use_or_request_page(vg_page_base(0u) + id.x); }}\n\
         }}",
        prelude_wgsl(0, 0)
    )
}

#[test]
fn streams_requested_pages_through_the_gpu() {
    let Some((device, queue)) = device() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let mut store = VgAssetStore::new(
        &device,
        &queue,
        VgStreamConfig {
            budget_mb: 4,
            initial_slots: 4,
            grow_chunk_slots: 4,
            ..Default::default()
        },
    );
    let src = Arc::new(FakeSource::new(42, PAGES, ROOTS));
    let h = store.register(src).expect("register");
    assert_eq!(h.slot(), 0);
    assert!(!store.is_ready(h));
    for _ in 0..2000 {
        store.update();
        if store.is_ready(h) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(store.is_ready(h), "root pages never became resident");
    let base = store.global_page(h, 0).unwrap();

    // Pool contents of root pages match the source bytes.
    for local in 0..ROOTS {
        let slot = store.page_slot(base + local).expect("root resident");
        let bytes = read_buffer(
            &device,
            &queue,
            store.buffer(VgBuffer::PagePool),
            u64::from(slot) * u64::from(PAGE_BYTES),
            u64::from(PAGE_BYTES),
        );
        assert_eq!(
            bytes,
            FakeSource::new(42, PAGES, ROOTS).read_page(local).unwrap()
        );
    }

    // Shader: every page resolves a slot or appends a request.
    let layout = VgAssetStore::bind_group_layout(&device, VgBindKind::Compute, 0);
    let out_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vg stream test"),
        source: wgpu::ShaderSource::Wgsl(shader().into()),
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout), Some(&out_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&pl),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(PAGES) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let out_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &out_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: out.as_entire_binding(),
        }],
    });

    let run = |store: &mut VgAssetStore| -> Vec<u32> {
        let bg = store.create_bind_group(&layout, VgBindKind::Compute, 0);
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.set_bind_group(1, &out_bg, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        store.record_feedback_readback(&mut enc);
        queue.submit(Some(enc.finish()));
        store.after_submit();
        bytemuck::pod_collect_to_vec(&read_buffer(&device, &queue, &out, 0, u64::from(PAGES) * 4))
    };

    let first = run(&mut store);
    for i in 0..PAGES {
        let expect = store.page_slot(base + i).unwrap_or(PAGE_NOT_RESIDENT);
        assert_eq!(first[i as usize], expect, "page {i} slot as seen by shader");
    }
    assert!(first[ROOTS as usize..]
        .iter()
        .all(|&s| s == PAGE_NOT_RESIDENT));

    // Feedback arrives asynchronously; keep stepping until every page streams in
    // (pool limit is 32 slots, the asset has 12 pages).
    let mut last = first;
    for _ in 0..4000 {
        store.update();
        last = run(&mut store);
        if last.iter().all(|&s| s != PAGE_NOT_RESIDENT) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        last.iter().all(|&s| s != PAGE_NOT_RESIDENT),
        "pages never streamed in: {last:?}"
    );

    // Residency table on the GPU agrees with the CPU view; slots are distinct.
    let table: Vec<u32> = bytemuck::pod_collect_to_vec(&read_buffer(
        &device,
        &queue,
        store.buffer(VgBuffer::Residency),
        u64::from(base) * 4,
        u64::from(PAGES) * 4,
    ));
    let mut slots = table.clone();
    slots.sort_unstable();
    slots.dedup();
    assert_eq!(slots.len(), PAGES as usize);
    for i in 0..PAGES {
        assert_eq!(table[i as usize], store.page_slot(base + i).unwrap());
    }
    let s = store.stats();
    assert_eq!(s.resident_pages, PAGES);
    assert!(s.pool_buffer_bytes >= u64::from(PAGES) * u64::from(PAGE_BYTES));
    assert!(
        store.bind_version() > 7,
        "pool growth must bump a buffer version"
    );

    // Release frees the table entries.
    store.release(h);
    let table: Vec<u32> = bytemuck::pod_collect_to_vec(&read_buffer(
        &device,
        &queue,
        store.buffer(VgBuffer::Residency),
        u64::from(base) * 4,
        u64::from(PAGES) * 4,
    ));
    assert!(table.iter().all(|&v| v == PAGE_NOT_RESIDENT));
    assert_eq!(store.stats().resident_pages, 0);
}
