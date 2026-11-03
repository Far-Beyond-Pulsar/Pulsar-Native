//! Does light stay on the lit side of an occluder?
//!
//! Each scene is a glowing emitter block, an opaque wall and empty space,
//! rendered by the real [`RadianceCascades2DPass`] through a `RenderGraph`.
//! The pass models no bounce light: occluders absorb, so a texel with no line
//! of sight to the emitter must read black. Light reaching a shadowed texel is
//! a leak through the wall (Helio#174); a dark band between the emitter and an
//! open, lit region is ringing.

use std::sync::{Arc, OnceLock};

use bytemuck::{Pod, Zeroable};
use helio_core::{GpuCameraUniforms, RenderGraph, SceneInput};
use helio_pass_radiance_cascades_2d::{RadianceCascades2DPass, RadianceCascadesConfig};
use wgpu::util::DeviceExt;

const W: u32 = 128;
const H: u32 = 128;

/// Mirrors the pass's `Emitter` WGSL struct (32-byte array stride).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuEmitter {
    pos: [f32; 2],
    radius: f32,
    rgb: [f32; 3],
    _pad: [f32; 2],
}

struct Input {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    camera: wgpu::Buffer,
    camera_data: GpuCameraUniforms,
}

impl SceneInput for Input {
    fn world_origin(&self) -> glam::DVec3 {
        glam::DVec3::ZERO
    }
    fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }
    fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
    fn frame_count(&self) -> u64 {
        0
    }
    fn camera(&self) -> &wgpu::Buffer {
        &self.camera
    }
    fn camera_data(&self) -> &GpuCameraUniforms {
        &self.camera_data
    }
    fn camera_generation(&self) -> u64 {
        0
    }
    fn scene_buffers(&self) -> &helio_core::SceneBufferProjection {
        static EMPTY: OnceLock<helio_core::SceneBufferProjection> = OnceLock::new();
        EMPTY.get_or_init(helio_core::SceneBufferProjection::empty)
    }
}

/// Occluders in scene-texel coordinates (x right, y down), one cell per texel.
struct Scene {
    occluded: Vec<bool>,
    emitter_block: ([u32; 2], [u32; 2]),
}

impl Scene {
    /// An emitter block at x 16..24, y 56..72 lit by one emitter at its
    /// centre, with a 4-texel wall at x 56..60 open over `door` rows
    /// (`0..H` leaves no wall).
    fn wall(door: std::ops::Range<u32>) -> Self {
        let mut scene = Self {
            occluded: vec![false; (W * H) as usize],
            emitter_block: ([16, 56], [24, 72]),
        };
        let ([x0, y0], [x1, y1]) = scene.emitter_block;
        for y in y0..y1 {
            for x in x0..x1 {
                scene.occluded[(y * W + x) as usize] = true;
            }
        }
        for y in (0..H).filter(|y| !door.contains(y)) {
            for x in 56..60 {
                scene.occluded[(y * W + x) as usize] = true;
            }
        }
        scene
    }

    fn is_wall(&self, x: i32, y: i32) -> bool {
        let ([x0, y0], [x1, y1]) = self.emitter_block;
        let in_block = (x0 as i32..x1 as i32).contains(&x) && (y0 as i32..y1 as i32).contains(&y);
        x >= 0
            && y >= 0
            && x < W as i32
            && y < H as i32
            && !in_block
            && self.occluded[(y as u32 * W + x as u32) as usize]
    }

