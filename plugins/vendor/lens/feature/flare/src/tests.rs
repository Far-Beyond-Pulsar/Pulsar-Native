//! Execute the production lens WGSL through the real pass recording path.
//! GPU tests require an adapter and fail rather than silently skipping.

use super::*;
use helio_pass_postprocess::{GpuPostProcessUniforms, LensFlareSettings, PostProcessSettings};
use wgpu::util::DeviceExt;

const SIZE: u32 = 128;
const OUT: u32 = SIZE / 4;

/// Tests create their own devices; creating several concurrently hangs some
/// drivers (observed on NVIDIA Vulkan), so GPU tests run one at a time.
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> (
    wgpu::Device,
    wgpu::Queue,
    std::sync::MutexGuard<'static, ()>,
) {
    let serial = GPU_SERIAL
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (device, queue) = pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = instance
            .request_adapter(&Default::default())
            .await
            .expect("GPU adapter required for lens regressions");
        adapter.request_device(&Default::default()).await.unwrap()
    });
    (device, queue, serial)
}

fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if value == 0.0 || exponent <= 0 {
        return sign;
    }
    if exponent >= 31 {
        return sign | 0x7bff;
    }
    sign | ((exponent as u16) << 10) | (mantissa >> 13) as u16
}

fn f16_to_f32(half: u16) -> f32 {
    let sign = if half & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((half >> 10) & 0x1f) as i32;
    let mantissa = (half & 0x3ff) as f32;
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// Black HDR image with the given scene-linear point sources.
fn image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sources: &[(u32, u32, f32)],
) -> wgpu::TextureView {
    let mut texels = vec![0u16; (SIZE * SIZE * 4) as usize];
    for &(x, y, radiance) in sources {
        let i = ((y * SIZE + x) * 4) as usize;
        texels[i..i + 3].fill(f32_to_f16(radiance));
    }
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("lens test input"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            Default::default(),
            bytemuck::cast_slice(&texels),
        )
        .create_view(&Default::default())
}

fn uniforms(device: &wgpu::Device, lens: LensFlareSettings) -> wgpu::Buffer {
    let settings = PostProcessSettings {
        lens_flare: lens,
        ..Default::default()
    };
    let gpu: GpuPostProcessUniforms = settings.to_gpu();
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("lens test settings"),
        contents: bytemuck::bytes_of(&gpu),
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

/// Only the named component contributes; linear extraction.
fn isolated(component: &str) -> LensFlareSettings {
    let mut lens = LensFlareSettings {
        enabled: true,
        intensity: 1.0,
        threshold: 0.0,
        soft_knee: 0.0,
        ghost_intensity: 0.0,
        halo_intensity: 0.0,
        glare_intensity: 0.0,
        streak_intensity: 0.0,
        dispersion: 0.0,
        starburst_intensity: 0.0,
        coating_strength: 0.0,
        ghost_rim: 0.0,
        light_sources: false,
        response_time: 0.0,
        ..Default::default()
    };
    match component {
        "ghost" => lens.ghost_intensity = 1.0,
        "halo" => lens.halo_intensity = 1.0,
        "glare" => lens.glare_intensity = 1.0,
        "streak" => lens.streak_intensity = 1.0,
        "starburst" => lens.starburst_intensity = 1.0,
        "lights" => {
            lens.ghost_intensity = 1.0;
            lens.light_sources = true;
        }
        _ => unreachable!(),
    }
    lens
}

fn run(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pass: &mut LensFlarePass,
    input: Option<&wgpu::TextureView>,
    pp: Option<&wgpu::Buffer>,
) -> Vec<[f32; 3]> {
    run_with(device, queue, pass, input, pp, OpticsInputs::default())
}

/// Record the pass once and read back its reduced-resolution RGB response.
fn run_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pass: &mut LensFlarePass,
    input: Option<&wgpu::TextureView>,
    pp: Option<&wgpu::Buffer>,
    optics: OpticsInputs<'_>,
) -> Vec<[f32; 3]> {
    let row_bytes = OUT * 8;
    assert_eq!(row_bytes % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lens test readback"),
        size: u64::from(row_bytes * OUT),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    pass.record(
        device,
        &mut CommandRecorder::from_encoder(&mut encoder),
        input,
        pp,
        optics,
    );
    encoder.copy_texture_to_buffer(
        pass.output.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(OUT),
            },
        },
        wgpu::Extent3d {
            width: OUT,
            height: OUT,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let halves: Vec<u16> =
        bytemuck::cast_slice(&readback.slice(..).get_mapped_range().unwrap()).to_vec();
    halves
        .chunks_exact(4)
        .map(|t| [f16_to_f32(t[0]), f16_to_f32(t[1]), f16_to_f32(t[2])])
        .collect()
}

/// The camera's optics, as the lens pass resolves them.
fn optics(device: &wgpu::Device, lens: ResolvedLens) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("lens test optics"),
        contents: bytemuck::bytes_of(&lens),
        usage: wgpu::BufferUsages::STORAGE,
    })
}

