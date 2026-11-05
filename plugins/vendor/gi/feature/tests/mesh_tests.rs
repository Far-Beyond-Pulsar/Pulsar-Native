//! The meshes' tracer (`GiMeshTracePass`) against what the probes must hold:
//! a closed emissive box (a white furnace, single and multiple bounces), a
//! Cornell box lit by a point light (colour bleeding off its red and green
//! walls), the nearer of its hit and another tracer's, and a cut-out canopy
//! under the sky (shade where its alpha says leaves, sky through its
//! holes). Needs hardware ray queries: without them each test returns at
//! once.

mod common;

use common::{pack, probe};
use glam::{DVec3, Vec3};
use helio_core::{BlasGeometry, BlasManager, TlasInstanceInput, TlasManager};
use helio_mesh_api::{RayInstance, RAY_INSTANCE_MASKED, RAY_INSTANCE_TWO_SIDED};
use helio_pass_gi::{Clipmap, GiConfig, GiMeshTracePass, GiPass, PROBES, RAYS};
use std::sync::Arc;
use wgpu::util::DeviceExt;

struct Gpu {
    device: Arc<wgpu::Device>,
    queue: wgpu::Queue,
}

/// A device with ray queries (none without them).
fn gpu() -> Option<&'static Gpu> {
    static GPU: std::sync::OnceLock<Option<Gpu>> = std::sync::OnceLock::new();
    GPU.get_or_init(|| {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        if !adapter
            .features()
            .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
        {
            eprintln!("no ray queries on {:?}: skipped", adapter.get_info().name);
            return None;
        }
        let limits = adapter.limits();
        // The material table as the renderer binds it (bindless when it can).
        let bindless = adapter.features() & helio_mats::BINDLESS_MATERIAL_FEATURES;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY | bindless,
            required_limits: wgpu::Limits {
                max_buffer_size: limits.max_buffer_size.min(u32::MAX as u64),
                ..limits
            },
            // These tests exercise wgpu's experimental ray queries on purpose.
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        }))
        .ok()?;
        Some(Gpu {
            device: Arc::new(device),
            queue,
        })
    })
    .as_ref()
}

/// A quad of a scene: its corners (a parallelogram: `corner`, plus `u`, plus
/// `v`), the side it faces (its vertex normals) and its surface; UVs from 0
/// to `uv` across it, its row's `RAY_INSTANCE_*` flags and material row.
#[derive(Default)]
struct Quad {
    corner: Vec3,
    u: Vec3,
    v: Vec3,
    normal: Vec3,
    base_color: Vec3,
    emissive: Vec3,
    uv: f32,
    flags: u32,
    material: u32,
}

/// The quads as a TLAS (an instance each) with their rows, the way a
/// frontend projection publishes them; the material table cut-outs are
/// tested against, and the sky's radiance.
struct Scene {
    tlas: TlasManager,
    _blas: BlasManager,
    instances: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    materials: Option<wgpu::BindGroup>,
    sky: Vec3,
}

fn snorm8(n: Vec3) -> u32 {
    let b = |x: f32| ((x.clamp(-1.0, 1.0) * 127.0).round() as i8) as u8 as u32;
    b(n.x) | (b(n.y) << 8) | (b(n.z) << 16)
}

