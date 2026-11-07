//! Headless end-to-end test of resolve -> bin -> shade -> final depth on a
//! synthetic two-triangle scene. Skips (passes) when no adapter / device with
//! the required capabilities is available.

use glam::{Mat4, Vec2, Vec3, Vec4};
use helio_pass_vg_shade::{
    cpu_ref, shader_source, VgShade, VgShadeGlobals, VgShadeInputs, VgShadeTargets,
    VgVisibleCluster,
};
use helio_vg_format::{vis_pack, GpuVgInstance};
use wgpu::util::DeviceExt;

const N: u32 = 64;
const HIDDEN_FROM_ROW: u32 = 52;
const CLASSIC_DEPTH: f32 = 0.5;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let mut adapter = None;
    for force_fallback_adapter in [false, true] {
        if let Ok(a) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter,
            apply_limit_buckets: false,
        })) {
            adapter = Some(a);
            break;
        }
    }
    let Some(adapter) = adapter else {
        eprintln!("SKIP: no adapter");
        return None;
    };
    let limits = adapter.limits();
    if limits.max_storage_buffers_per_shader_stage < 20
        || limits.max_storage_textures_per_shader_stage < 8
    {
        eprintln!("SKIP: adapter limits too low for the shade pass");
        return None;
    }
    let features = adapter.features() & helio_mats::BINDLESS_MATERIAL_FEATURES;
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vg-shade test"),
            required_features: features,
            required_limits: limits,
            ..Default::default()
        })) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("SKIP: device creation failed: {e}");
                return None;
            }
        };
    device.on_uncaptured_error(std::sync::Arc::new(|e| {
        panic!("GPU validation error: {e:?}")
    }));
    Some(Gpu { device, queue })
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn upload(gpu: &Gpu, tex: &wgpu::Texture, bytes: &[u8], bytes_per_pixel: u32) {
    gpu.queue.write_texture(
        tex.as_image_copy(),
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(N * bytes_per_pixel),
            rows_per_image: Some(N),
        },
        wgpu::Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
    );
}

/// A Depth32Float texture holding `data`, produced through a render pass
/// (depth32 is not a texture-copy destination).
fn depth_texture(gpu: &Gpu, label: &str, data: &[f32]) -> wgpu::Texture {
    let device = &gpu.device;
    let source = texture(
        device,
        "depth source",
        wgpu::TextureFormat::R32Float,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    upload(gpu, &source, bytemuck::cast_slice(data), 4);
    let depth = texture(
        device,
        label,
        wgpu::TextureFormat::Depth32Float,
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("depth fill"),
        source: wgpu::ShaderSource::Wgsl(shader_source::depth().into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_depth_only"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let view = source.create_view(&Default::default());
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    let depth_view = depth.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    gpu.queue.submit([encoder.finish()]);
    depth
}

fn read_texture(
    gpu: &Gpu,
    tex: &wgpu::Texture,
    bytes_per_pixel: u32,
    aspect: wgpu::TextureAspect,
) -> Vec<u8> {
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(N * N * bytes_per_pixel),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(N * bytes_per_pixel),
                rows_per_image: Some(N),
            },
        },
        wgpu::Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    map(gpu, &staging)
}

fn read_buffer(gpu: &Gpu, src: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(src, 0, &staging, 0, size);
    gpu.queue.submit([encoder.finish()]);
    map(gpu, &staging)
}

fn map(gpu: &Gpu, staging: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().unwrap().unwrap();
    let data = staging
        .slice(..)
        .get_mapped_range()
        .expect("mapped")
        .to_vec();
    staging.unmap();
    data
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x3ff) as f32;
    match exp {
        0 => sign * mant * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + mant / 1024.0) * 2f32.powi(exp - 15),
    }
}

fn f16s(bytes: &[u8], pixel: usize) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (i, o) in out.iter_mut().enumerate() {
        let at = pixel * 8 + i * 2;
        *o = f16_to_f32(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
    }
    out
}