fn luminance(c: &[f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn total(response: &[[f32; 3]]) -> f32 {
    response.iter().map(luminance).sum()
}

/// Luminance-weighted centroid and variance along x and y, in output texels.
fn moments(response: &[[f32; 3]]) -> ([f32; 2], [f32; 2]) {
    let mut sum = 0.0;
    let mut mean = [0.0; 2];
    for (i, c) in response.iter().enumerate() {
        let w = luminance(c);
        sum += w;
        mean[0] += w * (i as u32 % OUT) as f32;
        mean[1] += w * (i as u32 / OUT) as f32;
    }
    mean = mean.map(|m| m / sum);
    let mut variance = [0.0; 2];
    for (i, c) in response.iter().enumerate() {
        let w = luminance(c);
        variance[0] += w * ((i as u32 % OUT) as f32 - mean[0]).powi(2);
        variance[1] += w * ((i as u32 / OUT) as f32 - mean[1]).powi(2);
    }
    (mean, variance.map(|v| v / sum))
}

#[test]
fn gpu_lens_block_matches_the_resolved_postprocess_abi() {
    assert_eq!(
        LENS_BLOCK_OFFSET as usize,
        std::mem::offset_of!(GpuPostProcessUniforms, lens_enabled)
    );
    assert_eq!(
        POSTPROCESS_BINDING_SIZE as usize,
        std::mem::size_of::<GpuPostProcessUniforms>()
    );
    let lens = LensFlareSettings {
        ghost_count: 6,
        dispersion: 0.02,
        ..isolated("glare")
    };
    let gpu = PostProcessSettings {
        lens_flare: lens,
        ..Default::default()
    }
    .to_gpu();
    let bytes = bytemuck::bytes_of(&gpu);
    let block: GpuLensResponse = bytemuck::pod_read_unaligned(
        &bytes[LENS_BLOCK_OFFSET as usize..(LENS_BLOCK_OFFSET + LENS_BLOCK_SIZE) as usize],
    );
    assert_eq!((block.enabled, block.ghost_count), (1, 6));
    assert_eq!(block.dispersion, 0.02);
    assert_eq!(block.reserved_vignette, 0.0);
}

#[test]
fn disabled_or_unbound_lens_clears_stale_response() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    let input = image(&device, &queue, &[(64, 64, 500.0)]);
    let on = uniforms(&device, isolated("glare"));
    assert!(total(&run(&device, &queue, &mut pass, Some(&input), Some(&on))) > 0.0);

    let off = uniforms(
        &device,
        LensFlareSettings {
            enabled: false,
            ..isolated("glare")
        },
    );
    let response = run(&device, &queue, &mut pass, Some(&input), Some(&off));
    assert!(
        response.iter().all(|c| *c == [0.0; 3]),
        "disabled lens left residue"
    );

    // A missing producer fails closed without an invalid binding.
    let response = run(&device, &queue, &mut pass, Some(&input), None);
    assert!(response.iter().all(|c| *c == [0.0; 3]));
}

#[test]
fn light_below_threshold_produces_no_optical_response() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    // A 4x4 block at 0.9 averages to 0.9 in the extracted image.
    let dim: Vec<_> = (60..64)
        .flat_map(|y| (60..64).map(move |x| (x, y, 0.9)))
        .collect();
    let input = image(&device, &queue, &dim);
    let lens = LensFlareSettings {
        threshold: 1.0,
        soft_knee: 0.0,
        ..isolated("glare")
    };
    let response = run(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&uniforms(&device, lens)),
    );
    assert_eq!(total(&response), 0.0);
}