fn scene(gpu: &Gpu, quads: &[Quad]) -> Scene {
    // `PackedVertex` rows: position, bitangent sign, two UVs, normal, tangent.
    let mut vertices = Vec::<u32>::new();
    let mut indices = Vec::<u32>::new();
    let mut rows = Vec::new();
    for q in quads {
        let first_index = indices.len() as u32;
        let base_vertex = (vertices.len() / 10) as u32;
        for (p, [s, t]) in [
            (q.corner, [0.0, 0.0]),
            (q.corner + q.u, [q.uv, 0.0]),
            (q.corner + q.u + q.v, [q.uv, q.uv]),
            (q.corner + q.v, [0.0, q.uv]),
        ] {
            vertices.extend_from_slice(&[
                p.x.to_bits(),
                p.y.to_bits(),
                p.z.to_bits(),
                1f32.to_bits(),
                f32::to_bits(s),
                f32::to_bits(t),
                0,
                0,
                snorm8(q.normal),
                0,
            ]);
        }
        indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        rows.push(RayInstance {
            first_index,
            base_vertex,
            flags: q.flags,
            material: q.material,
            base_color: q.base_color.extend(1.0).to_array(),
            emissive: q.emissive.extend(0.0).to_array(),
            transmission: [0.0; 4],
        });
    }
    let usage =
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::COPY_DST;
    let init = |label, contents: &[u8], usage| {
        gpu.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
    };
    let vertex_buffer = init("mesh vertices", bytemuck::cast_slice(&vertices), usage);
    let index_buffer = init("mesh indices", bytemuck::cast_slice(&indices), usage);
    let instances = init(
        "ray instances",
        bytemuck::cast_slice(&rows),
        wgpu::BufferUsages::STORAGE,
    );
    let mut blas = BlasManager::new(gpu.device.clone());
    let mut tlas = TlasManager::new(gpu.device.clone(), quads.len() as u32);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut inputs = Vec::new();
    for (k, row) in rows.iter().enumerate() {
        // Cut-outs are candidates, as the projection builds them.
        let opaque = row.flags & RAY_INSTANCE_MASKED == 0;
        blas.build_from_buffers_with_opacity(
            k as u64,
            &mut encoder,
            BlasGeometry {
                revision: 0,
                vertices: &vertex_buffer,
                first_vertex: row.base_vertex,
                vertex_count: 4,
                vertex_stride: 40,
                indices: Some(&index_buffer),
                first_index: row.first_index,
                index_count: 6,
            },
            opaque,
        )
        .unwrap();
        inputs.push(TlasInstanceInput {
            mesh_id: k as u64,
            transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        });
    }
    tlas.build(&mut encoder, &inputs, &blas).unwrap();
    gpu.queue.submit([encoder.finish()]);
    Scene {
        tlas,
        _blas: blas,
        instances,
        vertices: vertex_buffer,
        indices: index_buffer,
        materials: None,
        sky: Vec3::ZERO,
    }
}

/// The six inward faces of the box centred at the origin with half extent
/// `h`: the -x wall `left`, the +x wall `right`, the others `rest`, all
/// giving off `glow`.
fn room(h: f32, left: Vec3, right: Vec3, rest: Vec3, glow: Vec3) -> Vec<Quad> {
    let quad = |corner: Vec3, u: Vec3, v: Vec3, normal: Vec3, base_color: Vec3| Quad {
        corner,
        u,
        v,
        normal,
        base_color,
        emissive: glow,
        ..Default::default()
    };
    let s = 2.0 * h;
    vec![
        quad(
            Vec3::new(-h, -h, -h),
            Vec3::Y * s,
            Vec3::Z * s,
            Vec3::X,
            left,
        ),
        quad(
            Vec3::new(h, -h, -h),
            Vec3::Z * s,
            Vec3::Y * s,
            -Vec3::X,
            right,
        ),
        quad(
            Vec3::new(-h, -h, -h),
            Vec3::Z * s,
            Vec3::X * s,
            Vec3::Y,
            rest,
        ),
        quad(
            Vec3::new(-h, h, -h),
            Vec3::X * s,
            Vec3::Z * s,
            -Vec3::Y,
            rest,
        ),
        quad(
            Vec3::new(-h, -h, -h),
            Vec3::X * s,
            Vec3::Y * s,
            Vec3::Z,
            rest,
        ),
        quad(
            Vec3::new(-h, -h, h),
            Vec3::Y * s,
            Vec3::X * s,
            -Vec3::Z,
            rest,
        ),
    ]
}

/// The probes' spacing in these rooms.
const SPACING: f64 = 0.5;

