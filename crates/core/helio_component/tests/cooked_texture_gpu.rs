//! Cook, upload and sample: what a shader reads from a cooked texture matches
//! the colours that went in, for the uncompressed and the BC7 forms, at the
//! base level and down the mip chain.
//!
//! Needs a GPU adapter; skips without one (BC7 also needs BC support).

use helio_component::cooked_texture::{cook, CookFormat, CookedTexture};
use pulsar_scenedb::gpu::TextureStore;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    has_bc: bool,
}

fn gpu() -> Option<Gpu> {
    // Creating devices from many tests at once stalls some drivers; take turns.
    static DEVICE_CREATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _device_creation = DEVICE_CREATION.lock().unwrap_or_else(|p| p.into_inner());
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let has_bc = adapter.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: if has_bc {
            wgpu::Features::TEXTURE_COMPRESSION_BC
        } else {
            wgpu::Features::empty()
        },
        ..Default::default()
    }))
    .ok()?;
    Some(Gpu { device, queue, has_bc })
}

const SHADER: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> out: array<vec4<f32>>;

@compute @workgroup_size(1)
fn main() {
    let levels = i32(textureNumLevels(tex));
    for (var level = 0; level < levels; level = level + 1) {
        out[level] = textureLoad(tex, vec2<i32>(0, 0), level);
    }
}
"#;

/// Texel (0, 0) of every level, as the shader decodes it (linear light).
fn sample_levels(gpu: &Gpu, cooked: &CookedTexture) -> Vec<[f32; 4]> {
    let desc = wgpu::TextureDescriptor {
        label: Some("cooked"),
        size: wgpu::Extent3d { width: cooked.width, height: cooked.height, depth_or_array_layers: 1 },
        mip_level_count: cooked.mip_count(),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: cooked.wgpu_format(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    let mut store = TextureStore::new(1);
    let slot = store
        .register_mips(&gpu.device, &gpu.queue, &desc, &cooked.mips())
        .expect("upload");
    let view = store.texture(slot).unwrap().create_view(&Default::default());

    let levels = cooked.mip_count() as u64;
    let size = levels * 16;
    let storage = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("out"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let module = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sample-levels"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = gpu.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("sample-levels"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: storage.as_entire_binding() },
        ],
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&storage, 0, &staging, 0, size);
    gpu.queue.submit([encoder.finish()]);
    staging.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = staging.slice(..).get_mapped_range().expect("range");
    let floats: &[f32] = bytemuck::cast_slice(&mapped);
    floats.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect()
}

fn srgb_to_linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn solid(width: u32, height: u32, texel: [u8; 4]) -> Vec<u8> {
    (0..width * height).flat_map(|_| texel).collect()
}

fn close(actual: [f32; 4], expected: [f32; 4], tolerance: f32) -> bool {
    actual.iter().zip(expected).all(|(a, e)| (a - e).abs() <= tolerance)
}

fn check_solid_colour(gpu: &Gpu, format: CookFormat, tolerance: f32) {
    let texel = [200u8, 100, 50, 255];
    let cooked = cook(&solid(8, 8, texel), 8, 8, format).unwrap();
    let expected = [
        srgb_to_linear(texel[0]),
        srgb_to_linear(texel[1]),
        srgb_to_linear(texel[2]),
        1.0,
    ];
    let levels = sample_levels(gpu, &cooked);
    assert_eq!(levels.len(), 4);
    for (level, sampled) in levels.iter().enumerate() {
        assert!(
            close(*sampled, expected, tolerance),
            "{} level {level}: shader read {sampled:?}, expected about {expected:?}",
            format.label()
        );
    }
}

#[test]
fn an_uncompressed_cook_samples_back_exactly_down_the_chain() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    check_solid_colour(&gpu, CookFormat::Rgba8, 0.003);
}