#[test]
fn glare_is_energy_bounded_centered_and_aperture_normalized() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    let radiance = 400.0;
    let input = image(&device, &queue, &[(64, 64, radiance)]);
    let response = run(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&uniforms(&device, isolated("glare"))),
    );
    // Each output texel covers 16 input pixels. The glare kernel scatters a
    // calibrated 6% of the extracted energy; bilinear taps may redistribute
    // but must not create energy.
    let scattered = total(&response) * 16.0;
    assert!(scattered > 0.0);
    assert!(
        scattered <= 0.06 * radiance * 1.05,
        "glare created energy: {scattered}"
    );
    let (mean, _) = moments(&response);
    assert!(
        (mean[0] - 16.0).abs() < 1.0 && (mean[1] - 16.0).abs() < 1.0,
        "off-centre glare {mean:?}"
    );

    // Aperture changes the kernel's shape, not the scattered energy. The
    // f-number comes from the camera's lens, not the post-process block.
    let stopped_down = optics(
        &device,
        ResolvedLens {
            f_number: 16.0,
            ..Default::default()
        },
    );
    let narrow = run_with(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&uniforms(&device, isolated("glare"))),
        OpticsInputs {
            lens: Some(&stopped_down),
            ..Default::default()
        },
    );
    let ratio = total(&narrow) / total(&response);
    assert!(
        (ratio - 1.0).abs() < 0.1,
        "aperture changed exposure by {ratio}"
    );
}

#[test]
fn ghosts_reflect_across_the_optical_axis() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    // Source right of centre; the first reference ghost has magnification
    // -0.42, so its image lies left of centre on the same axis.
    let input = image(&device, &queue, &[(102, 64, 2000.0)]);
    let lens = LensFlareSettings {
        ghost_count: 1,
        ..isolated("ghost")
    };
    let response = run(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&uniforms(&device, lens)),
    );
    let (left, right): (f32, f32) =
        response
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(l, r), (i, c)| {
                if (i as u32 % OUT) < OUT / 2 {
                    (l + luminance(c), r)
                } else {
                    (l, r + luminance(c))
                }
            });
    assert!(
        left > 0.0 && left > 10.0 * right,
        "ghost not mirrored: left {left}, right {right}"
    );
    let (mean, _) = moments(&response);
    let expected = (0.5 - 0.42 * (102.5 / SIZE as f32 - 0.5)) * OUT as f32;
    assert!(
        (mean[0] - expected).abs() < 1.5,
        "ghost at {mean:?}, expected x {expected}"
    );
}

#[test]
fn anamorphic_streak_is_horizontal_and_wider_than_spherical() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    let input = image(&device, &queue, &[(64, 64, 2000.0)]);
    let spherical = run(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&uniforms(&device, isolated("streak"))),
    );
    let anamorphic_lens = LensFlareSettings {
        profile: 1,
        ..isolated("streak")
    };
    let anamorphic = run(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&uniforms(&device, anamorphic_lens)),
    );
    let (_, s) = moments(&spherical);
    let (_, a) = moments(&anamorphic);
    assert!(s[0] > 4.0 * s[1], "spherical streak not horizontal: {s:?}");
    assert!(
        a[0] > s[0],
        "anamorphic streak {a:?} not wider than spherical {s:?}"
    );
}