fn put_u32(page: &mut [u8], at: usize, v: u32) {
    page[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(page: &mut [u8], at: usize, v: f32) {
    page[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Attribute rows (16 B per vertex): normal (0,0,1) snorm8x4, tangent (1,0,0)
/// with bitangent sign +, uv0 f16x2, uv1 zero.
fn attr_rows(uv: [[u16; 2]; 3]) -> Vec<[u8; 16]> {
    let mut rows = Vec::new();
    for i in 0..3 {
        let mut r = [0u8; 16];
        r[0..4].copy_from_slice(&(127u32 << 16).to_le_bytes());
        r[4..8].copy_from_slice(&(127u32 | (127 << 24)).to_le_bytes());
        r[8..12]
            .copy_from_slice(&(u32::from(uv[i][0]) | (u32::from(uv[i][1]) << 16)).to_le_bytes());
        rows.push(r);
    }
    rows
}

fn object_row(material_slot: u32) -> Vec<u32> {
    let mut row = vec![0u32; 63];
    row[2] = material_slot;
    let identity = Mat4::IDENTITY.to_cols_array();
    for i in 0..16 {
        row[4 + i] = identity[i].to_bits();
        row[20 + i] = identity[i].to_bits();
    }
    // normal_mat rows (vec4 each)
    for (r, base) in [36usize, 40, 44].into_iter().enumerate() {
        row[base + r] = 1.0f32.to_bits();
    }
    row
}

fn material(base_color: [f32; 4], class: u32) -> [u32; 24] {
    let mut m = [0u32; 24];
    for (i, v) in base_color.iter().enumerate() {
        m[i] = v.to_bits();
    }
    m[8] = 0.5f32.to_bits(); // roughness
    m[9] = 0.1f32.to_bits(); // metallic
    m[10] = 1.5f32.to_bits();
    m[11] = 0.5f32.to_bits();
    for slot in &mut m[12..17] {
        *slot = u32::MAX; // NO_TEXTURE
    }
    m[19] = class;
    m
}

#[test]
fn resolve_bin_shade_and_final_depth() {
    let Some(gpu) = gpu() else { return };
    let device = &gpu.device;
    let init = |label: &str, data: &[u8], usage: wgpu::BufferUsages| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: data,
            usage,
        })
    };
    let storage = wgpu::BufferUsages::STORAGE;

    // ── camera: at the origin, looking down -Z ──
    let (w, h) = (N as f32, N as f32);
    let view = Mat4::IDENTITY;
    let proj = helio_core::depth::perspective(60f32.to_radians(), 1.0, 0.1);
    let vp = proj * view;
    let cam = helio_core::GpuCameraUniforms {
        view: view.to_cols_array(),
        proj: proj.to_cols_array(),
        view_proj: vp.to_cols_array(),
        inv_view_proj: vp.inverse().to_cols_array(),
        position_near: [0.0, 0.0, 0.0, 0.1],
        forward_far: [0.0, 0.0, -1.0, 100.0],
        jitter_frame: [0.0; 4],
        prev_view_proj: vp.to_cols_array(),
    };
    let camera = init("camera", bytemuck::cast_slice(&[cam, cam]), storage);

    // ── scene: two triangles in two material sections of one instance ──
    let tri_a = [
        Vec3::new(0.1, -1.0, -2.5),
        Vec3::new(1.6, -0.9, -3.5),
        Vec3::new(0.3, 1.1, -3.0),
    ];
    let tri_b = [
        Vec3::new(-1.7, -1.0, -3.2),
        Vec3::new(-0.2, -1.1, -2.6),
        Vec3::new(-0.9, 1.2, -3.0),
    ];
    let one = 0x3c00u16;
    let pos = |t: &[Vec3; 3]| t.iter().map(|v| v.to_array()).collect::<Vec<[f32; 3]>>();
    let (pa, pb) = (pos(&tri_a), pos(&tri_b));
    let uvs = [[0, 0], [one, 0], [0, one]];
    let (aa, ab) = (attr_rows(uvs), attr_rows(uvs));
    let page = helio_vg_format::testing::TestPage::build(
        0,
        &[
            helio_vg_format::testing::TestCluster {
                positions: &pa,
                tris: &[[0, 1, 2]],
                material: 0,
                attrs: Some(&aa),
            },
            helio_vg_format::testing::TestCluster {
                positions: &pb,
                tris: &[[0, 1, 2]],
                material: 1,
                attrs: Some(&ab),
            },
        ],
    );
    let pages = init("pages", &page.pool(), storage);
    let asset_quant = init("asset_quant", bytemuck::bytes_of(&page.quant), storage);
    let instances = init(
        "instances",
        bytemuck::bytes_of(&GpuVgInstance {
            asset_slot: 0,
            flags: 1,
            lod_error_scale: 1.0,
            object_row: 0,
        }),
        storage,
    );
    let visible = init(
        "visible",
        bytemuck::cast_slice(&[
            VgVisibleCluster {
                instance: 0,
                cluster_slot: page.slots[0],
            },
            VgVisibleCluster {
                instance: 0,
                cluster_slot: page.slots[1],
            },
        ]),
        storage,
    );
    let mut rows = object_row(0);
    rows.extend(object_row(1));
    let static_objects = init("static_objects", bytemuck::cast_slice(&rows), storage);
    let mut mat_a = material([0.8, 0.4, 0.2, 1.0], 0);
    mat_a[12] = 0; // base colour texture 0 (white): exercises textureSampleGrad
    let mut materials_words = mat_a.to_vec();
    materials_words.extend(material([0.2, 0.6, 0.9, 1.0], 1)); // clear-coat class
    let materials = init("materials", bytemuck::cast_slice(&materials_words), storage);
    let mut spaces = vec![0f32; 256 * 16];
    spaces[..16].copy_from_slice(&Mat4::IDENTITY.to_cols_array());
    let coordinate_spaces = init("spaces", bytemuck::cast_slice(&spaces), storage);

    let binding = helio_mats::MaterialBindingConfig::for_device(device);
    let mut fallback = [0u32; 88];
    fallback[87] = (-1.0f32).to_bits();
    let material_textures = init(
        "material textures",
        bytemuck::cast_slice(&fallback),
        storage,
    );
    let white = texture(
        device,
        "white",
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    upload(&gpu, &white, &vec![255u8; (N * N * 4) as usize], 4);
    let white_view = white.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let views: Vec<&wgpu::TextureView> = (0..binding.max_textures).map(|_| &white_view).collect();
    let samplers: Vec<&wgpu::Sampler> = (0..binding.max_textures).map(|_| &sampler).collect();

    // ── raster results, simulated on the CPU ──
    struct Expect {
        id: u32,
        bary: [f32; 3],
        depth: f32,
        tri: usize,
    }
    let inv = vp.inverse();
    let mut expect: Vec<Option<Expect>> = (0..N * N).map(|_| None).collect();
    let mut hw_vis = vec![u32::MAX; (N * N) as usize];
    let mut hw_depth = vec![0f32; (N * N) as usize];
    let mut sw_keys = vec![0u32; (N * N * 2) as usize];
    let mut classic = vec![0f32; (N * N) as usize];
    for y in 0..N {
        for x in 0..N {
            let i = (y * N + x) as usize;
            if y >= HIDDEN_FROM_ROW {
                classic[i] = CLASSIC_DEPTH;
            }
            let ndc =
                cpu_ref::pixel_ndc(Vec2::new(x as f32 + 0.5, y as f32 + 0.5), Vec2::new(w, h));
            let p = inv * Vec4::new(ndc.x, ndc.y, 0.5, 1.0);
            let dir = (p.truncate() / p.w).normalize();
            for (t, tri) in [tri_a, tri_b].iter().enumerate() {
                let Some(r) = cpu_ref::bary_from_ray(*tri, Vec3::ZERO, dir) else {
                    continue;
                };
                if r.iter().any(|v| *v < 1e-3) {
                    continue;
                }
                let c = tri.map(|v| cpu_ref::clip(view, proj, v));
                let depth = cpu_ref::depth_at(c, r);
                let id = vis_pack(t as u32, 0);
                if t == 0 {
                    hw_vis[i] = id;
                    hw_depth[i] = depth;
                } else {
                    sw_keys[i * 2] = id;
                    sw_keys[i * 2 + 1] = depth.to_bits();
                }
                if y < HIDDEN_FROM_ROW {
                    expect[i] = Some(Expect {
                        id,
                        bary: r,
                        depth,
                        tri: t,
                    });
                }
            }
        }
    }
    let covered = expect.iter().flatten().count();
    let in_a = expect.iter().flatten().filter(|e| e.tri == 0).count();
    let in_b = covered - in_a;
    assert!(
        in_a > 100 && in_b > 100,
        "scene must cover both triangles: {in_a} / {in_b}"
    );

    let hw_vis_tex = texture(
        device,
        "hw vis",
        wgpu::TextureFormat::R32Uint,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    upload(&gpu, &hw_vis_tex, bytemuck::cast_slice(&hw_vis), 4);
    let hw_depth_tex = depth_texture(&gpu, "hw depth", &hw_depth);
    let scene_depth_tex = depth_texture(&gpu, "scene depth", &classic);
    let sw_buffer = init("sw keys", bytemuck::cast_slice(&sw_keys), storage);

    // ── targets ──
    let target = |label: &str, format| {
        texture(
            device,
            label,
            format,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        )
    };
    use wgpu::TextureFormat as F;
    let albedo = target("albedo", F::Rgba8Unorm);
    let normal = target("normal", F::Rgba16Float);
    let orm = target("orm", F::Rgba8Unorm);
    let emissive = target("emissive", F::Rgba16Float);
    let lightmap_uv = texture(
        device,
        "lightmap uv",
        F::Rg16Float,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let sss = target("sss", F::Rgba16Float);
    let extra = target("extra", F::Rgba16Float);
    let velocity = target("velocity", F::Rgba16Float);
    let v = |t: &wgpu::Texture| t.create_view(&Default::default());
    let (albedo_v, normal_v, orm_v, emissive_v, lm_v, sss_v, extra_v, velocity_v) = (
        v(&albedo),
        v(&normal),
        v(&orm),
        v(&emissive),
        v(&lightmap_uv),
        v(&sss),
        v(&extra),
        v(&velocity),
    );
    let hw_vis_v = v(&hw_vis_tex);
    let hw_depth_v = hw_depth_tex.create_view(&wgpu::TextureViewDescriptor {
        aspect: wgpu::TextureAspect::DepthOnly,
        ..Default::default()
    });
    let scene_depth_v = scene_depth_tex.create_view(&wgpu::TextureViewDescriptor {
        aspect: wgpu::TextureAspect::DepthOnly,
        ..Default::default()
    });

    eprintln!("material binding: {:?}", binding.mode);
    macro_rules! inputs {
        ($debug:expr) => {
            VgShadeInputs {
                camera: &camera,
                static_objects: &static_objects,
                coordinate_spaces: &coordinate_spaces,
                coordinate_spaces_prev: &coordinate_spaces,
                materials: &materials,
                material_textures: helio_mats::MaterialTextureBindings {
                    material_textures: &material_textures,
                    texture_views: &views,
                    samplers: &samplers,
                    version: 1,
                },
                visible: &visible,
                asset_quant: &asset_quant,
                instances: &instances,
                pages: &pages,
                hw_vis: &hw_vis_v,
                hw_depth: &hw_depth_v,
                scene_depth: &scene_depth_v,
                sw_keys: Some(&sw_buffer),
                dense_keys: None,
                rt: None,
                rt_visible: None,
                dense_mask: None,
                dense_merge: false,
                debug: $debug,
            }
        };
    }
    let mut pass = VgShade::new(device);
    let mut encoder = device.create_command_encoder(&Default::default());
    pass.encode(
        device,
        &gpu.queue,
        &mut encoder,
        N,
        N,
        &inputs!(None),
        &VgShadeTargets {
            albedo: &albedo_v,
            normal: &normal_v,
            orm: &orm_v,
            emissive: &emissive_v,
            lightmap_uv: &lm_v,
            sss: &sss_v,
            extra: &extra_v,
            velocity: &velocity_v,
            depth: &scene_depth_v,
        },
        &VgShadeGlobals {
            screen_width: w,
            screen_height: h,
            ..Default::default()
        },
    );
    gpu.queue.submit([encoder.finish()]);

    // ── resolve: payload, depth ──
    let payload = read_texture(
        &gpu,
        pass.payload_texture().unwrap(),
        8,
        wgpu::TextureAspect::All,
    );
    let resolved = read_texture(
        &gpu,
        pass.resolved_depth_texture().unwrap(),
        4,
        wgpu::TextureAspect::All,
    );
    let mut worst_bary = 0.0f32;
    for i in 0..(N * N) as usize {
        let id = u32::from_le_bytes(payload[i * 8..i * 8 + 4].try_into().unwrap());
        let bary_word = u32::from_le_bytes(payload[i * 8 + 4..i * 8 + 8].try_into().unwrap());
        let depth = f32::from_le_bytes(resolved[i * 4..i * 4 + 4].try_into().unwrap());
        match &expect[i] {
            Some(e) => {
                assert_eq!(id, e.id, "pixel {i}");
                assert_eq!(
                    depth, e.depth,
                    "pixel {i} resolved depth (merged raster depth)"
                );
                let b = cpu_ref::payload_unbary(bary_word);
                for k in 0..3 {
                    worst_bary = worst_bary.max((b[k] - e.bary[k]).abs());
                }
            }
            None => {
                assert_eq!(id, u32::MAX, "pixel {i} must be left to classic geometry");
                assert_eq!(depth, 0.0, "pixel {i}");
            }
        }
    }
    assert!(worst_bary < 1e-3, "barycentric error {worst_bary}");
    eprintln!("worst barycentric error on GPU: {worst_bary:e}");

    // ── resolve: the per-pixel source of each winner ──
    // Triangle A reached the resolve through the hardware vis target and triangle
    // B through the software keys; pixels left to classic geometry have no source.
    {
        use helio_vg_format::debug_view::pixel_source;
        let source = read_texture(
            &gpu,
            pass.source_texture().unwrap(),
            4,
            wgpu::TextureAspect::All,
        );
        let (mut hw, mut sw) = (0, 0);
        for i in 0..(N * N) as usize {
            let got = u32::from_le_bytes(source[i * 4..i * 4 + 4].try_into().unwrap());
            let want = match &expect[i] {
                Some(e) if e.tri == 0 => {
                    hw += 1;
                    pixel_source::HW
                }
                Some(_) => {
                    sw += 1;
                    pixel_source::SW
                }
                None => pixel_source::NONE,
            };
            assert_eq!(got, want, "pixel {i}: recorded source");
        }
        assert!(
            hw > 100 && sw > 100,
            "both sources must be exercised: {hw} hardware / {sw} software"
        );
    }

    // ── binning ──
    let bins = read_buffer(&gpu, pass.bins_buffer(), 16 * 16);
    let word = |b: &[u8], i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
    assert_eq!(word(&bins, 1) as usize, in_a, "bin 0 count");
    assert_eq!(word(&bins, 4 + 1) as usize, in_b, "bin 1 count");
    assert_eq!(word(&bins, 0), 0);
    assert_eq!(word(&bins, 4) as usize, in_a, "bin 1 starts after bin 0");
    let args = read_buffer(&gpu, pass.dispatch_args_buffer(), 16 * 16);
    assert_eq!(word(&args, 0) as usize, in_a.div_ceil(64));
    assert_eq!(word(&args, 4) as usize, in_b.div_ceil(64));
    assert_eq!(word(&args, 8), 0, "empty bin dispatches nothing");
    let list = read_buffer(
        &gpu,
        pass.pixel_list_buffer().unwrap(),
        u64::from(N * N * 4),
    );
    let mut seen = std::collections::HashSet::new();
    for slot in 0..(in_a + in_b) {
        let pixel = word(&list, slot) as usize;
        assert!(seen.insert(pixel), "pixel {pixel} listed twice");
        let want_bin = expect[pixel].as_ref().expect("listed pixel is VG").tri;
        assert_eq!(
            usize::from(slot >= in_a),
            want_bin,
            "pixel {pixel} in the wrong bin"
        );
    }

    // ── shaded G-buffer ──
    let albedo_px = read_texture(&gpu, &albedo, 4, wgpu::TextureAspect::All);
    let normal_px = read_texture(&gpu, &normal, 8, wgpu::TextureAspect::All);
    let orm_px = read_texture(&gpu, &orm, 4, wgpu::TextureAspect::All);
    let lm_px = read_texture(&gpu, &lightmap_uv, 4, wgpu::TextureAspect::All);
    let velocity_px = read_texture(&gpu, &velocity, 8, wgpu::TextureAspect::All);
    let near = |a: f32, b: f32, tol: f32, what: &str, i: usize| {
        assert!((a - b).abs() <= tol, "pixel {i} {what}: {a} vs {b}");
    };
    let (mut shaded_a, mut shaded_b) = (0, 0);
    for i in 0..(N * N) as usize {
        let rgba = &albedo_px[i * 4..i * 4 + 4];
        match &expect[i] {
            Some(e) if e.tri == 0 => {
                // class 0 (default PBR): base colour, +Z normal, roughness 0.5, metallic 0.1.
                for (k, want) in [0.8f32, 0.4, 0.2, 1.0].into_iter().enumerate() {
                    near(f32::from(rgba[k]) / 255.0, want, 1.5 / 255.0, "albedo", i);
                }
                let n = f16s(&normal_px, i);
                near(n[0], 0.0, 0.01, "normal.x", i);
                near(n[1], 0.0, 0.01, "normal.y", i);
                near(n[2], 1.0, 0.01, "normal.z", i);
                near(n[3], 0.04 + (0.8 - 0.04) * 0.1, 0.01, "F0.r", i);
                near(
                    f32::from(orm_px[i * 4 + 1]) / 255.0,
                    0.5,
                    1.5 / 255.0,
                    "roughness",
                    i,
                );
                near(
                    f32::from(orm_px[i * 4 + 2]) / 255.0,
                    0.1,
                    1.5 / 255.0,
                    "metallic",
                    i,
                );
                // Static scene, same view: no motion. Valid-correction marker set.
                let vel = f16s(&velocity_px, i);
                near(vel[0], 0.0, 0.02, "velocity.x", i);
                near(vel[1], 0.0, 0.02, "velocity.y", i);
                near(
                    f16_to_f32(u16::from_le_bytes([lm_px[i * 4], lm_px[i * 4 + 1]])),
                    -1.0,
                    0.001,
                    "lightmap u",
                    i,
                );
                shaded_a += 1;
            }
            Some(_) => {
                // class 1 (clear coat template): evaluated through its override.
                assert!(
                    rgba[3] > 0 && rgba[2] > 0,
                    "pixel {i} was not shaded by the class-1 pipeline"
                );
                shaded_b += 1;
            }
            None => assert_eq!(rgba, &[0, 0, 0, 0], "pixel {i}: stray G-buffer write"),
        }
    }
    assert_eq!((shaded_a, shaded_b), (in_a, in_b));

    // ── final depth: VG where it won, classic where it was nearer ──
    let final_depth = read_texture(&gpu, &scene_depth_tex, 4, wgpu::TextureAspect::DepthOnly);
    for i in 0..(N * N) as usize {
        let d = f32::from_le_bytes(final_depth[i * 4..i * 4 + 4].try_into().unwrap());
        match &expect[i] {
            Some(e) => assert_eq!(d, e.depth, "pixel {i} final depth"),
            None => assert_eq!(d, classic[i], "pixel {i} must keep the classic depth"),
        }
    }

    // ── debug views: every view runs clean and writes what it should ──
    let zeros = |label: &str, bytes: usize| init(label, &vec![0u8; bytes], storage);
    let residency = zeros("residency", 256);
    let nodes = zeros("nodes", 1024);
    let group_refs = zeros("group refs", 256);
    let asset_headers = zeros("asset headers", 256);
    let mut bases = [0u32; 4];
    bases[3] = 1;
    let asset_bases = init("asset bases", bytemuck::cast_slice(&bases), storage);
    let feedback = zeros("feedback", 256);
    let cull_state = zeros("cull state", 128 * 4);
    let tiles = N / 16;
    let mut counters = Vec::new();
    let mut mask = Vec::new();
    for t in 0..tiles * tiles {
        counters.extend_from_slice(&[1 + (t % 7) * 9, (t % 5) * 300]);
        mask.push(u32::from(t % 3 == 0));
    }
    let tile_counters = init("tile counters", bytemuck::cast_slice(&counters), storage);
    let dense_mask = init("dense mask", bytemuck::cast_slice(&mask), storage);
    let targets = VgShadeTargets {
        albedo: &albedo_v,
        normal: &normal_v,
        orm: &orm_v,
        emissive: &emissive_v,
        lightmap_uv: &lm_v,
        sss: &sss_v,
        extra: &extra_v,
        velocity: &velocity_v,
        depth: &scene_depth_v,
    };
    use helio_vg_format::debug_view::VgDebugView as D;
    // Distinct colours expected over the VG pixels (tile views and the mask:
    // over every pixel). The synthetic scene has one instance, equal normals
    // and no error, so those views are legitimately uniform.
    let expected_distinct = |view: D| match view {
        D::TriangleId | D::ClusterId | D::RasterTier | D::MaterialBin => 2,
        D::Barycentrics => 8,
        D::SwOverdraw | D::DenseOverlap | D::DenseMask | D::VgMask => 2,
        _ => 1,
    };
    for view in helio_vg_format::debug_view::ALL {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = device.create_command_encoder(&Default::default());
        pass.encode(
            device,
            &gpu.queue,
            &mut encoder,
            N,
            N,
            &inputs!(Some(helio_pass_vg_shade::VgShadeDebug {
                view,
                stream: helio_vg_stream::VgStreamBuffers {
                    page_pool: &pages,
                    residency: &residency,
                    nodes: &nodes,
                    group_refs: &group_refs,
                    asset_headers: &asset_headers,
                    asset_bases: &asset_bases,
                    asset_quant: &asset_quant,
                    feedback: &feedback,
                },
                cull_state: &cull_state,
                bin_capacity: 1,
                tile_counters: &tile_counters,
                dense_mask: &dense_mask,
                resolved_mask: None,
                tiles: [tiles, tiles],
                tau_px: 1.0,
            })),
            &targets,
            &VgShadeGlobals {
                screen_width: w,
                screen_height: h,
                ..Default::default()
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{view:?}: GPU validation error: {error:?}");
        let px = read_texture(&gpu, &albedo, 4, wgpu::TextureAspect::All);
        let whole_frame = matches!(
            view,
            D::SwOverdraw | D::DenseOverlap | D::DenseMask | D::VgMask
        );
        let colours: std::collections::HashSet<[u8; 4]> = (0..(N * N) as usize)
            .filter(|&i| whole_frame || expect[i].is_some())
            .map(|i| px[i * 4..i * 4 + 4].try_into().unwrap())
            .collect();
        assert!(
            colours.len() >= expected_distinct(view),
            "{view:?}: {} distinct colours, wanted >= {}",
            colours.len(),
            expected_distinct(view)
        );
        eprintln!("{view:?}: {} distinct colours", colours.len());
    }

    // The mask separates VG from classic pixels exactly.
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = device.create_command_encoder(&Default::default());
    pass.encode(
        device,
        &gpu.queue,
        &mut encoder,
        N,
        N,
        &inputs!(Some(helio_pass_vg_shade::VgShadeDebug {
            view: D::VgMask,
            stream: helio_vg_stream::VgStreamBuffers {
                page_pool: &pages,
                residency: &residency,
                nodes: &nodes,
                group_refs: &group_refs,
                asset_headers: &asset_headers,
                asset_bases: &asset_bases,
                asset_quant: &asset_quant,
                feedback: &feedback,
            },
            cull_state: &cull_state,
            bin_capacity: 1,
            tile_counters: &tile_counters,
            dense_mask: &dense_mask,
            resolved_mask: None,
            tiles: [tiles, tiles],
            tau_px: 1.0,
        })),
        &targets,
        &VgShadeGlobals {
            screen_width: w,
            screen_height: h,
            ..Default::default()
        },
    );
    gpu.queue.submit([encoder.finish()]);
    assert!(pollster::block_on(scope.pop()).is_none());
    let px = read_texture(&gpu, &albedo, 4, wgpu::TextureAspect::All);
    for i in 0..(N * N) as usize {
        let rgba = &px[i * 4..i * 4 + 4];
        if expect[i].is_some() {
            assert_eq!(rgba, &[38, 217, 89, 255], "pixel {i} is VG");
        } else {
            assert_eq!(rgba, &[46, 46, 46, 255], "pixel {i} is classic");
        }
    }

    // "Ray Trace vs Raster" (renderer mode 33): VG pixels are painted from the
    // source the resolve recorded (hardware orange, compute micro-raster yellow);
    // everything else is left for the lighting pass (its albedo is untouched).
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = device.create_command_encoder(&Default::default());
    pass.encode(
        device,
        &gpu.queue,
        &mut encoder,
        N,
        N,
        &inputs!(Some(helio_pass_vg_shade::VgShadeDebug {
            view: D::RayVsRaster,
            stream: helio_vg_stream::VgStreamBuffers {
                page_pool: &pages,
                residency: &residency,
                nodes: &nodes,
                group_refs: &group_refs,
                asset_headers: &asset_headers,
                asset_bases: &asset_bases,
                asset_quant: &asset_quant,
                feedback: &feedback,
            },
            cull_state: &cull_state,
            bin_capacity: 1,
            tile_counters: &tile_counters,
            dense_mask: &dense_mask,
            resolved_mask: None,
            tiles: [tiles, tiles],
            tau_px: 1.0,
        })),
        &targets,
        &VgShadeGlobals {
            screen_width: w,
            screen_height: h,
            ..Default::default()
        },
    );
    gpu.queue.submit([encoder.finish()]);
    assert!(pollster::block_on(scope.pop()).is_none());
    let px = read_texture(&gpu, &albedo, 4, wgpu::TextureAspect::All);
    // Painted pixels carry albedo alpha 0, the marker the lighting pass looks for;
    // a classic pixel keeps its own albedo and alpha.
    let near = |got: &[u8], want: [u8; 3]| {
        got[..3].iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1) && got[3] == 0
    };
    for i in 0..(N * N) as usize {
        let rgba = &px[i * 4..i * 4 + 4];
        match &expect[i] {
            Some(e) if e.tri == 0 => assert!(
                near(rgba, [255, 92, 20]),
                "pixel {i}: hardware raster {rgba:?}"
            ),
            Some(_) => assert!(
                near(rgba, [255, 217, 26]),
                "pixel {i}: micro-raster {rgba:?}"
            ),
            None => assert_eq!(
                rgba,
                &[46, 46, 46, 255],
                "pixel {i}: left to the lighting pass"
            ),
        }
    }
}