/// One frame: the probes' update, `tracer` (another tracer, on the CPU:
/// rays' words in, hits' words out), then the meshes' tracer; the scene's
/// sky above the horizon, none below.
fn frame(
    gpu: &Gpu,
    pass: &mut GiPass,
    mesh: &GiMeshTracePass,
    scene: &Scene,
    eye: DVec3,
    n: u64,
    lights: Option<&wgpu::Buffer>,
    tracer: Option<&dyn Fn(&[u32]) -> Vec<u32>>,
) {
    pass.begin(&gpu.queue, eye, n, scene.sky, Vec3::ZERO);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut cmds = helio_core::CommandRecorder::from_encoder(&mut encoder);
    pass.record(&gpu.device, &mut cmds, None, None, lights, None);
    drop(cmds);
    gpu.queue.submit([encoder.finish()]);
    let [frame_buffer, _, rays, hits, sun, light] = pass.buffers();
    if let Some(trace) = tracer {
        let words = common::read(&gpu.device, &gpu.queue, rays);
        let count = pass.updates().len() * RAYS as usize;
        gpu.queue
            .write_buffer(hits, 0, bytemuck::cast_slice(&trace(&words[..count * 8])));
    }
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut cmds = helio_core::CommandRecorder::from_encoder(&mut encoder);
    let eye = eye.as_vec3().to_array();
    mesh.record(
        &gpu.device,
        &gpu.queue,
        &mut cmds,
        [frame_buffer, rays, hits, sun, light],
        scene.tlas.tlas().unwrap(),
        eye,
        [&scene.instances, &scene.vertices, &scene.indices],
        None,
        scene.materials.as_ref(),
        lights.map(|l| (l, 1)),
        None,
        None,
        n as u32,
    );
    drop(cmds);
    gpu.queue.submit([encoder.finish()]);
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
}

/// Each updated, active probe strictly inside the box of half extent `h`
/// (a spacing in from its walls): its index and position.
fn inside(words: &[u32], clipmap: &Clipmap, h: f64) -> Vec<(u32, DVec3)> {
    (0..PROBES)
        .filter_map(|index| {
            let (_, state) = probe(words, index, Vec3::Y);
            let p = clipmap.position(index);
            let level = (index / helio_pass_gi::LEVEL_PROBES) as usize;
            (state >> 8 > 0
                && state & 1 == 1
                && p.abs().max_element() < h - clipmap.spacing(level).min(h * 0.5))
            .then_some((index, p))
        })
        .collect()
}

const DIRECTIONS: [Vec3; 6] = [
    Vec3::X,
    Vec3::NEG_X,
    Vec3::Y,
    Vec3::NEG_Y,
    Vec3::Z,
    Vec3::NEG_Z,
];

/// A closed box whose walls give off radiance 1 and reflect nothing: every
/// probe inside sees radiance 1 everywhere, irradiance / pi 1 in every
/// direction (a white furnace). Probes in its walls' backs (outside) see
/// their insides: inactive.
#[test]
fn a_glowing_box_lights_its_probes_evenly() {
    let Some(gpu) = gpu() else { return };
    let mesh = GiMeshTracePass::new(&gpu.device, &gpu.queue).unwrap();
    let h = 2.1;
    let scene = scene(gpu, &room(h, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ONE));
    let mut pass = GiPass::new(
        &gpu.device,
        GiConfig {
            finest_spacing: SPACING,
            needs_tlas: true,
        },
    );
    let eye = DVec3::new(0.1, 0.3, -0.2);
    // Every probe of the finest levels once (13824 at 680 a frame), those
    // that came in first, in order from the clipmap's far corner.
    for n in 0..24 {
        frame(gpu, &mut pass, &mesh, &scene, eye, n, None, None);
    }
    let clipmap = Clipmap::around(eye, SPACING);
    let words = common::read(&gpu.device, &gpu.queue, pass.buffers()[1]);
    let probes = inside(&words, &clipmap, f64::from(h));
    assert!(probes.len() > 200, "{} probes inside", probes.len());
    for &(index, p) in &probes {
        for d in DIRECTIONS {
            let e = probe(&words, index, d).0;
            assert!(
                (e - Vec3::ONE).abs().max_element() < 0.02,
                "probe {index} at {p} toward {d}: {e}"
            );
        }
    }
    // Just outside the middle of a wall, nearly every ray toward the box
    // meets its back: about half of them, so the probe is taken to be
    // inside a solid.
    let mut outside = 0;
    for index in 0..PROBES {
        let (_, state) = probe(&words, index, Vec3::Y);
        let p = clipmap.position(index);
        let level = (index / helio_pass_gi::LEVEL_PROBES) as usize;
        let a = p.abs();
        let past = a.max_element() - f64::from(h);
        // Over the middle of a face: its other two coordinates well inside.
        let middle = a.x + a.y + a.z - a.max_element() - a.min_element() < f64::from(h) - 1.0;
        if state >> 8 > 0 && level == 0 && past > 0.0 && past < 0.5 && middle {
            outside += 1;
            assert_eq!(
                state & 1,
                0,
                "probe {index} at {p}, just outside a wall, active"
            );
        }
    }
    assert!(outside > 10, "{outside} probes just outside");
}

