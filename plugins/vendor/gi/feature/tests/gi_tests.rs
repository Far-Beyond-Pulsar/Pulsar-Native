//! The probes' light against what it must be: a uniform sky (a white
//! furnace), the sun on flat ground, and the resolve at pixels.

mod common;

use common::{half2, pack, probe};
use glam::{DVec3, Mat4, Vec3};
use helio_pass_gi::{
    Clipmap, GiConfig, GiPass, GiResolvePass, DEFAULT_FINEST_SPACING, LEVEL_PROBES, PROBES, RAYS,
};

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn read(gpu: &Gpu, buffer: &wgpu::Buffer) -> Vec<u32> {
    common::read(&gpu.device, &gpu.queue, buffer)
}

/// A directional light straight down, illuminance `e`.
fn sun_down(gpu: &Gpu, e: f32) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    let light = common::sun_down(e);
    gpu.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sun"),
            contents: bytemuck::cast_slice(&light),
            usage: wgpu::BufferUsages::STORAGE,
        })
}

fn gpu() -> Option<&'static Gpu> {
    static GPU: std::sync::OnceLock<Option<Gpu>> = std::sync::OnceLock::new();
    GPU.get_or_init(|| {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits {
                max_buffer_size: limits.max_buffer_size.min(u32::MAX as u64),
                ..limits
            },
            ..Default::default()
        }))
        .ok()?;
        Some(Gpu { device, queue })
    })
    .as_ref()
}

/// One frame of the pass; `tracer` writes the hits of its rays (`gi_rays`
/// words, a ray each: origin, most distance, direction, probe) into words of
/// `gi_hits`.
fn frame(
    gpu: &Gpu,
    pass: &mut GiPass,
    eye: DVec3,
    n: u64,
    sky: f32,
    lights: Option<&wgpu::Buffer>,
    tracer: Option<&dyn Fn(&[u32]) -> Vec<u32>>,
) {
    pass.begin(&gpu.queue, eye, n, Vec3::splat(sky), Vec3::splat(sky));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut cmds = helio_core::CommandRecorder::from_encoder(&mut encoder);
    pass.record(&gpu.device, &mut cmds, None, None, lights, None);
    drop(cmds);
    gpu.queue.submit([encoder.finish()]);
    if let Some(trace) = tracer {
        let [_, _, rays, hits, ..] = pass.buffers();
        let rays = read(gpu, rays);
        let count = pass.updates().len() * RAYS as usize;
        let words = trace(&rays[..count * 8]);
        gpu.queue
            .write_buffer(pass.buffers()[3], 0, bytemuck::cast_slice(&words));
        let _ = hits;
    }
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
}

/// A uniform sky of radiance 1 and nothing to hit: every updated probe's
/// irradiance / pi is 1 in every direction, its distances the most.
#[test]
fn a_uniform_sky_lights_probes_evenly() {
    let Some(gpu) = gpu() else { return };
    let mut pass = GiPass::new(&gpu.device, GiConfig::default());
    let eye = DVec3::new(0.3, 1.7, -0.2);
    for n in 0..4 {
        frame(gpu, &mut pass, eye, n, 1.0, None, None);
    }
    let words = read(gpu, pass.buffers()[1]);
    let mut updated = 0;
    for index in 0..PROBES {
        let (e, state) = probe(&words, index, Vec3::Y);
        if state >> 8 == 0 {
            continue;
        }
        updated += 1;
        assert_eq!(state & 1, 1, "probe {index} inactive");
        for d in [Vec3::X, Vec3::NEG_Y, Vec3::Z, Vec3::new(0.6, -0.48, 0.64)] {
            let e = probe(&words, index, d).0;
            assert!(
                (e - Vec3::ONE).abs().max_element() < 0.02,
                "probe {index} toward {d}: {e}"
            );
        }
        assert!((e - Vec3::ONE).abs().max_element() < 0.02);
    }
    assert!(
        updated >= 3 * helio_pass_gi::MAX_UPDATES,
        "{updated} probes updated"
    );
}