#[test]
fn starburst_spikes_are_perpendicular_to_the_iris_blades() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    let input = image(&device, &queue, &[(64, 64, 4000.0)]);
    // Six blades at zero rotation: spikes at 30, 90 and 150 degrees, so
    // vertical carries a spike and horizontal lies between two.
    let hexagon = optics(
        &device,
        ResolvedLens {
            aperture_blades: 6,
            aperture_rotation: 0.0,
            ..Default::default()
        },
    );
    let response = run_with(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&uniforms(&device, isolated("starburst"))),
        OpticsInputs {
            lens: Some(&hexagon),
            ..Default::default()
        },
    );
    let at = |x: u32, y: u32| luminance(&response[(y * OUT + x) as usize]);
    let vertical = at(16, 16 - 6) + at(16, 16 + 6);
    let horizontal = at(16 - 6, 16) + at(16 + 6, 16);
    assert!(
        vertical > 0.0 && vertical > 4.0 * horizontal,
        "vertical {vertical}, horizontal {horizontal}"
    );
}

/// Camera at the origin looking down -Z, square aspect, 45 degree FOV.
fn camera_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    let projection = helio_core::depth::perspective(std::f32::consts::FRAC_PI_4, 1.0, 0.1);
    let camera = helio_core::GpuCameraUniforms::new(
        glam::Mat4::IDENTITY,
        projection,
        glam::Vec3::ZERO,
        0.1,
        100.0,
        0,
        [0.0; 2],
        projection,
    );
    let bytes = [bytemuck::bytes_of(&camera), bytemuck::bytes_of(&camera)].concat();
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("lens test camera"),
        contents: &bytes,
        usage: wgpu::BufferUsages::STORAGE,
    })
}

/// One unshadowed point light (128-byte GpuLight row).
fn light_buffer(device: &wgpu::Device, position: [f32; 3], candela: f32) -> wgpu::Buffer {
    let mut row = [0u32; 32];
    let floats = [
        position[0],
        position[1],
        position[2],
        100.0,
        0.0,
        -1.0,
        0.0,
        0.0,
        1.0,
        1.0,
        1.0,
        candela,
    ];
    for (i, v) in floats.iter().enumerate() {
        row[i] = v.to_bits();
    }
    row[12] = u32::MAX; // no shadow map
    row[13] = 1; // point
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("lens test light"),
        contents: bytemuck::cast_slice(&row),
        usage: wgpu::BufferUsages::STORAGE,
    })
}

#[test]
fn lights_just_outside_the_frame_still_flare_and_hand_over_smoothly() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    // Black image: nothing on screen to scatter.
    let input = image(&device, &queue, &[]);
    let camera = camera_buffer(&device);
    let half_width = (std::f32::consts::FRAC_PI_8).tan() * 5.0;
    let total_at = |pass: &mut LensFlarePass, ndc_x: f32, lens: LensFlareSettings| {
        let lights = light_buffer(&device, [ndc_x * half_width, 0.0, -5.0], 2000.0);
        let optics = OpticsInputs {
            camera: Some(&camera),
            lights: Some(&lights),
            ..Default::default()
        };
        run_with(
            &device,
            &queue,
            pass,
            Some(&input),
            Some(&uniforms(&device, lens)),
            optics,
        )
    };

    // 15% of a half-frame past the right edge: out of frame, inside the
    // lens field. Ghosts mirror it onto the left of the image.
    let off_frame = total_at(&mut pass, 1.15, isolated("lights"));
    let (mean, _) = moments(&off_frame);
    assert!(
        total(&off_frame) > 0.0,
        "off-frame light produced no ghosts"
    );
    assert!(
        mean[0] < OUT as f32 / 2.0,
        "ghosts not mirrored across the axis: {mean:?}"
    );

    // Well inside the frame the image path owns the light (here: black).
    assert_eq!(total(&total_at(&mut pass, 0.5, isolated("lights"))), 0.0);
    // Beyond the field margin the barrel blocks it.
    assert_eq!(total(&total_at(&mut pass, 1.6, isolated("lights"))), 0.0);
    // Disabled light sources contribute nothing.
    let off = LensFlareSettings {
        light_sources: false,
        ..isolated("lights")
    };
    assert_eq!(total(&total_at(&mut pass, 1.15, off)), 0.0);

    // With a black image only the analytic share shows: it rises
    // monotonically across the handover band at the frame edge (where, in a
    // real frame, the emitter leaving the image path takes over), then fades
    // smoothly to zero as the barrel vignettes the light.
    let samples: Vec<f32> = (0..=40)
        .map(|i| {
            total(&total_at(
                &mut pass,
                0.9 + 0.01 * i as f32,
                isolated("lights"),
            ))
        })
        .collect();
    let peak_at = samples
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    let peak = samples[peak_at];
    assert!(peak > 0.0);
    for pair in samples[..=peak_at].windows(2) {
        assert!(
            pair[1] >= pair[0] * 0.999,
            "handover not monotonic: {samples:?}"
        );
    }
    for pair in samples[peak_at..].windows(2) {
        assert!(
            pair[1] <= pair[0] * 1.001 && pair[0] - pair[1] <= 0.12 * peak,
            "fade not smooth: {samples:?}"
        );
    }
}