    /// Per texel: whether a straight line from its centre reaches some
    /// point on the emitter block's boundary without crossing the wall.
    fn visibility(&self) -> Vec<bool> {
        let ([x0, y0], [x1, y1]) = self.emitter_block;
        let (x0, y0, x1, y1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
        let mut targets = Vec::new();
        for i in 0..=32 {
            let t = i as f32 / 32.0;
            targets.push([x0 + (x1 - x0) * t, y0]);
            targets.push([x0 + (x1 - x0) * t, y1]);
            targets.push([x0, y0 + (y1 - y0) * t]);
            targets.push([x1, y0 + (y1 - y0) * t]);
        }
        let sees = |from: [f32; 2], to: [f32; 2]| {
            let length = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt();
            let steps = (length * 4.0).ceil().max(1.0) as i32;
            !(0..=steps).any(|i| {
                let t = i as f32 / steps as f32;
                let x = from[0] + (to[0] - from[0]) * t;
                let y = from[1] + (to[1] - from[1]) * t;
                self.is_wall(x.floor() as i32, y.floor() as i32)
            })
        };
        (0..W * H)
            .map(|i| {
                let from = [(i % W) as f32 + 0.5, (i / W) as f32 + 0.5];
                targets.iter().any(|&to| sees(from, to))
            })
            .collect()
    }
}

/// Whether every texel within `radius` (Chebyshev) of `(x, y)` has `value`.
fn all_near(map: &[bool], x: u32, y: u32, radius: i32, value: bool) -> bool {
    (-radius..=radius).all(|dy| {
        (-radius..=radius).all(|dx| {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            nx < 0
                || ny < 0
                || nx >= W as i32
                || ny >= H as i32
                || map[(ny as u32 * W + nx as u32) as usize] == value
        })
    })
}

/// Renders `scene` and returns cascade-0 radiance luminance per texel.
async fn render(scene: &Scene) -> Option<Vec<f32>> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: radiance cascades light leak");
        return None;
    };
    let (device, queue) = adapter
        .request_device(&Default::default())
        .await
        .expect("adapter must create a device");
    device.on_uncaptured_error(Arc::new(|error| {
        panic!("radiance cascades GPU validation error: {error:?}");
    }));
    let device = Arc::new(device);
    let queue = Arc::new(queue);

    // Occupancy rows are world-space, Y up: texel row y is grid row H-1-y.
    let mut occupancy = vec![0u32; (W * H).div_ceil(32) as usize];
    for y in 0..H {
        for x in 0..W {
            if scene.occluded[(y * W + x) as usize] {
                let cell = (H - 1 - y) * W + x;
                occupancy[(cell / 32) as usize] |= 1 << (cell % 32);
            }
        }
    }
    let ([x0, y0], [x1, y1]) = scene.emitter_block;
    let emitter = GpuEmitter {
        pos: [(x0 + x1) as f32 * 0.5, H as f32 - (y0 + y1) as f32 * 0.5],
        radius: 16.0,
        rgb: [4.0; 3],
        _pad: [0.0; 2],
    };
    let occupancy_buf = Arc::new(
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Test Occupancy"),
            contents: bytemuck::cast_slice(&occupancy),
            usage: wgpu::BufferUsages::STORAGE,
        }),
    );
    let emitters_buf = Arc::new(
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Test Emitters"),
            contents: bytemuck::bytes_of(&emitter),
            usage: wgpu::BufferUsages::STORAGE,
        }),
    );

    let mut pass = RadianceCascades2DPass::new(
        &device,
        &queue,
        RadianceCascadesConfig {
            scene_width: W,
            scene_height: H,
            max_emitters: 1,
            ..Default::default()
        },
        occupancy_buf,
        (W, H),
        1.0,
        [0.0, 0.0],
        emitters_buf,
    );
    pass.set_view(
        [W as f32 * 0.5, H as f32 * 0.5],
        [W as f32 * 0.5, H as f32 * 0.5],
    );
    pass.set_emitter_count(1);

    // Copies the finished radiance into a buffer, after the graph's submit.
    let copy_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Radiance Readback"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