/// The same box reflecting half (albedo 0.5) and giving off 0.5: radiance
/// 0.5 / (1 - 0.5) = 1 once every bounce is in, which the probes approach
/// from below as their own irradiance at the walls gives each further
/// bounce. They must never gain energy (above 1), and after ten rounds of
/// updates hold most of it.
#[test]
fn bounces_in_a_box_approach_the_furnace_without_gaining_energy() {
    let Some(gpu) = gpu() else { return };
    let mesh = GiMeshTracePass::new(&gpu.device, &gpu.queue).unwrap();
    let h = 2.1;
    let half = Vec3::splat(0.5);
    let scene = scene(gpu, &room(h, half, half, half, half));
    let mut pass = GiPass::new(
        &gpu.device,
        GiConfig {
            finest_spacing: SPACING,
            needs_tlas: true,
        },
    );
    let eye = DVec3::new(0.1, 0.3, -0.2);
    let clipmap = Clipmap::around(eye, SPACING);
    let mut means = Vec::new();
    // Ten rounds of the coarsest levels' updates (13824 at 340 a frame).
    for n in 0..400 {
        frame(gpu, &mut pass, &mesh, &scene, eye, n, None, None);
        if n % 100 == 99 {
            let words = common::read(&gpu.device, &gpu.queue, pass.buffers()[1]);
            let probes = inside(&words, &clipmap, f64::from(h));
            let mut sum = 0.0;
            for &(index, p) in &probes {
                for d in DIRECTIONS {
                    let e = probe(&words, index, d).0;
                    assert!(
                        e.max_element() < 1.02,
                        "probe {index} at {p} toward {d} gained energy: {e}"
                    );
                    sum += f64::from(e.x);
                }
            }
            means.push(sum / (probes.len() * DIRECTIONS.len()) as f64);
        }
    }
    eprintln!("mean irradiance / pi by 100 frames: {means:?}");
    assert!(
        means.windows(2).all(|w| w[1] >= w[0] - 0.01),
        "not converging: {means:?}"
    );
    assert!(*means.last().unwrap() > 0.85, "{means:?}");
}

/// A Cornell box (red wall at -x, green at +x, white others; albedo 0.7)
/// lit by a white point light near its ceiling, no sky: probes see the
/// walls' bounce of the light, tinted by each wall. Near the red wall,
/// irradiance from it (facing -x) is red; near the green wall, green; and
/// every probe holds light (its mean over directions: L1 harmonics ring
/// under the ceiling's bright spot over the light, so one direction alone
/// can read dark).
#[test]
fn a_cornell_box_bleeds_its_walls_colours() {
    let Some(gpu) = gpu() else { return };
    let mesh = GiMeshTracePass::new(&gpu.device, &gpu.queue).unwrap();
    let h = 2.1;
    let (red, green, white) = (
        Vec3::new(0.7, 0.05, 0.05),
        Vec3::new(0.05, 0.7, 0.05),
        Vec3::splat(0.7),
    );
    let scene = scene(gpu, &room(h, red, green, white, Vec3::ZERO));
    let light = common::point_light(Vec3::new(0.0, 1.6, 0.0), 20.0, 10.0);
    let lights = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("light"),
            contents: bytemuck::cast_slice(&light),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let mut pass = GiPass::new(
        &gpu.device,
        GiConfig {
            finest_spacing: SPACING,
            needs_tlas: true,
        },
    );
    let eye = DVec3::new(0.1, 0.3, -0.2);
    for n in 0..60 {
        frame(gpu, &mut pass, &mesh, &scene, eye, n, Some(&lights), None);
    }
    let clipmap = Clipmap::around(eye, SPACING);
    let words = common::read(&gpu.device, &gpu.queue, pass.buffers()[1]);
    let probes = inside(&words, &clipmap, f64::from(h));
    let (mut by_red, mut by_green) = (0, 0);
    for &(index, p) in &probes {
        let mean = (probe(&words, index, Vec3::NEG_Y).0 + probe(&words, index, Vec3::Y).0) * 0.5;
        assert!(
            mean.min_element() > 0.05,
            "probe {index} at {p}: no light ({mean})"
        );
        if (index / helio_pass_gi::LEVEL_PROBES) != 0 {
            continue;
        }
        if p.x < -1.2 {
            by_red += 1;
            let e = probe(&words, index, Vec3::NEG_X).0;
            assert!(
                e.x > 2.0 * e.y && e.x > 2.0 * e.z,
                "probe {index} at {p}, by the red wall: {e}"
            );
        } else if p.x > 1.2 {
            by_green += 1;
            let e = probe(&words, index, Vec3::X).0;
            assert!(
                e.y > 2.0 * e.x && e.y > 2.0 * e.z,
                "probe {index} at {p}, by the green wall: {e}"
            );
        }
    }
    assert!(
        by_red > 20 && by_green > 20,
        "{by_red} by red, {by_green} by green"
    );
}