#[test]
fn response_fades_in_and_out_instead_of_popping() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    // What prepare() writes each frame: 60 Hz, valid (black) history.
    queue.write_buffer(
        &pass.temporal_params,
        0,
        bytemuck::cast_slice(&[1.0f32 / 60.0, 1.0, 0.0, 0.0]),
    );
    let lit = image(&device, &queue, &[(64, 64, 2000.0)]);
    let dark = image(&device, &queue, &[]);
    let lens = uniforms(
        &device,
        LensFlareSettings {
            response_time: 0.06,
            ..isolated("glare")
        },
    );
    // A static camera: reprojection maps every texel onto itself.
    let camera = camera_buffer(&device);
    let frames: Vec<f32> = (0..40)
        .map(|frame| {
            let input = if frame < 20 { &lit } else { &dark };
            let optics = OpticsInputs {
                camera: Some(&camera),
                ..Default::default()
            };
            total(&run_with(
                &device,
                &queue,
                &mut pass,
                Some(input),
                Some(&lens),
                optics,
            ))
        })
        .collect();
    // Fade in: the first frame is a fraction of the settled response, and the
    // approach is monotonic, following 1 - exp(-t / tau).
    let settled = frames[19];
    let expected_first = 1.0 - (-(1.0f32 / 60.0) / 0.06).exp();
    assert!(
        (frames[0] / settled - expected_first).abs() < 0.05,
        "{frames:?}"
    );
    assert!(
        frames[..20].windows(2).all(|w| w[1] >= w[0]),
        "fade-in not monotonic: {frames:?}"
    );
    // Fade out after the source disappears (e.g. occluded), not a pop.
    assert!(
        frames[20] > 0.5 * settled && frames[20] < settled,
        "{frames:?}"
    );
    assert!(
        frames[20..].windows(2).all(|w| w[1] <= w[0]) && frames[39] < 0.01 * settled,
        "{frames:?}"
    );
}

