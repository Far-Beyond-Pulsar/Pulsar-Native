//! The ONE shared GPU decode (`helio_vg_format::DECODE_WGSL`) against the
//! builder's CPU decoder (`VgeoReader::decode`), on a real built asset, plus
//! the store's `vg_asset_quant` upload and the `vg_asset_ready` publication.

use helio_vg_build::{build_vgeo, BuildOptions, MeshInput, VgeoReader};
use helio_vg_format::*;
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

fn storage(device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8], rw: bool) -> wgpu::Buffer {
    let b = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bytes.len() as u64).max(16).next_multiple_of(4),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | if rw {
                wgpu::BufferUsages::COPY_SRC
            } else {
                wgpu::BufferUsages::empty()
            },
        mapped_at_creation: false,
    });
    queue.write_buffer(&b, 0, bytes);
    b
}

fn sphere_asset() -> Vec<u8> {
    // A torus with distinct per-vertex attribute bytes.
    let (nu, nv) = (90u32, 45u32);
    let mut pos = Vec::new();
    for j in 0..nv {
        for i in 0..nu {
            let a = std::f32::consts::TAU * i as f32 / nu as f32;
            let b = std::f32::consts::TAU * j as f32 / nv as f32;
            let r = 1.0 + 0.4 * b.cos();
            pos.push([
                r * a.cos() * 3.0 + 10.0,
                0.4 * b.sin() - 2.0,
                r * a.sin() * 3.0,
            ]);
        }
    }
    let at = |i: u32, j: u32| (j % nv) * nu + (i % nu);
    let mut idx = Vec::new();
    for j in 0..nv {
        for i in 0..nu {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            idx.extend([a, b, c, a, c, d]);
        }
    }
    let mut attrs = Vec::new();
    for v in 0..pos.len() as u32 {
        for k in 0..16u32 {
            attrs.push((v.wrapping_mul(31).wrapping_add(k * 17) & 0xff) as u8);
        }
    }
    let mats = vec![0u32; idx.len() / 3];
    build_vgeo(
        &MeshInput {
            positions: &pos,
            attributes: &attrs,
            attribute_stride: ATTR_STRIDE as usize,
            indices: &idx,
            tri_material: &mats,
        },
        &BuildOptions {
            content_id: [11, 13],
            resident_levels: 2,
        },
    )
    .unwrap()
}

const SHADER: &str = "
@group(1) @binding(0) var<storage, read> jobs: array<vec4<u32>>;
@group(1) @binding(1) var<storage, read_write> out_pos: array<vec4<f32>>;
@group(1) @binding(2) var<storage, read_write> out_q: array<vec4<u32>>;
@group(1) @binding(3) var<storage, read_write> out_tri: array<vec4<u32>>;
@group(1) @binding(4) var<storage, read_write> out_attr: array<vec4<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= arrayLength(&jobs)) { return; }
    let j = jobs[gid.x];   // cluster_slot, vertex offset, asset slot, triangle offset
    let cl = vgd_load_cluster(j.x);
    let blk = vgd_pos_block(j.x, cl, j.z);
    for (var v = 0u; v < vgd_vert_count(cl); v++) {
        out_q[j.y + v] = vec4<u32>(vgd_lattice(blk, v), 0u);
        out_pos[j.y + v] = vec4<f32>(vgd_pos(blk, v), 0.0);
        let a = vgd_attr(j.x, blk, v);
        out_attr[j.y + v] = vec4<f32>(a.normal, a.uv0.x);
    }
    for (var t = 0u; t < vgd_tri_count(cl); t++) {
        out_tri[j.w + t] = vec4<u32>(vgd_tri(j.x, cl, t), vgd_material(cl));
    }
}
";