@group(0) @binding(0) var radiance: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> out: array<vec4<f32>>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(radiance);
    if (gid.x < dims.x && gid.y < dims.y) {
        out[gid.y * dims.x + gid.x] = textureLoad(radiance, vec2<i32>(gid.xy), 0);
    }
}
"#
            .into(),
        ),
    });
    let copy_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Radiance Readback"),
        layout: None,
        module: &copy_shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let size = (W * H) as u64 * 16;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Radiance Output"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Radiance Readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let copy_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &copy_pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(pass.radiance_view()),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });

    let target = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("Unused Target"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let mut graph = RenderGraph::new(&device, &queue);
    graph.add_pass(Box::new(pass));
    graph.lock(W, H);
    let input = Input {
        device: device.clone(),
        queue: queue.clone(),
        camera: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Test Camera"),
            size: std::mem::size_of::<GpuCameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        camera_data: GpuCameraUniforms::zeroed(),
    };
    graph.execute(&input, &target, &target).unwrap();

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut copy = encoder.begin_compute_pass(&Default::default());
        copy.set_pipeline(&copy_pipeline);
        copy.set_bind_group(0, &copy_group, &[]);
        copy.dispatch_workgroups(W.div_ceil(8), H.div_ceil(8), 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let texels: Vec<[f32; 4]> =
        bytemuck::cast_slice(&readback.slice(..).get_mapped_range().unwrap()).to_vec();
    Some(texels.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect())
}

fn at(radiance: &[f32], x: u32, y: u32) -> f32 {
    radiance[(y * W + x) as usize]
}

#[test]
fn closed_wall_keeps_the_far_side_dark() {
    let scene = Scene::wall(0..0);
    let Some(radiance) = pollster::block_on(render(&scene)) else {
        return;
    };
    // Open space just before the wall sees the emitter.
    let lit = (48..56)
        .flat_map(|x| (56..72).map(move |y| (x, y)))
        .map(|(x, y)| at(&radiance, x, y))
        .fold(f32::INFINITY, f32::min);
    assert!(lit > 0.01, "the emitter must light the near side: {lit}");
    let (leak, (x, y)) = (60..W)
        .flat_map(|x| (0..H).map(move |y| (x, y)))
        .map(|(x, y)| (at(&radiance, x, y), (x, y)))
        .fold((0.0f32, (0, 0)), |a, b| if b.0 > a.0 { b } else { a });
    assert!(
        leak <= lit * 0.01,
        "light leaked through a closed wall: {leak} at ({x}, {y}), near side {lit}"
    );
}

#[test]
fn doorway_lights_only_what_sees_the_emitter() {
    let scene = Scene::wall(60..68);
    let Some(radiance) = pollster::block_on(render(&scene)) else {
        return;
    };
    // Cascades blur a shadow edge over a few probe spacings, so only texels
    // whose whole neighbourhood agrees on visibility are classified.
    let visible = scene.visibility();
    let mut lit = Vec::new();
    let mut shadowed = Vec::new();
    for y in 0..H {
        for x in 60..W {
            if all_near(&visible, x, y, 4, true) {
                lit.push(at(&radiance, x, y));
            } else if all_near(&visible, x, y, 8, false) {
                shadowed.push(((x, y), at(&radiance, x, y)));
            }
        }
    }
    assert!(lit.len() > 1000 && shadowed.len() > 4000);
    let lit_mean = lit.iter().sum::<f32>() / lit.len() as f32;
    assert!(lit_mean > 0.01, "the doorway must pass light: {lit_mean}");
    // A single bilinear tap of the coarser cascade measured a 7% peak and a
    // 0.3% mean here; the bilinear fix measures 0.9% and 0.007%.
    let ((x, y), leak) =
        shadowed
            .iter()
            .copied()
            .fold(((0, 0), 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
    assert!(
        leak <= lit_mean * 0.02,
        "light reached a texel with no line of sight to the emitter: {leak} at ({x}, {y}), \
         lit mean {lit_mean}"
    );
    let shadow_mean = shadowed.iter().map(|s| s.1).sum::<f32>() / shadowed.len() as f32;
    assert!(
        shadow_mean <= lit_mean * 0.0005,
        "light patches in the shadow: mean {shadow_mean}, lit mean {lit_mean}"
    );
}

#[test]
fn open_space_falls_off_without_dark_bands() {
    let scene = Scene::wall(0..H);
    let Some(radiance) = pollster::block_on(render(&scene)) else {
        return;
    };
    // With no wall, the emitter subtends a shrinking angle along every line
    // leaving its centre, so radiance never rises again on the way out.
    let ([x0, y0], [x1, y1]) = scene.emitter_block;
    let centre = [(x0 + x1) as f32 * 0.5, (y0 + y1) as f32 * 0.5];
    for k in 0..32 {
        let angle = k as f32 / 32.0 * std::f32::consts::TAU;
        let profile: Vec<f32> = (14..60)
            .map(|r| {
                [
                    centre[0] + angle.cos() * r as f32,
                    centre[1] + angle.sin() * r as f32,
                ]
            })
            .take_while(|p| p[0] >= 0.0 && p[1] >= 0.0 && p[0] < W as f32 && p[1] < H as f32)
            .map(|p| at(&radiance, p[0] as u32, p[1] as u32))
            .collect();
        for pair in profile.windows(2) {
            assert!(
                pair[1] <= pair[0] * 1.05,
                "radiance rises away from the emitter at angle {angle}: {profile:?}"
            );
        }
    }
}