#[test]
fn source_history_follows_the_camera_instead_of_trailing() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    queue.write_buffer(
        &pass.temporal_params,
        0,
        bytemuck::cast_slice(&[1.0f32 / 60.0, 1.0, 0.0, 0.0]),
    );
    let lens = uniforms(
        &device,
        LensFlareSettings {
            response_time: 0.06,
            ..isolated("glare")
        },
    );
    // The same source seen from a camera that panned by 16 input pixels:
    // with reprojection the settled glare follows to the new position with no
    // residue at the old one.
    let projection = helio_core::depth::perspective(std::f32::consts::FRAC_PI_4, 1.0, 0.1);
    let camera_at = |view: glam::Mat4, prev: glam::Mat4| {
        let data = helio_core::GpuCameraUniforms::new(
            view,
            projection,
            glam::Vec3::ZERO,
            0.1,
            100.0,
            0,
            [0.0; 2],
            prev,
        );
        let bytes = [bytemuck::bytes_of(&data), bytemuck::bytes_of(&data)].concat();
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &bytes,
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let still = camera_at(glam::Mat4::IDENTITY, projection);
    let before = image(&device, &queue, &[(48, 64, 2000.0)]);
    for _ in 0..30 {
        let optics = OpticsInputs {
            camera: Some(&still),
            ..Default::default()
        };
        run_with(
            &device,
            &queue,
            &mut pass,
            Some(&before),
            Some(&lens),
            optics,
        );
    }
    // The fallback depth is the depth clear value, sky, so history reprojects
    // as directions and only a rotation moves it. Solve the yaw that moves the
    // source's direction by 16 input pixels (NDC -0.25 -> -0.5 at 128 px).
    let half_width = (std::f32::consts::FRAC_PI_8).tan();
    let yaw = (-0.25 * half_width).atan() - (-0.5 * half_width).atan();
    let moved = camera_at(glam::Mat4::from_rotation_y(yaw), projection);
    // Yawing the view this way moves the scene left: 48 -> 32 input pixels.
    let after = image(&device, &queue, &[(32, 64, 2000.0)]);
    let optics = OpticsInputs {
        camera: Some(&moved),
        ..Default::default()
    };
    let response = run_with(
        &device,
        &queue,
        &mut pass,
        Some(&after),
        Some(&lens),
        optics,
    );
    let (mean, _) = moments(&response);
    // Settled history reprojected onto the new position (x=8); a trailing
    // blend would sit between 8 and 12.
    assert!(
        (mean[0] - 8.0).abs() < 0.6,
        "response trailed: centroid {mean:?}"
    );
}

/// Sky-only depth (the depth clear value) of any size.
fn far_depth(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lens test far depth"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(helio_core::depth::CLEAR),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    queue.submit([encoder.finish()]);
    view
}

#[test]
fn sky_history_reprojects_as_directions() {
    // The camera projection has no far plane: sky is depth 0, which
    // unprojects to w = 0. Sky texels must still reproject as directions
    // (not NaN, which the streaks then smear across whole rows), whatever the
    // near plane.
    let (device, queue, _serial) = gpu();
    let depth = far_depth(&device, &queue);
    let lens = uniforms(
        &device,
        LensFlareSettings {
            response_time: 0.06,
            ..isolated("glare")
        },
    );
    let input = image(&device, &queue, &[(64, 64, 2000.0)]);
    let settle = |near: f32| {
        let projection = helio_core::depth::perspective(std::f32::consts::FRAC_PI_4, 1.0, near);
        // Off the origin, so a direction built from the wrong eye would show.
        let eye = glam::Vec3::new(3.0, 4.0, 5.0);
        let view = glam::Mat4::from_translation(-eye);
        let data = helio_core::GpuCameraUniforms::new(
            view,
            projection,
            eye,
            near,
            4.0e7,
            0,
            [0.0; 2],
            projection * view,
        );
        let bytes = [bytemuck::bytes_of(&data), bytemuck::bytes_of(&data)].concat();
        let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
        queue.write_buffer(
            &pass.temporal_params,
            0,
            bytemuck::cast_slice(&[1.0f32 / 60.0, 1.0, 0.0, 0.0]),
        );
        let mut response = Vec::new();
        for _ in 0..30 {
            let optics = OpticsInputs {
                camera: Some(&camera),
                depth: Some(&depth),
                ..Default::default()
            };
            response = run_with(
                &device,
                &queue,
                &mut pass,
                Some(&input),
                Some(&lens),
                optics,
            );
        }
        response
    };
    // A planetary camera near the ground: near 5 cm.
    let extreme = settle(0.05);
    assert!(
        extreme.iter().flatten().all(|v| v.is_finite()),
        "non-finite lens response"
    );
    let reference = settle(0.1);
    let (a, b) = (total(&extreme), total(&reference));
    assert!(
        b > 0.0 && (a / b - 1.0).abs() < 0.01,
        "settled sky response {a} vs {b}"
    );
}