/// Another tracer's ground (height 0, glowing: radiance about 2 up) over a
/// brighter mesh slab under it (height -1), and that tracer reporting a far,
/// bright hit (10 km, radiance 4) for rays up, under a black mesh ceiling
/// at height 3: the meshes' tracer keeps the nearer of each. A probe
/// between them sees the ground from below and the black ceiling above.
#[test]
fn the_nearer_of_a_mesh_and_another_tracers_hit_stands() {
    let Some(gpu) = gpu() else { return };
    let mesh = GiMeshTracePass::new(&gpu.device, &gpu.queue).unwrap();
    // Planes wide enough that no ray up passes beside the ceiling.
    let plane = |y: f32, normal: Vec3, emissive: f32| Quad {
        corner: Vec3::new(-4000.0, y, -4000.0),
        u: Vec3::X * 8000.0,
        v: Vec3::Z * 8000.0,
        normal,
        base_color: Vec3::ZERO,
        emissive: Vec3::splat(emissive),
        ..Default::default()
    };
    let scene = scene(
        gpu,
        &[plane(3.0, Vec3::NEG_Y, 0.0), plane(-1.0, Vec3::Y, 8.0)],
    );
    let mut pass = GiPass::new(
        &gpu.device,
        GiConfig {
            finest_spacing: SPACING,
            needs_tlas: true,
        },
    );
    let eye = DVec3::new(0.1, 1.3, -0.2);
    let ground = |rays: &[u32]| -> Vec<u32> {
        let mut hits = vec![0u32; rays.len() / 2];
        for (k, ray) in rays.chunks(8).enumerate() {
            let f = |w: u32| f32::from_bits(w);
            let origin = Vec3::new(f(ray[0]), f(ray[1]), f(ray[2])).as_dvec3() + eye;
            let dir = Vec3::new(f(ray[4]), f(ray[5]), f(ray[6]));
            if origin.y <= 0.0 {
                // In the ground: a back face at once.
                hits[k * 4..k * 4 + 4].copy_from_slice(&[1e-3f32.to_bits(), pack(dir), 0, 1]);
                continue;
            }
            // Albedo 128/255 giving off 4 times it (64 sixteenths) down
            // there; albedo 1 giving off 4 10 km up.
            let hit = if dir.y < 0.0 {
                [
                    ((-origin.y / f64::from(dir.y)) as f32).to_bits(),
                    pack(Vec3::Y),
                    128 | (128 << 8) | (128 << 16),
                    1 | (64 << 24),
                ]
            } else {
                [10_000f32.to_bits(), pack(-dir), 0xff_ff_ff, 1 | (64 << 24)]
            };
            hits[k * 4..k * 4 + 4].copy_from_slice(&hit);
        }
        hits
    };
    for n in 0..24 {
        frame(gpu, &mut pass, &mesh, &scene, eye, n, None, Some(&ground));
    }
    let clipmap = Clipmap::around(eye, SPACING);
    let words = common::read(&gpu.device, &gpu.queue, pass.buffers()[1]);
    let below = 128.0 / 255.0 * 4.0;
    let mut between = 0;
    for index in 0..PROBES {
        let (_, state) = probe(&words, index, Vec3::Y);
        let p = clipmap.position(index);
        if state >> 8 == 0 || p.y < 0.5 || p.y > 2.5 || p.x.abs() > 20.0 || p.z.abs() > 20.0 {
            continue;
        }
        between += 1;
        let down = probe(&words, index, Vec3::NEG_Y).0;
        let up = probe(&words, index, Vec3::Y).0;
        assert!(
            (down - Vec3::splat(below)).abs().max_element() < 0.05 * below,
            "probe {index} at {p}: from below {down}, want {below}"
        );
        // 64 rays project the step from the light below to the black above
        // onto L1 harmonics within a few percent (as over flat ground).
        assert!(
            up.max_element() < 0.05 * below,
            "probe {index} at {p}: from above {up}, want the black ceiling"
        );
    }
    assert!(between > 1000, "{between} probes between");
}

