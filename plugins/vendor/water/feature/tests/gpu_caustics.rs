//! Does the caustics projection actually put signal in its target?
//!
//! The pass renders into an offscreen 256x256 texture that nothing displays
//! directly, so "no caustics on screen" is ambiguous between the projection
//! producing nothing and the consumer dropping it. This runs the projection
//! alone against a heightfield with known ripples and reads the result back, so
//! the two cases are distinguishable.

use wgpu::util::DeviceExt;

const SIM: u32 = 256;
const TARGET: u32 = 256;
const DETAIL: u32 = 128;

// ── f16, because the sim and caustics targets are Rgba16Float ────────────────

fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let mut exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x007f_ffff;

    if exp <= 0 {
        return sign;
    }
    if exp >= 0x1f {
        return sign | 0x7c00;
    }
    exp = exp.clamp(0, 0x1e);
    sign | ((exp as u16) << 10) | ((mantissa >> 13) as u16)
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let mantissa = (bits & 0x03ff) as u32;

    if exp == 0 {
        if mantissa == 0 {
            return f32::from_bits(sign);
        }
        // Subnormal: normalize it.
        let shift = mantissa.leading_zeros() - 21;
        let e = 127 - 15 - shift;
        let m = (mantissa << (shift + 1)) & 0x03ff;
        return f32::from_bits(sign | (e << 23) | (m << 13));
    }
    if exp == 0x1f {
        return f32::from_bits(sign | 0x7f80_0000 | (mantissa << 13));
    }
    f32::from_bits(sign | ((exp + 127 - 15) << 23) | (mantissa << 13))
}

/// A heightfield with ripples, encoded the way the sim pass writes it:
/// R = height, G = velocity, B/A = normal.xz in *normalized sim space*.
fn synthetic_heightfield(wavelength_texels: f32, amplitude: f32) -> Vec<u16> {
    let n = SIM as usize;
    let mut height = vec![0.0f32; n * n];
    let k = std::f32::consts::TAU / wavelength_texels;
    for y in 0..n {
        for x in 0..n {
            // Two crossing wave trains, so the field focuses in 2D rather than
            // being a pure 1D ridge.
            height[y * n + x] = amplitude
                * ((x as f32 * k).sin() + 0.7 * ((y as f32 * 0.8 + x as f32 * 0.3) * k).sin());
        }
    }

    let delta = 1.0 / SIM as f32;
    let mut texels = vec![0u16; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let h = height[y * n + x];
            let h_right = height[y * n + (x + 1) % n];
            let h_up = height[((y + 1) % n) * n + x];

            // Matches normal.frag.wgsl: cross(tangent_z, tangent_x), normalized.
            let tx = [delta, h_right - h, 0.0f32];
            let tz = [0.0f32, h_up - h, delta];
            let nx = tz[1] * tx[2] - tz[2] * tx[1];
            let ny = tz[2] * tx[0] - tz[0] * tx[2];
            let nz = tz[0] * tx[1] - tz[1] * tx[0];
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-9);

            let i = (y * n + x) * 4;
            texels[i] = f32_to_f16(h);
            texels[i + 1] = f32_to_f16(0.0);
            texels[i + 2] = f32_to_f16(nx / len);
            texels[i + 3] = f32_to_f16(nz / len);
        }
    }
    texels
}

/// The top-face grid `make_caustics_grid` builds: `w` = 0 marks a top-face
/// vertex (the shader discards the rest).
fn grid_mesh() -> (Vec<[f32; 4]>, Vec<u32>) {
    let n = DETAIL + 1;
    let mut verts = Vec::new();
    for j in 0..n {
        for i in 0..n {
            verts.push([
                i as f32 / DETAIL as f32 * 2.0 - 1.0,
                j as f32 / DETAIL as f32 * 2.0 - 1.0,
                0.0,
                0.0,
            ]);
        }
    }
    let mut indices = Vec::new();
    for j in 0..DETAIL {
        for i in 0..DETAIL {
            let (tl, tr) = (j * n + i, j * n + i + 1);
            let (bl, br) = ((j + 1) * n + i, (j + 1) * n + i + 1);
            indices.extend_from_slice(&[tl, bl, tr, tr, bl, br]);
        }
    }
    (verts, indices)
}

