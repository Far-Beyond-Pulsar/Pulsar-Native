//! The page mask `sphere_pages` (in `shadow.wgsl`, mirrored in
//! `shadow_dirty.wgsl`) is what lets a moved shadow caster redraw only part of
//! a face. Its one job is to be conservative: every point of the sphere must
//! land in a page whose bit is set, or the shadow of a moving object keeps
//! stale depth. Run on the GPU, over random views and spheres.

use glam::{Mat4, Vec3, Vec4};

const SHADOW_WGSL: &str = include_str!("../shaders/shadow.wgsl");
const SHADOW_DIRTY_WGSL: &str =
    include_str!("../../shadow-dirty/shaders/shadow_dirty.wgsl");

/// The page-mask helpers of `source`: from `fn ratio_range` through the end of
/// `fn sphere_pages`.
fn page_helpers(source: &str) -> &str {
    // `shadow_dirty.wgsl` declares the grid constants just ahead of the helpers.
    let start = source
        .find("const PAGE_GRID")
        .or_else(|| source.find("fn ratio_range"))
        .expect("ratio_range");
    let sphere = source.find("fn sphere_pages").expect("sphere_pages");
    let end = sphere + source[sphere..].find("\n}").expect("end of sphere_pages") + 2;
    &source[start..end]
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Case {
    matrix: [f32; 16],
    sphere: [f32; 4],
}

fn pages_on_gpu(cases: &[Case], helpers: &str) -> Option<Vec<[u32; 2]>> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()?;
        let source = format!(
            "{helpers}\n\
             struct Case {{ m: mat4x4f, sphere: vec4f }}\n\
             @group(0) @binding(0) var<storage, read> cases: array<Case>;\n\
             @group(0) @binding(1) var<storage, read_write> result: array<vec2u>;\n\
             @compute @workgroup_size(64)\n\
             fn main(@builtin(global_invocation_id) g: vec3u) {{\n\
                 if g.x >= arrayLength(&cases) {{ return; }}\n\
                 let c = cases[g.x];\n\
                 result[g.x] = sphere_pages(c.m, c.sphere.xyz, c.sphere.w);\n\
             }}\n"
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("page mask test"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("page mask test"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let input = wgpu::util::DeviceExt::create_buffer_init(
            &device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("cases"),
                contents: bytemuck::cast_slice(cases),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );
        let bytes = (cases.len() * 8) as u64;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pages"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups((cases.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, bytes);
        queue.submit(Some(encoder.finish()));
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mapped = staging.slice(..).get_mapped_range().unwrap();
        let words: &[u32] = bytemuck::cast_slice(&mapped);
        Some(words.chunks(2).map(|w| [w[0], w[1]]).collect())
    })
}

fn view(rng: &mut Rng, perspective: bool) -> Mat4 {
    let eye = Vec3::new(
        rng.range(-5.0, 5.0),
        rng.range(2.0, 8.0),
        rng.range(-5.0, 5.0),
    );
    let target = Vec3::new(rng.range(-2.0, 2.0), 0.0, rng.range(-2.0, 2.0));
    let look = Mat4::look_at_rh(eye, target, Vec3::Y);
    let projection = if perspective {
        Mat4::perspective_rh(rng.range(0.6, 1.6), 1.0, 0.1, 100.0)
    } else {
        let half = rng.range(4.0, 14.0);
        Mat4::orthographic_rh(-half, half, -half, half, 0.1, 100.0)
    };
    projection * look
}

fn bit(mask: [u32; 2], page: u32) -> bool {
    let word = if page < 32 { mask[0] } else { mask[1] };
    (word >> (page & 31)) & 1 != 0
}

#[test]
fn page_masks_contain_every_point_of_the_sphere() {
    let mut rng = Rng(0x5eed);
    let mut cases = Vec::new();
    for i in 0..4000 {
        let matrix = view(&mut rng, i % 2 == 0);
        let center = Vec3::new(
            rng.range(-8.0, 8.0),
            rng.range(-1.0, 6.0),
            rng.range(-8.0, 8.0),
        );
        let radius = rng.range(0.05, 3.0);
        cases.push(Case {
            matrix: matrix.to_cols_array(),
            sphere: [center.x, center.y, center.z, radius],
        });
    }
    for (label, source) in [
        ("shadow.wgsl", SHADOW_WGSL),
        ("shadow_dirty.wgsl", SHADOW_DIRTY_WGSL),
    ] {
        let Some(masks) = pages_on_gpu(&cases, page_helpers(source)) else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: page mask");
            return;
        };
        let mut sample = Rng(7);
        for (case, mask) in cases.iter().zip(masks) {
            let matrix = Mat4::from_cols_array(&case.matrix);
            let center = Vec3::new(case.sphere[0], case.sphere[1], case.sphere[2]);
            for _ in 0..48 {
                // A point in the ball.
                let direction = loop {
                    let d = Vec3::new(
                        sample.range(-1.0, 1.0),
                        sample.range(-1.0, 1.0),
                        sample.range(-1.0, 1.0),
                    );
                    if d.length_squared() <= 1.0 && d.length_squared() > 1e-6 {
                        break d;
                    }
                };
                let point = center + direction * case.sphere[3];
                let clip: Vec4 = matrix * point.extend(1.0);
                if clip.w <= 1e-4 {
                    continue;
                }
                let ndc = clip.truncate() / clip.w;
                if ndc.x.abs() > 1.0 || ndc.y.abs() > 1.0 {
                    continue;
                }
                let column = ((ndc.x * 0.5 + 0.5) * 8.0).floor().clamp(0.0, 7.0) as u32;
                let row = ((ndc.y * 0.5 + 0.5) * 8.0).floor().clamp(0.0, 7.0) as u32;
                assert!(
                    bit(mask, row * 8 + column),
                    "{label}: point {point:?} (ndc {ndc:?}) falls in page ({column}, {row}) \
                     outside mask {mask:?} for sphere {:?}",
                    case.sphere
                );
            }
        }
    }
}

#[test]
fn a_small_sphere_dirties_few_pages_and_a_huge_one_all() {
    let matrix = Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0)
        * Mat4::look_at_rh(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y);
    let cases = [
        Case {
            matrix: matrix.to_cols_array(),
            sphere: [0.0, 0.0, 0.0, 0.3],
        },
        Case {
            matrix: matrix.to_cols_array(),
            sphere: [0.0, 0.0, 9.0, 4.0],
        },
        Case {
            // Behind the camera: nothing to prove, everything is acceptable.
            matrix: matrix.to_cols_array(),
            sphere: [0.0, 0.0, 30.0, 1.0],
        },
    ];
    let Some(masks) = pages_on_gpu(&cases, page_helpers(SHADOW_WGSL)) else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: page mask");
        return;
    };
    let count = |m: [u32; 2]| m[0].count_ones() + m[1].count_ones();
    assert!(
        count(masks[0]) >= 1 && count(masks[0]) <= 8,
        "{:?}",
        masks[0]
    );
    assert_eq!(
        count(masks[1]),
        64,
        "a sphere around the camera covers every page"
    );
}