/// No sky; the sun (illuminance 2) straight down on flat ground of albedo
/// about 0.5 at height 0. A probe over it sees the ground's radiance a 2 / pi
/// below and nothing above: irradiance / pi that facing down, half of it
/// sideways, none facing up (L1 harmonics give a hemisphere's step exactly
/// facing along it). Probes under the ground are inside it: inactive.
#[test]
fn the_sun_on_the_ground_lights_probes_from_below() {
    let Some(gpu) = gpu() else { return };
    let mut pass = GiPass::new(&gpu.device, GiConfig::default());
    let sun = sun_down(gpu, 2.0);
    let eye = DVec3::new(0.3, 1.7, -0.2);
    let ground = |rays: &[u32]| -> Vec<u32> {
        let mut hits = vec![0u32; rays.len() / 2];
        for (k, ray) in rays.chunks(8).enumerate() {
            let f = |w: u32| f32::from_bits(w);
            let origin = Vec3::new(f(ray[0]), f(ray[1]), f(ray[2])).as_dvec3() + eye;
            let dir = Vec3::new(f(ray[4]), f(ray[5]), f(ray[6]));
            let hit = if origin.y < 0.0 {
                // Inside the ground: a back face at once.
                Some((1e-3f32, pack(dir), 0u32))
            } else if dir.y < 0.0 {
                // Albedo 0.5, the sun seen.
                Some((
                    (-origin.y / f64::from(dir.y)) as f32,
                    pack(Vec3::Y),
                    128 | (128 << 8) | (128 << 16) | (255 << 24),
                ))
            } else {
                None
            };
            if let Some((t, n, albedo)) = hit {
                hits[k * 4..k * 4 + 4].copy_from_slice(&[t.to_bits(), n, albedo, 1]);
            }
        }
        hits
    };
    for n in 0..4 {
        frame(gpu, &mut pass, eye, n, 0.0, Some(&sun), Some(&ground));
    }
    let clipmap = Clipmap::around(eye, DEFAULT_FINEST_SPACING);
    let words = read(gpu, pass.buffers()[1]);
    // Albedo 128/255 (RGB8), illuminance 2.
    let below = 128.0 / 255.0 * 2.0 / std::f32::consts::PI;
    let (mut above, mut under) = (0, 0);
    for index in 0..PROBES {
        let (_, state) = probe(&words, index, Vec3::Y);
        if state >> 8 == 0 {
            continue;
        }
        let y = clipmap.position(index).y;
        if y < 0.0 {
            under += 1;
            assert_eq!(state & 1, 0, "probe {index} at {y} active");
            continue;
        }
        if y < 0.5 {
            continue;
        }
        above += 1;
        let down = probe(&words, index, Vec3::NEG_Y).0;
        let side = probe(&words, index, Vec3::X).0;
        let up = probe(&words, index, Vec3::Y).0;
        assert!(
            (down - Vec3::splat(below)).abs().max_element() < 0.05 * below,
            "probe {index} at {y}: down {down}, want {below}"
        );
        assert!(
            (side - Vec3::splat(below * 0.5)).abs().max_element() < 0.08 * below,
            "probe {index} at {y}: side {side}"
        );
        assert!(
            up.max_element() < 0.05 * below,
            "probe {index} at {y}: up {up}"
        );
    }
    assert!(above > 1000 && under > 1000, "{above} above, {under} under");
    let _ = LEVEL_PROBES;
}

