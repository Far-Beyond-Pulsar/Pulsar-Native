//! Does the caustics projection land the sun where the waves send it?
//!
//! The caustics are area-ratio splats: a grid over the water's source cells
//! is refracted by the ocean slopes and drawn additively at where its light
//! lands, each fragment the ratio of its source area to its landed area. So
//! flat water lands 1 everywhere, and waves focus light (above 1) and spread
//! it (below 1) while the light that lands stays the light that fell. This
//! drives the pass's own projection (`pipeline::CausticsProjection`, its
//! grid and its near slice's parameters) over synthetic slopes and reads the
//! slice back.

use helio_pass_water_sim::pipeline::CausticsProjection;
use wgpu::util::DeviceExt;

/// The near slice's source cells and landing margin (`WaterSimPass::new`).
const CELLS: u32 = 128;
const MARGIN: u32 = 6;
const TARGET: u32 = 512;
/// The slope layer the projection reads (`SLOPE_CASCADE` in caustics.wgsl).
const SLOPE_LAYER: u32 = 2;
const SLOPES: u32 = 128;

fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x007f_ffff;
    if exp <= 0 {
        return sign;
    }
    if exp >= 0x1f {
        return sign | 0x7c00;
    }
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

/// Slopes (x, z) of a tileable ripple field of `slope` peak steepness, in
/// the slope layer of a three-layer array (`Rgba16Float`, as the ocean's).
fn slopes(slope: f32) -> Vec<u16> {
    let n = SLOPES as usize;
    let k = std::f32::consts::TAU * 4.0 / n as f32;
    let mut texels = vec![0u16; n * n * 4 * (SLOPE_LAYER as usize + 1)];
    let layer = n * n * 4 * SLOPE_LAYER as usize;
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 * k, y as f32 * k);
            let i = layer + (y * n + x) * 4;
            texels[i] = f32_to_f16(slope * fx.cos());
            texels[i + 1] = f32_to_f16(slope * 0.7 * (fy + 0.5 * fx).cos());
        }
    }
    texels
}

/// The `indoor_cathedral_water` pool, the sun high over it.
fn pool_volume() -> helio_pass_water_sim::GpuWaterVolume {
    let sun = {
        let (x, y, z) = (0.3f32, 1.0f32, 0.2f32);
        let l = (x * x + y * y + z * z).sqrt();
        [x / l, y / l, z / l, 0.0]
    };
    helio_pass_water_sim::GpuWaterVolume {
        bounds_min: [-6.0, 0.3, -6.0, 0.0],
        bounds_max: [6.0, 2.5, 6.0, 1.8],
        wave_params: [0.1, 0.75, 3.2, 0.22],
        wave_direction: [0.6, 0.3, 0.0, 0.0],
        water_color: [0.05, 0.20, 0.30, 0.76],
        extinction: [0.08, 0.04, 0.02, 0.45],
        reflection_refraction: [0.65, 1.0, 5.0, 0.0],
        caustics_params: [1.0, 1.0, 8.0, 0.0],
        fog_params: [0.0, 0.5, 0.0, 0.0],
        sim_params: [1.333, 1.0, 0.1, 0.03],
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
    mean: f32,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "min={:.3} max={:.3} mean={:.3}",
            self.min, self.max, self.mean
        )
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Water Caustics Test Device"),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()?;
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        panic!("water caustics GPU validation error: {error:?}");
    }));
    Some(Gpu { device, queue })
}

/// The near slice over ripples of `slope` steepness: its landed light (the R
/// channel) over the target's interior.
fn project(gpu: &Gpu, slope: f32) -> Stats {
    let Gpu { device, queue } = gpu;
    let projection = CausticsProjection::new(device);

    let slopes_texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("Test Slopes"),
            size: wgpu::Extent3d {
                width: SLOPES,
                height: SLOPES,
                depth_or_array_layers: SLOPE_LAYER + 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&slopes(slope)),
    );
    let slopes_view = slopes_texture.create_view(&wgpu::TextureViewDescriptor {
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
        contents: bytemuck::bytes_of(&pool_volume()),
        usage: wgpu::BufferUsages::STORAGE,
    });
    // The near slice's parameters (`WaterSimPass::new`), at slope mip 0.
    let slice = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Test Slice"),
        contents: bytemuck::cast_slice(&[
            1.5f32,
            CELLS as f32,
            0.0,
            MARGIN as f32 / CELLS as f32,
            TARGET as f32,
            0.0,
            0.0,
            0.0,
        ]),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Test Caustics"),
        layout: &projection.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: volumes.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&slopes_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: slice.as_entire_binding(),
            },
        ],
    });
    let (vertices, indices, index_count) =
        helio_pass_water_sim::make_caustics_grid(device, CELLS, MARGIN);

    let target = |label, samples, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: TARGET,
                height: TARGET,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage,
            view_formats: &[],
        })
    };
    let msaa = target(
        "Test Caustics MSAA",
        4,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let resolved = target(
        "Test Caustics",
        1,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let row_bytes = TARGET * 8;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Test Readback"),
        size: (row_bytes * TARGET) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let msaa_view = msaa.create_view(&Default::default());
        let resolved_view = resolved.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Test Caustics"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa_view,
                resolve_target: Some(&resolved_view),
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&projection.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..index_count, 0, 0..1);
    }
    encoder.copy_texture_to_buffer(
        resolved.as_image_copy(),
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

    // The interior: the landing margin keeps the edges' light partial.
    let edge = TARGET / 8;
    let (mut min, mut max, mut sum, mut count) = (f32::INFINITY, f32::NEG_INFINITY, 0.0f64, 0u32);
    for y in edge..TARGET - edge {
        for x in edge..TARGET - edge {
            let r = f16_to_f32(halves[((y * TARGET + x) * 4) as usize]);
            min = min.min(r);
            max = max.max(r);
            sum += f64::from(r);
            count += 1;
        }
    }
    Stats {
        min,
        max,
        mean: (sum / f64::from(count)) as f32,
    }
}

#[test]
fn caustics_focus_the_light_that_falls_and_keep_it() {
    let Some(gpu) = gpu() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER");
        return;
    };
    let flat = project(&gpu, 0.0);
    let waves = project(&gpu, 0.12);
    eprintln!("caustics, flat water: {flat}");
    eprintln!("caustics, waves:      {waves}");

    // Flat water lands every source cell on itself: the light unchanged.
    assert!(
        (flat.min - 1.0).abs() < 0.02 && (flat.max - 1.0).abs() < 0.02,
        "flat water must land 1 everywhere: {flat}"
    );
    // Waves focus some light and spread the rest...
    assert!(
        waves.max > 1.2 && waves.min < 0.85,
        "waves must focus and spread the light: {waves}"
    );
    // ...and keep it: what lands is what fell.
    assert!(
        (waves.mean - 1.0).abs() < 0.05,
        "the light that lands must be the light that fell: {waves}"
    );
}