/// A mipped RGBA8 texture whose alpha at mip 0 is `alpha(x, y)` over
/// `size` x `size` texels (white colour); its mips box-filtered.
fn alpha_texture(gpu: &Gpu, size: u32, alpha: impl Fn(u32, u32) -> u8) -> wgpu::Texture {
    let levels = size.ilog2() + 1;
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cut-out alpha"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut level: Vec<f32> = (0..size * size)
        .map(|i| f32::from(alpha(i % size, i / size)) / 255.0)
        .collect();
    let mut side = size;
    for mip in 0..levels {
        let texels: Vec<u8> = level
            .iter()
            .flat_map(|a| [255, 255, 255, (a * 255.0).round() as u8])
            .collect();
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * side),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
        if side == 1 {
            break;
        }
        let half = side / 2;
        level = (0..half * half)
            .map(|i| {
                let (x, y) = (2 * (i % half), 2 * (i / half));
                (level[(y * side + x) as usize]
                    + level[(y * side + x + 1) as usize]
                    + level[((y + 1) * side + x) as usize]
                    + level[((y + 1) * side + x + 1) as usize])
                    / 4.0
            })
            .collect();
        side = half;
    }
    texture
}

/// The material table of one alpha-tested, double-sided black material
/// (row 0) whose base colour texture is `texture` (slot 0, repeating), as
/// the renderer binds it: `GpuMaterial` rows, and the single-row texture
/// metadata whose `params.w = -1` takes the row's texture ids and cutoff
/// 0.5.
fn cut_out_materials(
    gpu: &Gpu,
    mesh: &GiMeshTracePass,
    texture: &wgpu::Texture,
) -> wgpu::BindGroup {
    let material = helio_mats::GpuMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        emissive: [0.0; 4],
        roughness_metallic: [1.0, 0.0, 1.5, 0.0],
        tex_base_color: 0,
        tex_normal: helio_mats::GpuMaterial::NO_TEXTURE,
        tex_roughness: helio_mats::GpuMaterial::NO_TEXTURE,
        tex_emissive: helio_mats::GpuMaterial::NO_TEXTURE,
        tex_occlusion: helio_mats::GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: helio_mats::FLAG_ALPHA_TEST | helio_mats::FLAG_DOUBLE_SIDED,
        material_class: 0,
        class_params: [0.0; 4],
    };
    let init = |label, contents: &[u8]| {
        gpu.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE,
            })
    };
    let rows = init("materials", bytemuck::bytes_of(&material));
    let mut metadata = [0u32; helio_mats::vg_alpha::MATERIAL_TEXTURE_WORDS as usize];
    metadata[metadata.len() - 1] = (-1f32).to_bits();
    let metadata = init("material texture metadata", bytemuck::cast_slice(&metadata));
    let binding = helio_mats::MaterialBindingConfig::for_device(&gpu.device);
    let view = texture.create_view(&Default::default());
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let views = vec![&view; binding.max_textures];
    let samplers = vec![&sampler; binding.max_textures];
    mesh.materials(
        &gpu.device,
        &rows,
        helio_mats::MaterialTextureBindings {
            material_textures: &metadata,
            texture_views: &views,
            samplers: &samplers,
            version: 0,
        },
    )
}