/// The resolve at pixels of a uniform sky's probes: every point the
/// clipmap holds gets irradiance / pi 1, held fully.
#[test]
fn the_resolve_reads_the_probes_at_pixels() {
    let Some(gpu) = gpu() else { return };
    let mut pass = GiPass::new(&gpu.device, GiConfig::default());
    let eye = DVec3::new(0.3, 1.7, -0.2);
    // Every probe updated: 13824 a level at 680 a frame.
    for n in 0..22 {
        frame(gpu, &mut pass, eye, n, 1.0, None, None);
    }
    let size = [16u32, 16];
    let texture = |format, label| {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    };
    let (normal, depth) = (
        texture(wgpu::TextureFormat::Rgba16Float, "normal"),
        texture(wgpu::TextureFormat::Depth32Float, "depth"),
    );
    let (normal_view, depth_view) = (
        normal.create_view(&Default::default()),
        depth.create_view(&Default::default()),
    );
    let view = Mat4::look_to_rh(Vec3::ZERO, Vec3::new(0.2, -0.3, -1.0).normalize(), Vec3::Y);
    let proj = helio_core::depth::perspective(1.0, 1.0, 0.1);
    let camera = helio_core::GpuCameraUniforms::new(
        view,
        proj,
        Vec3::ZERO,
        0.1,
        1000.0,
        0,
        [0.0; 2],
        proj * view,
    );
    use wgpu::util::DeviceExt;
    let camera = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&camera),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let mut resolve = GiResolvePass::new(&gpu.device);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    // Points about 4 m away (reversed depth near / distance), facing up.
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &normal_view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.0,
                    g: 1.0,
                    b: 0.0,
                    a: 0.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &depth_view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(0.1 / 4.0),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    let mut cmds = helio_core::CommandRecorder::from_encoder(&mut encoder);
    let [frame_buffer, probes, ..] = pass.buffers();
    resolve.record(
        &gpu.device,
        &gpu.queue,
        &mut cmds,
        [frame_buffer, probes, &camera],
        &normal_view,
        &depth_view,
        size,
    );
    drop(cmds);
    gpu.queue.submit([encoder.finish()]);
    let words = read(gpu, resolve.irradiance().unwrap());
    for (p, w) in words.chunks(2).enumerate() {
        let (rg, ba) = (half2(w[0]), half2(w[1]));
        let e = Vec3::new(rg[0], rg[1], ba[0]);
        assert!(
            (e - Vec3::ONE).abs().max_element() < 0.03 && (ba[1] - 1.0).abs() < 1e-3,
            "pixel {p}: {e} held {}",
            ba[1]
        );
    }
}

/// No sky; the sun (illuminance 2) straight down on a canopy at height 4.5
/// that lets a leaf's share of light through (0.3; albedo about 0.5, its
/// far side seen by the sun). A probe under it sees the canopy's underside
/// give that share of the light on its top: radiance 0.3 a 2 / pi above
/// (within a tenth), nothing below.
#[test]
fn light_through_a_canopy_lights_probes_under_it() {
    let Some(gpu) = gpu() else { return };
    let mut pass = GiPass::new(&gpu.device, GiConfig::default());
    let sun = sun_down(gpu, 2.0);
    let eye = DVec3::new(0.3, 1.7, -0.2);
    let canopy = |rays: &[u32]| -> Vec<u32> {
        let mut hits = vec![0u32; rays.len() / 2];
        for (k, ray) in rays.chunks(8).enumerate() {
            let f = |w: u32| f32::from_bits(w);
            let origin = Vec3::new(f(ray[0]), f(ray[1]), f(ray[2])).as_dvec3() + eye;
            let dir = Vec3::new(f(ray[4]), f(ray[5]), f(ray[6]));
            let albedo = 128 | (128 << 8) | (128 << 16);
            let t = ((4.5 - origin.y) / f64::from(dir.y)) as f32;
            let hit = if origin.y < 4.5 && dir.y > 0.0 {
                // The underside: facing away from the sun, a leaf's share
                // (77/255) let through, the sun on its far side.
                Some((t, pack(Vec3::NEG_Y), albedo, 1 | (77 << 8) | (255 << 16)))
            } else if origin.y > 4.5 && dir.y < 0.0 {
                Some((t, pack(Vec3::Y), albedo | (255 << 24), 1 | (77 << 8)))
            } else {
                None
            };
            if let Some((t, n, albedo, light)) = hit {
                hits[k * 4..k * 4 + 4].copy_from_slice(&[t.to_bits(), n, albedo, light]);
            }
        }
        hits
    };
    for n in 0..4 {
        frame(gpu, &mut pass, eye, n, 0.0, Some(&sun), Some(&canopy));
    }
    let clipmap = Clipmap::around(eye, DEFAULT_FINEST_SPACING);
    let words = read(gpu, pass.buffers()[1]);
    let above = 77.0 / 255.0 * 128.0 / 255.0 * 2.0 / std::f32::consts::PI;
    // The underside also passes on the light under it (the probes' own,
    // their multi-bounce term), a gain of its transmission times albedo:
    // up to the series' sum, as far as the probes have taken it in.
    let gain = 77.0 / 255.0 * 128.0 / 255.0;
    let most = above / (1.0 - gain);
    let mut under = 0;
    for index in 0..PROBES {
        let (_, state) = probe(&words, index, Vec3::Y);
        let y = clipmap.position(index).y;
        if state >> 8 == 0 || !(0.0..4.0).contains(&y) {
            continue;
        }
        under += 1;
        let up = probe(&words, index, Vec3::Y).0;
        let down = probe(&words, index, Vec3::NEG_Y).0;
        // The light through the canopy, and at most all of its bounce back
        // (how much of it the probes have taken in after four frames depends
        // on the order they updated in); a few percent either way for those
        // just under it (a spacing away), which leak a little of their own
        // through the visibility test (DDGI's thin occluders).
        assert!(
            up.min_element() > 0.95 * above && up.max_element() < 1.05 * most,
            "probe {index} at {y}: up {up}, want {above} to {most}"
        );
        assert!(
            down.max_element() < 0.06 * above,
            "probe {index} at {y}: down {down}"
        );
    }
    assert!(under > 500, "{under} under the canopy");
}