#[test]
fn a_bc7_cook_samples_back_close_to_the_source_down_the_chain() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    if !gpu.has_bc {
        eprintln!("adapter has no TEXTURE_COMPRESSION_BC; skipping");
        return;
    }
    // BC7 stores endpoints at 7 or 8 bits plus interpolation; a flat colour
    // should come back within a couple of output steps.
    check_solid_colour(&gpu, CookFormat::Bc7, 0.02);
}

#[test]
fn the_top_of_a_bc7_chain_is_the_average_light_of_a_checkerboard() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    if !gpu.has_bc {
        eprintln!("adapter has no TEXTURE_COMPRESSION_BC; skipping");
        return;
    }
    let mut checker = Vec::new();
    for y in 0..8u32 {
        for x in 0..8u32 {
            let on = (x + y) % 2 == 0;
            checker.extend_from_slice(if on { &[255, 255, 255, 255] } else { &[0, 0, 0, 255] });
        }
    }
    let cooked = cook(&checker, 8, 8, CookFormat::Bc7).unwrap();
    let levels = sample_levels(&gpu, &cooked);
    // 1x1 level: half the light, whatever the block does with the contrast below it.
    let top = levels[3];
    assert!(close(top, [0.5, 0.5, 0.5, 1.0], 0.06), "1x1 level read {top:?}");
}

/// A roughness map, an occlusion map or a normal map holds numbers. They must
/// come back from the shader as the bytes that went in divided by 255, and not
/// be bent by an sRGB decode.
/// Arbitrary numbers, as a roughness or occlusion map might hold.
const DATA_TEXEL: [u8; 4] = [200, 100, 50, 255];
/// A unit-length tangent-space normal (0.6, 0, 0.8): a normal map's mips are
/// renormalised, so the texel must be a real normal for the numbers to stay.
const NORMAL_TEXEL: [u8; 4] = [204, 128, 230, 255];

fn check_data_numbers(
    gpu: &Gpu,
    format: CookFormat,
    usage: helio_component::cooked_texture::TextureUse,
    texel: [u8; 4],
    tolerance: f32,
) {
    let cooked = helio_component::cooked_texture::cook_as(&solid(8, 8, texel), 8, 8, format, usage).unwrap();
    let expected = [
        f32::from(texel[0]) / 255.0,
        f32::from(texel[1]) / 255.0,
        f32::from(texel[2]) / 255.0,
        1.0,
    ];
    let colour_would_read = [
        srgb_to_linear(texel[0]),
        srgb_to_linear(texel[1]),
        srgb_to_linear(texel[2]),
        1.0,
    ];
    let levels = sample_levels(gpu, &cooked);
    assert_eq!(levels.len(), 4);
    for (level, sampled) in levels.iter().enumerate() {
        assert!(
            close(*sampled, expected, tolerance),
            "{} {} level {level}: shader read {sampled:?}, expected the raw numbers {expected:?}",
            format.label(),
            usage.label()
        );
        assert!(
            !close(*sampled, colour_would_read, 0.05),
            "{} level {level} was gamma-decoded like a colour: {sampled:?}",
            format.label()
        );
    }
}

#[test]
fn data_and_normal_textures_sample_back_as_raw_numbers_uncompressed() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use helio_component::cooked_texture::TextureUse;
    check_data_numbers(&gpu, CookFormat::Rgba8, TextureUse::Data, DATA_TEXEL, 0.003);
    check_data_numbers(&gpu, CookFormat::Rgba8, TextureUse::Normal, NORMAL_TEXEL, 0.003);
}

#[test]
fn data_and_normal_textures_sample_back_as_raw_numbers_as_bc7() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    if !gpu.has_bc {
        eprintln!("adapter has no TEXTURE_COMPRESSION_BC; skipping");
        return;
    }
    use helio_component::cooked_texture::TextureUse;
    check_data_numbers(&gpu, CookFormat::Bc7, TextureUse::Data, DATA_TEXEL, 0.02);
    check_data_numbers(&gpu, CookFormat::Bc7, TextureUse::Normal, NORMAL_TEXEL, 0.02);
}