#[test]
fn shared_gpu_decode_matches_the_builders_cpu_decoder_for_every_vertex() {
    let Some((device, queue)) = device() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let bytes = sphere_asset();
    let r = VgeoReader::parse(&bytes).unwrap();
    let src = VgeoBytes::new(bytes.clone()).unwrap();
    let pages = r.header.asset.page_count;
    assert!(pages >= 1);

    // Pool: slot == page id.
    let mut pool = Vec::new();
    for p in 0..pages {
        pool.extend_from_slice(&src.read_page(p).unwrap());
    }
    let quant = src.quant();
    // Jobs and expected values.
    let (mut jobs, mut exp_q, mut exp_pos, mut exp_tri, mut exp_attr): (
        Vec<[u32; 4]>,
        Vec<[u32; 3]>,
        Vec<[f32; 3]>,
        Vec<[u32; 4]>,
        Vec<[f32; 4]>,
    ) = Default::default();
    for p in 0..pages {
        let page = r.page(p).unwrap();
        for i in 0..page.header.cluster_count {
            let d = page.decode_cluster(i).unwrap();
            let c = page.cluster(i).unwrap();
            jobs.push([
                cluster_slot(p, i),
                exp_q.len() as u32,
                0,
                exp_tri.len() as u32,
            ]);
            for (v, q) in d.lattice.iter().enumerate() {
                exp_q.push(*q);
                exp_pos.push(d.positions[v]);
                let a = &d.attributes[v * 16..v * 16 + 16];
                let sn = |b: u8| ((b as i8) as f32 / 127.0).max(-1.0);
                let uv = {
                    let w = u32::from_le_bytes(a[8..12].try_into().unwrap());
                    half_to_f32((w & 0xffff) as u16)
                };
                exp_attr.push([sn(a[0]), sn(a[1]), sn(a[2]), uv]);
            }
            for t in &d.tris {
                exp_tri.push([t[0] as u32, t[1] as u32, t[2] as u32, c.material_slot()]);
            }
        }
    }
    let job_bytes: &[u8] = bytemuck::cast_slice(&jobs);
    let jobs_buf = storage(&device, &queue, job_bytes, false);
    let mk_out = |n: usize, stride: usize| storage(&device, &queue, &vec![0u8; n * stride], true);
    let out_pos = mk_out(exp_q.len(), 16);
    let out_q = mk_out(exp_q.len(), 16);
    let out_tri = mk_out(exp_tri.len(), 16);
    let out_attr = mk_out(exp_q.len(), 16);
    let pool_buf = storage(&device, &queue, &pool, false);
    let quant_buf = storage(&device, &queue, bytemuck::bytes_of(&quant), false);

    let src_wgsl = format!(
        "{}\n{}\n{}\n{}",
        helio_vg_format::WGSL,
        decode_bindings_wgsl(0, 0, 1),
        DECODE_WGSL.to_string() + DECODE_ATTR_WGSL,
        SHADER
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("decode test"),
        source: wgpu::ShaderSource::Wgsl(src_wgsl.into()),
    });
    let ro = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let rw = |binding| wgpu::BindGroupLayoutEntry {
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        ..ro(binding)
    };
    let l0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[ro(0), ro(1)],
    });
    let l1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[ro(0), rw(1), rw(2), rw(3), rw(4)],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&l0), Some(&l1)],
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
    fn e(b: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding: b,
            resource: buf.as_entire_binding(),
        }
    }
    let bg0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &l0,
        entries: &[e(0, &pool_buf), e(1, &quant_buf)],
    });
    let bg1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &l1,
        entries: &[
            e(0, &jobs_buf),
            e(1, &out_pos),
            e(2, &out_q),
            e(3, &out_tri),
            e(4, &out_attr),
        ],
    });
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg0, &[]);
        pass.set_bind_group(1, &bg1, &[]);
        pass.dispatch_workgroups((jobs.len() as u32).div_ceil(64), 1, 1);
    }
    queue.submit(Some(enc.finish()));

    let got_q: Vec<[u32; 4]> = bytemuck::pod_collect_to_vec(&read_buffer(
        &device,
        &queue,
        &out_q,
        0,
        exp_q.len() as u64 * 16,
    ));
    let got_pos: Vec<[f32; 4]> = bytemuck::pod_collect_to_vec(&read_buffer(
        &device,
        &queue,
        &out_pos,
        0,
        exp_q.len() as u64 * 16,
    ));
    let got_tri: Vec<[u32; 4]> = bytemuck::pod_collect_to_vec(&read_buffer(
        &device,
        &queue,
        &out_tri,
        0,
        exp_tri.len() as u64 * 16,
    ));
    let got_attr: Vec<[f32; 4]> = bytemuck::pod_collect_to_vec(&read_buffer(
        &device,
        &queue,
        &out_attr,
        0,
        exp_q.len() as u64 * 16,
    ));
    assert!(exp_q.len() > 1000);
    for i in 0..exp_q.len() {
        assert_eq!(got_q[i][..3], exp_q[i], "lattice coordinate of vertex {i}");
        for a in 0..3 {
            let tol = 4.0 * f32::EPSILON * exp_pos[i][a].abs().max(1.0);
            assert!(
                (got_pos[i][a] - exp_pos[i][a]).abs() <= tol,
                "position {i}.{a}: {} vs {}",
                got_pos[i][a],
                exp_pos[i][a]
            );
        }
        for a in 0..3 {
            assert!(
                (got_attr[i][a] - exp_attr[i][a]).abs() < 1e-6,
                "normal {i}.{a}"
            );
        }
        assert!(
            (got_attr[i][3] - exp_attr[i][3]).abs() < 1e-6 * exp_attr[i][3].abs().max(1.0)
                || exp_attr[i][3].is_nan(),
            "uv {i}"
        );
    }
    assert_eq!(got_tri, exp_tri, "triangle indices + material");
}