/// No sky, no sun; a glowing floor at height 0 (albedo about 0.5, giving
/// off its albedo times 4: radiance 2): a probe over it sees that radiance
/// below and nothing above, irradiance / pi 2 facing down.
#[test]
fn glowing_ground_lights_probes() {
    let Some(gpu) = gpu() else { return };
    let mut pass = GiPass::new(&gpu.device, GiConfig::default());
    let eye = DVec3::new(0.3, 1.7, -0.2);
    let floor = |rays: &[u32]| -> Vec<u32> {
        let mut hits = vec![0u32; rays.len() / 2];
        for (k, ray) in rays.chunks(8).enumerate() {
            let f = |w: u32| f32::from_bits(w);
            let origin = Vec3::new(f(ray[0]), f(ray[1]), f(ray[2])).as_dvec3() + eye;
            let dir = Vec3::new(f(ray[4]), f(ray[5]), f(ray[6]));
            if origin.y > 0.0 && dir.y < 0.0 {
                let t = (-origin.y / f64::from(dir.y)) as f32;
                // Albedo 128/255, the sun unseen, glow 4 (64 sixteenths).
                hits[k * 4..k * 4 + 4].copy_from_slice(&[
                    t.to_bits(),
                    pack(Vec3::Y),
                    128 | (128 << 8) | (128 << 16),
                    1 | (64 << 24),
                ]);
            }
        }
        hits
    };
    for n in 0..4 {
        frame(gpu, &mut pass, eye, n, 0.0, None, Some(&floor));
    }
    let clipmap = Clipmap::around(eye, DEFAULT_FINEST_SPACING);
    let words = read(gpu, pass.buffers()[1]);
    let below = 128.0 / 255.0 * 4.0;
    let mut over = 0;
    for index in 0..PROBES {
        let (_, state) = probe(&words, index, Vec3::Y);
        let y = clipmap.position(index).y;
        if state >> 8 == 0 || y < 0.5 {
            continue;
        }
        over += 1;
        let down = probe(&words, index, Vec3::NEG_Y).0;
        let up = probe(&words, index, Vec3::Y).0;
        assert!(
            (down - Vec3::splat(below)).abs().max_element() < 0.05 * below,
            "probe {index} at {y}: down {down}, want {below}"
        );
        assert!(
            up.max_element() < 0.05 * below,
            "probe {index} at {y}: up {up}"
        );
    }
    assert!(over > 1000, "{over} probes over the floor");
}