/// A sky of radiance 1 over a black, alpha-tested, double-sided canopy at
/// height 3 (its texture tiled every 2 m): under it, a probe's irradiance
/// from above is the sky's through the canopy's holes. A canopy of leaves
/// alone (alpha 1) shades the probes; one of holes alone (alpha 0) is not
/// there; a checker of 8-texel leaves and holes lets about half through,
/// near and far alike: its coarse mips' alpha is 0.5, which a fixed cutoff
/// would take for all leaves.
#[test]
fn a_cut_out_canopy_shades_its_probes_through_its_holes() {
    let Some(gpu) = gpu() else { return };
    let eye = DVec3::new(0.1, 1.3, -0.2);
    let clipmap = Clipmap::around(eye, SPACING);
    // Mean irradiance / pi from above at the finest level's probes under
    // the canopy, after every one of them updated.
    let under = |texture: Option<wgpu::Texture>| -> (usize, f32) {
        let mesh = GiMeshTracePass::new(&gpu.device, &gpu.queue).unwrap();
        let canopy = Quad {
            corner: Vec3::new(-4000.0, 3.0, -4000.0),
            u: Vec3::X * 8000.0,
            v: Vec3::Z * 8000.0,
            normal: Vec3::NEG_Y,
            uv: 4000.0,
            flags: RAY_INSTANCE_MASKED | RAY_INSTANCE_TWO_SIDED,
            ..Default::default()
        };
        // No canopy: a black floor far below, out of the probes' sight up.
        let quads = if texture.is_some() {
            vec![canopy]
        } else {
            vec![Quad {
                corner: Vec3::new(-1.0, -500.0, -1.0),
                u: Vec3::X,
                v: Vec3::Z,
                normal: Vec3::Y,
                ..Default::default()
            }]
        };
        let mut scene = scene(gpu, &quads);
        scene.sky = Vec3::ONE;
        scene.materials = texture.map(|t| cut_out_materials(gpu, &mesh, &t));
        let mut pass = GiPass::new(
            &gpu.device,
            GiConfig {
                finest_spacing: SPACING,
                needs_tlas: true,
            },
        );
        for n in 0..48 {
            frame(gpu, &mut pass, &mesh, &scene, eye, n, None, None);
        }
        let words = common::read(&gpu.device, &gpu.queue, pass.buffers()[1]);
        let mut sum = 0.0;
        let mut count = 0;
        for index in 0..helio_pass_gi::LEVEL_PROBES {
            let (_, state) = probe(&words, index, Vec3::Y);
            let p = clipmap.position(index);
            if state >> 8 == 0 || state & 1 == 0 || p.y < 0.5 || p.y > 2.5 {
                continue;
            }
            sum += probe(&words, index, Vec3::Y).0.x;
            count += 1;
        }
        (count, sum / count.max(1) as f32)
    };
    let (n, open) = under(None);
    assert!(n > 1000 && open > 0.5, "{n} probes, open sky {open}");
    let (n, leaves) = under(Some(alpha_texture(gpu, 64, |_, _| 255)));
    assert!(
        n > 1000 && leaves < 0.03 * open,
        "{n} probes under leaves: {leaves} of the open sky's {open}"
    );
    let (n, holes) = under(Some(alpha_texture(gpu, 64, |_, _| 0)));
    assert!(
        n > 1000 && (holes - open).abs() < 0.02 * open,
        "{n} probes under holes: {holes} of the open sky's {open}"
    );
    let (n, half) = under(Some(alpha_texture(gpu, 64, |x, y| {
        if (x / 8 + y / 8) % 2 == 0 {
            255
        } else {
            0
        }
    })));
    eprintln!(
        "irradiance / pi from above: open {open}, leaves {leaves}, holes {holes}, checker {half}"
    );
    assert!(
        n > 1000 && (half / open - 0.5).abs() < 0.1,
        "{n} probes under the checker: {half} of the open sky's {open}"
    );
}