#[test]
fn optics_come_from_the_resolved_lens() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    let input = image(&device, &queue, &[(64, 64, 400.0)]);
    let pp = uniforms(&device, isolated("glare"));
    // Without a lens pass the neutral 50 mm f/2.8 camera applies.
    let default = run(&device, &queue, &mut pass, Some(&input), Some(&pp));
    let neutral = optics(&device, ResolvedLens::default());
    let explicit = run_with(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&pp),
        OpticsInputs {
            lens: Some(&neutral),
            ..Default::default()
        },
    );
    assert_eq!(default, explicit);
    // Stopping down widens the diffraction part of the glare kernel.
    let stopped_down = optics(
        &device,
        ResolvedLens {
            f_number: 32.0,
            ..Default::default()
        },
    );
    let wide = run_with(
        &device,
        &queue,
        &mut pass,
        Some(&input),
        Some(&pp),
        OpticsInputs {
            lens: Some(&stopped_down),
            ..Default::default()
        },
    );
    let (_, narrow_variance) = moments(&default);
    let (_, wide_variance) = moments(&wide);
    assert!(
        wide_variance[0] > 1.5 * narrow_variance[0],
        "{wide_variance:?} vs {narrow_variance:?}"
    );
}

#[test]
fn analytic_lights_take_the_lenss_natural_falloff_and_the_image_does_not() {
    let (device, queue, _serial) = gpu();
    let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
    let camera = camera_buffer(&device);
    let half_width = (std::f32::consts::FRAC_PI_8).tan() * 5.0;
    let lights = light_buffer(&device, [1.15 * half_width, 0.0, -5.0], 2000.0);
    let black = image(&device, &queue, &[]);
    let lit = |natural: f32, input: &wgpu::TextureView, lights: Option<&wgpu::Buffer>| {
        let lens = optics(
            &device,
            ResolvedLens {
                natural_vignette: natural,
                ..Default::default()
            },
        );
        let optics = OpticsInputs {
            camera: Some(&camera),
            lights,
            lens: Some(&lens),
            ..Default::default()
        };
        let mut pass = LensFlarePass::new_hdr(&device, SIZE, SIZE);
        let pp = uniforms(&device, isolated("lights"));
        total(&run_with(
            &device,
            &queue,
            &mut pass,
            Some(input),
            Some(&pp),
            optics,
        ))
    };
    // An off-frame light is not in the image the lens vignetted, so the
    // flare applies the lens's falloff to it: a corner source dims.
    let (without, with) = (
        lit(0.0, &black, Some(&lights)),
        lit(1.0, &black, Some(&lights)),
    );
    assert!(
        without > 0.0 && with < 0.98 * without,
        "analytic source {without} -> {with}"
    );
    // The image path takes the image as it is: it already has the falloff.
    let mut glare = |natural: f32| {
        let lens = optics(
            &device,
            ResolvedLens {
                natural_vignette: natural,
                ..Default::default()
            },
        );
        let input = image(&device, &queue, &[(8, 8, 400.0)]);
        let pp = uniforms(&device, isolated("glare"));
        let optics = OpticsInputs {
            lens: Some(&lens),
            ..Default::default()
        };
        total(&run_with(
            &device,
            &queue,
            &mut pass,
            Some(&input),
            Some(&pp),
            optics,
        ))
    };
    let (a, b) = (glare(0.0), glare(1.0));
    assert!(
        a > 0.0 && (a / b - 1.0).abs() < 1.0e-4,
        "image path changed with the lens's falloff: {a} vs {b}"
    );
}