/// The `indoor_cathedral_water` pool, which is the case that looked empty.
fn pool_volume(
    wave_amplitude: f32,
    caustics_intensity: f32,
) -> helio_pass_water_sim::GpuWaterVolume {
    let sun = {
        let (x, y, z) = (0.5f32, 1.0f32, 0.5f32);
        let l = (x * x + y * y + z * z).sqrt();
        [x / l, y / l, z / l, 0.0]
    };
    helio_pass_water_sim::GpuWaterVolume {
        bounds_min: [-6.0, 0.3, -6.0, 0.0],
        bounds_max: [6.0, 2.5, 6.0, 1.8],
        wave_params: [wave_amplitude, 0.75, 3.2, 0.22],
        wave_direction: [0.6, 0.3, 0.0, 0.0],
        water_color: [0.05, 0.20, 0.30, 0.76],
        extinction: [0.08, 0.04, 0.02, 0.45],
        reflection_refraction: [0.65, 1.0, 5.0, 0.0],
        caustics_params: [1.0, caustics_intensity, 8.0, 0.0],
        fog_params: [0.0, 0.5, 0.0, 0.0],
        sim_params: [1.333, caustics_intensity, 0.1, 0.03],
        shadow_params: [1.0, 0.0, 1.0, 0.0],
        sun_direction: sun,
        ssr_params: [1.0, 64.0, 0.05, 0.02],
        sim_dynamics: [1.2, 0.985, 1.0, 0.0],
        wind_params: [0.0, 0.0, 0.0, 0.0],
        world_phase: [0.0; 4],
    }
}

struct Stats {
    min: f32,
    max: f32,
    mean_abs: f32,
    nonzero_fraction: f32,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "min={:.4} max={:.4} mean|v|={:.4} nonzero={:.1}%",
            self.min,
            self.max,
            self.mean_abs,
            self.nonzero_fraction * 100.0
        )
    }
}

async fn project_caustics(wave_amplitude: f32, caustics_intensity: f32) -> Option<Stats> {
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
            label: Some("Water Caustics Test Device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .expect("adapter must create a device");
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        panic!("water caustics GPU validation error: {error:?}");
    }));

    // ── Sim heightfield ─────────────────────────────────────────────────────
    let sim_tex = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: Some("Test Sim"),
            size: wgpu::Extent3d {
                width: SIM,
                height: SIM,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&synthetic_heightfield(24.0, 0.6)),
    );
    // One layer per water volume, as `WaterSimPass` allocates it.
    let sim_view = sim_tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        ..Default::default()
    });

    let volumes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Test Volumes"),
        contents: bytemuck::bytes_of(&pool_volume(wave_amplitude, caustics_intensity)),
        usage: wgpu::BufferUsages::STORAGE,
    });

    // ── Pipeline, matching WaterSimPass::new ────────────────────────────────
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Test Caustics BGL"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Test Caustics BG"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: volumes.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&sim_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    let shader = helio_core::shader::module(
        &device,
        "Test Caustics Shader",
        include_str!("../shaders/caustics.wgsl"),
    );
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Test Caustics PL"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Test Caustics Pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                }],
            })],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });

    let (verts, indices) = grid_mesh();
    let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Test VB"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Test IB"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Test Caustics Target"),
        size: wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());

    // 256 texels * 8 bytes = 2048, already a multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
    let row_bytes = TARGET * 8;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Test Readback"),
        size: (row_bytes * TARGET) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Test Caustics"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(TARGET),
            },
        },
        wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().expect("map callback").expect("map succeeded");

    let data = slice.get_mapped_range().expect("get_mapped_range");
    let halves: &[u16] = bytemuck::cast_slice(&data);

    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut sum_abs = 0.0f64;
    let mut nonzero = 0usize;
    let count = (TARGET * TARGET) as usize;
    for i in 0..count {
        let r = f16_to_f32(halves[i * 4]);
        min = min.min(r);
        max = max.max(r);
        sum_abs += r.abs() as f64;
        if r.abs() > 1e-3 {
            nonzero += 1;
        }
    }
    drop(data);
    readback.unmap();

    Some(Stats {
        min,
        max,
        mean_abs: (sum_abs / count as f64) as f32,
        nonzero_fraction: nonzero as f32 / count as f32,
    })
}

#[test]
fn caustics_projection_writes_signal_for_the_cathedral_pool() {
    pollster::block_on(async {
        // The demo's amplitude, which is what looked empty on screen.
        let Some(shallow) = project_caustics(0.035, 1.0).await else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER");
            return;
        };
        // An amplitude that unambiguously focuses light, as a control: if this
        // one is also flat, the projection is broken rather than merely weak.
        let steep = project_caustics(0.35, 1.0)
            .await
            .expect("adapter was available a moment ago");

        eprintln!("caustics @ amplitude 0.035 (demo): {shallow}");
        eprintln!("caustics @ amplitude 0.35  (control): {steep}");

        assert!(
            steep.mean_abs > 1e-3,
            "the projection produced no signal even for a steep surface, so it is \
             broken rather than weak: {steep}"
        );
        assert!(
            shallow.mean_abs > 1e-4,
            "the projection produced nothing at the demo's amplitude ({shallow}), \
             while the steep control worked ({steep})"
        );
    });
}