fn half_to_f32(h: u16) -> f32 {
    let (s, e, m) = ((h >> 15) as u32, ((h >> 10) & 31) as i32, (h & 1023) as u32);
    let v = if e == 0 {
        (m as f32) * 2f32.powi(-24)
    } else if e == 31 {
        if m == 0 {
            f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        (1.0 + m as f32 / 1024.0) * 2f32.powi(e - 15)
    };
    if s == 1 {
        -v
    } else {
        v
    }
}

#[test]
fn store_uploads_asset_quant_and_publishes_asset_ready_rows() {
    let Some((device, queue)) = device() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let bytes = sphere_asset();
    let r = VgeoReader::parse(&bytes).unwrap();
    let mut store = VgAssetStore::new(
        &device,
        &queue,
        VgStreamConfig {
            budget_mb: 64,
            ..Default::default()
        },
    );
    let h = store
        .register(Arc::new(VgeoBytes::new(bytes.clone()).unwrap()))
        .unwrap();
    let content = r.header.content_id;
    let mapping = [
        ([99u64, 99], 5000u32, 10u32),
        (content, 300, 8100),
        ([1, 2], 40, 3),
    ];
    let rows = store.asset_ready_rows(&mapping, 4);
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows[0],
        VgAssetReadyRow {
            vertex_offset: 300,
            vertex_count: 8100,
            asset_slot: h.slot(),
            flags: 0
        },
        "registered but not ready"
    );
    assert!(rows[1..].iter().all(|x| *x == VgAssetReadyRow::PADDING));
    for _ in 0..2000 {
        store.update();
        if store.is_ready(h) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(store.is_ready(h));
    let rows = store.asset_ready_rows(&mapping, 1);
    assert_eq!(rows[0].flags, VG_ASSET_READY_FLAG_READY);
    assert_eq!(rows.len(), 1);
    // Sorted by vertex_offset even when the mapping is not.
    let h2 = store
        .register(Arc::new(helio_vg_stream::testing::FakeSource::new(5, 4, 1)))
        .unwrap();
    let rows = store.asset_ready_rows(&[(content, 900, 1), ([5, 0], 100, 1)], 0);
    assert_eq!(
        rows.iter().map(|x| x.vertex_offset).collect::<Vec<_>>(),
        vec![100, 900]
    );
    assert_eq!(rows[0].asset_slot, h2.slot());

    // The quant table row on the GPU equals the file's meta block.
    let got: Vec<f32> = bytemuck::pod_collect_to_vec(&read_buffer(
        &device,
        &queue,
        store.buffer(VgBuffer::AssetQuant),
        u64::from(h.slot()) * 16,
        16,
    ));
    assert_eq!(got[..3], r.meta.origin);
    assert_eq!(got[3], r.meta.step);
}
