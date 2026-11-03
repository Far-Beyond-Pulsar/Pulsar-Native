//! Execute the production lens WGSL through the real pass recording path.
//! GPU tests require an adapter and fail rather than silently skipping.

use super::*;
use wgpu::util::DeviceExt;

const SIZE: u32 = 128;
const NEAR: f32 = 0.1;
/// The range of distance-limited effects (`forward_far.w`); not a clip plane.
const FAR: f32 = 100.0;

/// The depth-buffer value of a surface `distance` metres away (the engine's
/// reversed depth with no far plane: `near / distance`, sky 0).
fn raw_depth(distance: f32) -> f32 {
    if distance.is_finite() {
        NEAR / distance
    } else {
        helio_core::depth::CLEAR
    }
}
const VIEW: u32 = 7;

/// Tests create their own devices; creating several concurrently hangs some
/// drivers (observed on NVIDIA Vulkan), so GPU tests run one at a time.
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// An image and the view distance (metres, `f32::INFINITY` for sky) of every
/// pixel, row-major.
struct Scene {
    color: Vec<[f32; 3]>,
    distance: Vec<f32>,
}

impl Scene {
    fn new(color: impl Fn(u32, u32) -> [f32; 3], distance: impl Fn(u32, u32) -> f32) -> Self {
        let pixels = || (0..SIZE).flat_map(|y| (0..SIZE).map(move |x| (x, y)));
        Self {
            color: pixels().map(|(x, y)| color(x, y)).collect(),
            distance: pixels().map(|(x, y)| distance(x, y)).collect(),
        }
    }

    /// A fine two-tone checker: every pixel differs from its neighbours, so
    /// any blur shows.
    fn checker(x: u32, y: u32) -> [f32; 3] {
        let v = if (x + y).is_multiple_of(2) { 0.2 } else { 2.0 };
        [v, v * 0.5, v * 2.0]
    }
}

struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pass: LensPass,
    camera: wgpu::Buffer,
    /// Aperture shape array handed to the pass; none binds the neutral layer.
    shapes: Option<wgpu::TextureView>,
    /// Mask array handed to the pass; none binds the transparent layer.
    masks: Option<wgpu::TextureView>,
    /// Prescription bank handed to the pass; none binds an empty one.
    prescriptions: Option<PrescriptionBank>,
    _serial: std::sync::MutexGuard<'static, ()>,
}

/// One recorded frame's results.
struct Frame {
    output: Vec<u16>,
    coc: Vec<[f32; 4]>,
}

impl Frame {
    fn pixel(&self, x: u32, y: u32) -> [f32; 3] {
        let i = ((y * SIZE + x) * 4) as usize;
        [
            f16_to_f32(self.output[i]),
            f16_to_f32(self.output[i + 1]),
            f16_to_f32(self.output[i + 2]),
        ]
    }

    fn coc(&self, x: u32, y: u32) -> [f32; 4] {
        self.coc[(y * SIZE + x) as usize]
    }
}

fn bytes<T: bytemuck::Pod>(rows: &[T]) -> &[u8] {
    bytemuck::cast_slice(rows)
}

impl Rig {
    fn new() -> Self {
        Self::with_user(None)
    }

    /// A rig whose lens shader has the USER hooks `snippet` defines.
    fn with_user(snippet: Option<&str>) -> Self {
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
        let projection = helio_core::depth::perspective(0.8, 1.0, NEAR);
        let camera = helio_core::GpuCameraUniforms::new(
            glam::Mat4::IDENTITY,
            projection,
            glam::Vec3::ZERO,
            NEAR,
            FAR,
            0,
            [0.0; 2],
            glam::Mat4::IDENTITY,
        )
        .with_view_id(VIEW);
        let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("lens test cameras"),
            contents: bytes(&[camera, camera]),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let pass = LensPass::new_with_user(&device, SIZE, SIZE, snippet);
        Self {
            device,
            queue,
            pass,
            camera,
            shapes: None,
            masks: None,
            prescriptions: None,
            _serial: serial,
        }
    }

    fn color_view(&self, scene: &Scene) -> wgpu::TextureView {
        let mut texels = vec![0u16; (SIZE * SIZE * 4) as usize];
        for (i, c) in scene.color.iter().enumerate() {
            for (k, v) in c.iter().enumerate() {
                texels[i * 4 + k] = f32_to_f16(*v);
            }
            texels[i * 4 + 3] = f32_to_f16(1.0);
        }
        self.device
            .create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some("lens test colour"),
                    size: wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                },
                Default::default(),
                bytes(&texels),
            )
            .create_view(&Default::default())
    }

    /// The raw depth-buffer values of the engine projection (the pass reads
    /// them as plain floats).
    fn depth_view(&self, scene: &Scene) -> wgpu::TextureView {
        let raw: Vec<f32> = scene.distance.iter().map(|&d| raw_depth(d)).collect();
        self.device
            .create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some("lens test depth"),
                    size: wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                Default::default(),
                bytes(&raw),
            )
            .create_view(&Default::default())
    }

    fn read_buffer(&self, buffer: &wgpu::Buffer, size: u64) -> Vec<u8> {
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lens test readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, size);
        self.queue.submit([encoder.finish()]);
        self.map(&readback)
    }

    fn map(&self, readback: &wgpu::Buffer) -> Vec<u8> {
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        let data = readback.slice(..).get_mapped_range().unwrap().to_vec();
        data
    }

    fn read_texture(&self, view: &wgpu::TextureView) -> Vec<u16> {
        let (w, h) = (view.texture().width(), view.texture().height());
        let row_bytes = w * 8;
        assert_eq!(row_bytes % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lens test texture readback"),
            size: u64::from(row_bytes * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            view.texture().as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        bytemuck::cast_slice(&self.map(&readback)).to_vec()
    }

    /// Record the pass once. `lenses` and `elements` are the SceneDB rows,
    /// or `None` for a graph that has neither buffer.
    fn record(
        &mut self,
        scene: &Scene,
        lenses: Option<&[LensRow]>,
        elements: Option<&[LensElementRow]>,
    ) -> Frame {
        let color = self.color_view(scene);
        let depth = self.depth_view(scene);
        let buffer = |rows: &[u8]| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("lens test rows"),
                    contents: rows,
                    usage: wgpu::BufferUsages::STORAGE,
                })
        };
        let lenses = lenses.map(|rows| buffer(bytes(rows)));
        let elements = elements.map(|rows| buffer(bytes(rows)));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.pass.record(
            &self.device,
            &mut CommandRecorder::from_encoder(&mut encoder),
            Some(&color),
            LensInputs {
                camera: Some(&self.camera),
                depth: Some(&depth),
                lenses: lenses.as_ref(),
                elements: elements.as_ref(),
                aperture_shapes: self.shapes.as_ref(),
                masks: self.masks.as_ref(),
                prescriptions: self.prescriptions.as_ref().map(PrescriptionBank::buffer),
            },
        );
        self.queue.submit([encoder.finish()]);
        let output = self.read_texture(&self.pass.targets.output);
        let coc = self
            .read_texture(&self.pass.targets.coc)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|t| {
                [
                    f16_to_f32(t[0]),
                    f16_to_f32(t[1]),
                    f16_to_f32(t[2]),
                    f16_to_f32(t[3]),
                ]
            })
            .collect();
        Frame { output, coc }
    }

    fn resolved(&self) -> ResolvedLens {
        let size = std::mem::size_of::<ResolvedLens>() as u64;
        bytemuck::pod_read_unaligned(&self.read_buffer(&self.pass.resolved, size))
    }

    fn args(&self) -> Vec<u32> {
        bytemuck::cast_slice(&self.read_buffer(&self.pass.args, ARGS_SIZE)).to_vec()
    }

    /// Frame time for autofocus easing.
    fn set_delta_time(&self, seconds: f32) {
        self.queue
            .write_buffer(&self.pass.frame, 0, bytes(&[seconds, 0.0, 0.0, 0.0]));
    }
}

/// The thin-lens circle of confusion in pixels for an `SIZE`-wide image.
fn expected_coc(lens: &LensRow, focus: f32, distance: f32) -> f32 {
    let f = lens.focal_length_mm * 1.0e-3;
    let aperture = f / lens.f_number;
    let distance = if distance.is_finite() {
        distance
    } else {
        1.0e5
    };
    let on_sensor = aperture * (distance - focus).abs() / distance * f / (focus - f);
    on_sensor / (lens.sensor_width_mm * 1.0e-3) * SIZE as f32 * (distance - focus).signum()
}

fn lens() -> LensRow {
    LensRow::new(VIEW)
}

#[test]
fn component_rows_have_the_wgsl_strides() {
    assert_eq!(std::mem::size_of::<LensRow>(), 96);
    assert_eq!(std::mem::offset_of!(LensRow, view_id), 0);
    assert_eq!(std::mem::offset_of!(LensRow, focal_length_mm), 16);
    assert_eq!(std::mem::offset_of!(LensRow, focus_distance_m), 36);
    assert_eq!(std::mem::offset_of!(LensRow, element_first), 56);
    assert_eq!(std::mem::offset_of!(LensRow, field_curvature), 72);
    assert_eq!(std::mem::offset_of!(LensRow, af_radius), 84);
    assert_eq!(std::mem::size_of::<LensElementRow>(), 160);
    assert_eq!(std::mem::offset_of!(LensElementRow, feather), 16);
    assert_eq!(std::mem::offset_of!(LensElementRow, params), 32);
    assert_eq!(std::mem::offset_of!(LensElementRow, region), 96);
    assert_eq!(std::mem::size_of::<ResolvedLens>(), 88);
    assert_eq!(std::mem::offset_of!(ResolvedLens, aberration_flags), 80);
    assert_eq!(std::mem::offset_of!(ResolvedLens, overscan), 84);
    assert_eq!(std::mem::offset_of!(LensRow, overscan), 88);
    assert_eq!(std::mem::offset_of!(ResolvedLens, natural_vignette), 20);
    assert_eq!(std::mem::offset_of!(ResolvedLens, required_overscan), 68);
    // A zero row is inert: never enabled, no element kind.
    let zero: LensRow = bytemuck::Zeroable::zeroed();
    assert_eq!(zero.enabled, 0);
    let zero: LensElementRow = bytemuck::Zeroable::zeroed();
    assert_eq!(zero.kind, element_kind::NONE);
}

#[test]
fn no_enabled_lens_for_the_view_passes_the_image_through_exactly() {
    let mut rig = Rig::new();
    // HDR values far above 1, so nothing here is clamped or tone mapped.
    let scene = Scene::new(
        |x, y| [x as f32 * 0.37, y as f32 * 3.1, 40.0 + (x ^ y) as f32],
        |x, _| 1.0 + x as f32 * 0.1,
    );
    let color = rig.color_view(&scene);
    let expected = rig.read_texture(&color);

    // A graph without the lens buffers.
    let frame = rig.record(&scene, None, None);
    assert_eq!(frame.output, expected, "missing buffers changed the image");
    let args = rig.args();
    assert_eq!(
        &args[..15],
        &[0; 15],
        "defocus dispatches must be empty with no lens"
    );
    assert!(args[15] > 0, "the copy is still dispatched");
    let resolved = rig.resolved();
    assert_eq!(
        resolved,
        ResolvedLens {
            focus_distance_m: resolved.focus_distance_m,
            ..Default::default()
        }
    );

    // A disabled row, and an enabled row belonging to another view.
    let mut disabled = lens();
    disabled.enabled = 0;
    let mut other = lens();
    other.view_id = VIEW + 1;
    let frame = rig.record(&scene, Some(&[disabled, other]), None);
    assert_eq!(
        frame.output, expected,
        "an unmatched lens changed the image"
    );
    assert_eq!(rig.resolved().enabled, 0);
}

#[test]
fn resolve_matches_the_first_enabled_row_of_the_view() {
    let mut rig = Rig::new();
    let scene = Scene::new(Scene::checker, |_, _| 3.0);
    let mut other_view = lens();
    other_view.view_id = VIEW + 1;
    other_view.f_number = 9.0;
    let mut disabled = lens();
    disabled.enabled = 0;
    disabled.f_number = 8.0;
    let mut wanted = lens();
    wanted.focal_length_mm = 85.0;
    wanted.f_number = 1.8;
    wanted.sensor_width_mm = 24.0;
    wanted.sensor_height_mm = 0.0;
    wanted.focus_distance_m = 4.0;
    wanted.aperture_blades = 9;
    wanted.aperture_rotation = 0.25;
    wanted.max_coc_px = 12.0;
    wanted.quality = 1;
    wanted.element_first = 3;
    wanted.element_count = 2;
    let mut later = lens();
    later.f_number = 5.6;
    rig.record(&scene, Some(&[other_view, disabled, wanted, later]), None);
    let resolved = rig.resolved();
    assert_eq!(resolved.enabled, 1);
    assert_eq!(resolved.f_number, 1.8);
    assert_eq!(resolved.focal_length_mm, 85.0);
    assert_eq!(resolved.sensor_width_mm, 24.0);
    assert_eq!(resolved.sensor_height_mm, 16.0, "zero height derives 3:2");
    assert!(
        (resolved.focus_distance_m - 4.0).abs() < 1.0e-4,
        "{resolved:?}"
    );
    assert_eq!(
        (resolved.aperture_blades, resolved.aperture_rotation),
        (9, 0.25)
    );
    assert_eq!((resolved.max_coc_px, resolved.quality), (12.0, 1));
    assert_eq!((resolved.element_first, resolved.element_count), (3, 2));
    let args = rig.args();
    assert!(
        args[..18].iter().all(|&n| n > 0),
        "an enabled lens dispatches every stage: {args:?}"
    );
}

#[test]
fn thin_lens_circle_of_confusion_matches_the_formula() {
    let mut rig = Rig::new();
    // Six depth bands from in front of the focus to the sky.
    let bands = [1.0, 2.0, 3.0, 5.0, 10.0, f32::INFINITY];
    let scene = Scene::new(Scene::checker, |x, _| bands[(x * 6 / SIZE) as usize]);
    let mut lens = lens();
    lens.focal_length_mm = 100.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 3.0;
    let frame = rig.record(&scene, Some(&[lens]), None);
    for (band, &distance) in bands.iter().enumerate() {
        let x = (band as u32 * SIZE + SIZE / 2) / 6;
        let expected = expected_coc(&lens, 3.0, distance);
        let [coc_a, coc_b, seam, transmission] = frame.coc(x, SIZE / 2);
        assert!(
            (coc_a - expected).abs() <= 0.01 * expected.abs() + 0.03,
            "depth {distance}: circle {coc_a}, expected {expected}"
        );
        assert_eq!(
            (coc_b, seam, transmission),
            (coc_a, 0.0, 1.0),
            "no seam without elements"
        );
    }
    // Near geometry is negative, far positive, the focal plane exactly sharp.
    assert!(frame.coc(10, 64)[0] < -10.0 && frame.coc(SIZE - 10, 64)[0] > 5.0);
    assert!(
        frame.coc(SIZE / 2 - 4, 64)[0].abs() < 0.03,
        "the focal plane is sharp"
    );

    // The aperture scales every circle; the clamp bounds them.
    lens.f_number = 2.8;
    let frame = rig.record(&scene, Some(&[lens]), None);
    let expected = expected_coc(&lens, 3.0, 1.0);
    assert!((frame.coc(10, 64)[0] - expected).abs() < 0.01 * expected.abs() + 0.03);
    lens.f_number = 1.4;
    lens.max_coc_px = 6.0;
    let frame = rig.record(&scene, Some(&[lens]), None);
    assert_eq!(frame.coc(10, 64)[0], -6.0);
}

#[test]
fn depth_rendered_below_image_resolution_keeps_foreground_focus() {
    let mut rig = Rig::new();
    // The depth buffer is a quarter-size texture; the foreground column's
    // footprint straddles two texels and must take the nearer.
    let scene = Scene::new(Scene::checker, |x, _| {
        if (60..68).contains(&x) {
            1.0
        } else {
            9.0
        }
    });
    let color = rig.color_view(&scene);
    let raw: Vec<f32> = (0..32 * 32)
        .map(|i| {
            raw_depth(if (15..17).contains(&(i % 32)) {
                1.0
            } else {
                9.0
            })
        })
        .collect();
    let depth = rig
        .device
        .create_texture_with_data(
            &rig.queue,
            &wgpu::TextureDescriptor {
                label: Some("lens test low-resolution depth"),
                size: wgpu::Extent3d {
                    width: 32,
                    height: 32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            Default::default(),
            bytes(&raw),
        )
        .create_view(&Default::default());
    let mut lens = lens();
    lens.focus_distance_m = 1.0;
    lens.focal_length_mm = 100.0;
    lens.f_number = 1.4;
    let lenses = rig
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("lens test rows"),
            contents: bytes(&[lens]),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let mut encoder = rig.device.create_command_encoder(&Default::default());
    rig.pass.record(
        &rig.device,
        &mut CommandRecorder::from_encoder(&mut encoder),
        Some(&color),
        LensInputs {
            camera: Some(&rig.camera),
            depth: Some(&depth),
            lenses: Some(&lenses),
            elements: None,
            aperture_shapes: None,
            masks: None,
            prescriptions: None,
        },
    );
    rig.queue.submit([encoder.finish()]);
    let coc = rig.read_texture(&rig.pass.targets.coc);
    let at = |x: u32| f16_to_f32(coc[((64 * SIZE + x) * 4) as usize]);
    assert!(at(62).abs() < 0.05, "foreground focus {}", at(62));
    assert!(at(20) > 3.0, "background blur {}", at(20));
}

#[test]
fn defocus_preserves_energy() {
    let mut rig = Rig::new();
    // A bright patch and a dimmer one on a dim floor, all on one plane far
    // behind the focus: a uniform circle of confusion of several pixels.
    let scene = Scene::new(
        |x, y| match (x, y) {
            (48..80, 48..80) => [8.0, 6.0, 4.0],
            (20..28, 90..98) => [3.0, 3.0, 12.0],
            _ => [0.05, 0.06, 0.07],
        },
        |_, _| f32::INFINITY,
    );
    let mut lens = lens();
    lens.focal_length_mm = 50.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 1.0;
    for quality in [0, 1] {
        lens.quality = quality;
        let frame = rig.record(&scene, Some(&[lens]), None);
        let expected = expected_coc(&lens, 1.0, f32::INFINITY);
        assert!(
            expected > 5.0 && expected < 8.0,
            "test geometry: circle {expected}"
        );
        assert!((frame.coc(64, 64)[0] - expected).abs() < 0.2);
        for channel in 0..3 {
            let sum = |pixel: &dyn Fn(u32, u32) -> f32| -> f32 {
                (0..SIZE)
                    .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
                    .map(|(x, y)| pixel(x, y))
                    .sum()
            };
            let before = sum(&|x, y| scene.color[(y * SIZE + x) as usize][channel]);
            let after = sum(&|x, y| frame.pixel(x, y)[channel]);
            assert!(
                (after / before - 1.0).abs() < 0.03,
                "quality {quality} channel {channel}: energy {before} -> {after}"
            );
        }
        // And it really did blur: the patch edge spreads outward and the
        // patch's own corner is no longer at full strength.
        assert!(frame.pixel(45, 64)[0] > 0.5, "no spill outside the patch");
        assert!(
            frame.pixel(49, 49)[0] < 7.0,
            "the patch corner stayed sharp"
        );
        assert!(
            frame.pixel(64, 64)[0] > 7.0,
            "the patch interior lost energy"
        );
    }
}

#[test]
fn near_field_blur_covers_what_is_behind_it() {
    let mut rig = Rig::new();
    // A bright near square, out of focus, in front of a dim in-focus wall.
    let scene = Scene::new(
        |x, y| {
            if (48..80).contains(&x) && (48..80).contains(&y) {
                [10.0; 3]
            } else {
                [0.5; 3]
            }
        },
        |x, y| {
            if (48..80).contains(&x) && (48..80).contains(&y) {
                0.6
            } else {
                3.0
            }
        },
    );
    let mut lens = lens();
    lens.focal_length_mm = 50.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 3.0;
    let frame = rig.record(&scene, Some(&[lens]), None);
    assert!(frame.coc(64, 64)[0] < -5.0, "near square is blurred");
    assert!(frame.coc(10, 10)[0].abs() < 0.03, "the wall is in focus");
    // The blurred square bleeds over the sharp wall, and the wall's far side
    // is untouched.
    assert!(
        frame.pixel(45, 64)[0] > 1.0,
        "near blur must cover the wall"
    );
    // Inside its own edge the blurred square fades into what is behind it.
    let inner = frame.pixel(49, 64)[0];
    assert!(inner > 3.0 && inner < 9.5, "inner edge {inner}");
    assert_eq!(frame.pixel(5, 5), [0.5; 3]);
    let total = |frame: &Frame| -> f32 {
        (0..SIZE)
            .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
            .map(|(x, y)| frame.pixel(x, y)[0])
            .sum()
    };
    let before: f32 = scene.color.iter().map(|c| c[0]).sum();
    // The square's light spreads over wall it hides; the displaced wall
    // light is the loss, so the total stays within a few percent.
    assert!(
        (total(&frame) / before - 1.0).abs() < 0.06,
        "{} vs {before}",
        total(&frame)
    );
}

/// Pixels of a row whose seam weight exceeds `threshold`.
fn seam_width(frame: &Frame, y: u32) -> u32 {
    (0..SIZE).filter(|&x| frame.coc(x, y)[2] > 0.02).count() as u32
}

#[test]
fn split_diopter_focuses_each_half_and_the_seam_follows_the_aperture() {
    let mut rig = Rig::new();
    // The left half-frame is a plane 2 m away, the right one 6 m away, both
    // carrying fine detail. The lens focuses 6 m; a diopter over the left
    // half brings the focus to 2 m there.
    let scene = Scene::new(Scene::checker, |x, _| if x < SIZE / 2 { 2.0 } else { 6.0 });
    let diopter = LensElementRow::diopter(
        2.0,
        LensRegion::HalfPlane {
            angle: std::f32::consts::PI,
            offset: 0.0,
        },
    )
    .with_mount_distance(0.5);
    let mut lens = lens();
    lens.focal_length_mm = 85.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 6.0;
    lens.element_count = 1;

    let wide = rig.record(&scene, Some(&[lens]), Some(&[diopter]));
    for x in (0..28).chain(100..SIZE) {
        let [coc_a, _, seam, _] = wide.coc(x, 64);
        assert!(
            coc_a.abs() < 0.1 && seam == 0.0,
            "column {x}: circle {coc_a}, seam {seam}"
        );
        assert_eq!(
            wide.pixel(x, 64),
            scene.color[(64 * SIZE + x) as usize].map(|v| f16_to_f32(f32_to_f16(v)))
        );
    }
    // Without the diopter the near plane is out of focus.
    lens.element_count = 0;
    let control = rig.record(&scene, Some(&[lens]), Some(&[diopter]));
    assert!(
        control.coc(10, 64)[0] < -4.0,
        "control circle {}",
        control.coc(10, 64)[0]
    );
    assert_ne!(
        control.pixel(10, 64),
        scene.color[(64 * SIZE + 10) as usize]
    );

    // The seam is the entrance pupil seen from the filter: A / e radians over
    // the sensor, so it narrows with the aperture.
    let widths: Vec<(f32, u32)> = [1.4, 5.6]
        .into_iter()
        .map(|f_number| {
            lens.element_count = 1;
            lens.f_number = f_number;
            let frame = rig.record(&scene, Some(&[lens]), Some(&[diopter]));
            // Both configurations meet at half weight in the middle.
            assert!(frame.coc(63, 64)[2] > 0.4 && frame.coc(63, 64)[2] <= 0.5);
            let aperture = 85.0e-3 / f_number;
            let feather = aperture / 0.5 * (85.0e-3 / 24.0e-3) * SIZE as f32;
            // Smoothstep: the weight is above 2% for 83% of the feather.
            let expected = 0.832 * feather;
            let width = seam_width(&frame, 64);
            assert!(
                (width as f32 - expected).abs() < 2.5,
                "f/{f_number}: seam {width} px, expected {expected}"
            );
            (f_number, width)
        })
        .collect();
    let ratio = widths[0].1 as f32 / widths[1].1 as f32;
    assert!(
        (ratio - 4.0).abs() < 0.6,
        "seam widths {widths:?} do not scale with 1/N"
    );

    // An authored feather replaces the physical one.
    lens.f_number = 1.4;
    let hard = diopter.with_feather(0.02);
    let frame = rig.record(&scene, Some(&[lens]), Some(&[hard]));
    assert!(
        seam_width(&frame, 64) <= 3,
        "authored feather ignored: {}",
        seam_width(&frame, 64)
    );
}

#[test]
fn a_diopter_region_blurs_each_pixel_by_its_own_depth() {
    let mut rig = Rig::new();
    // Inside the diopter (the left half): rows at 2 m, 6 m and 0.8 m. The
    // diopter brings 2 m into focus there, so only the 2 m rows are sharp and
    // the others blur by how far each is from 2 m - nearer than 2 m in front,
    // farther behind - not by a blur the region applies uniformly.
    let depth = |_: u32, y: u32| match y * 3 / SIZE {
        0 => 2.0,
        1 => 6.0,
        _ => 0.8,
    };
    let scene = Scene::new(Scene::checker, depth);
    let diopter = LensElementRow::diopter(
        2.0,
        LensRegion::HalfPlane {
            angle: std::f32::consts::PI,
            offset: 0.0,
        },
    )
    .with_feather(0.004);
    let mut lens = lens();
    lens.focal_length_mm = 85.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 6.0;
    lens.element_count = 1;
    lens.max_coc_px = 128.0;
    let (x, rows) = (20, [20, 64, 108]);
    let width = SIZE as f32;
    let thin_lens = |s1: f32, s2: f32| {
        let (f, a) = (85.0e-3, 85.0e-3 / 1.4);
        a * (s2 - s1).abs() / s2 * f / (s1 - f) / 36.0e-3 * width * (s2 - s1).signum()
    };
    for focus in [6.0, 20.0, 1.2] {
        // The region stays sharp at 2 m wherever the lens itself focuses.
        lens.focus_distance_m = focus;
        let frame = rig.record(&scene, Some(&[lens]), Some(&[diopter]));
        let circles = rows.map(|y| frame.coc(x, y)[0]);
        assert!(
            circles[0].abs() < 0.1,
            "focus {focus}: 2 m row circle {}",
            circles[0]
        );
        for (y, s2) in [(1, 6.0), (2, 0.8)] {
            let expected = thin_lens(2.0, s2);
            assert!(
                (circles[y] - expected).abs() < 0.02 * expected.abs().max(1.0),
                "focus {focus}: row at {s2} m circle {} expected {expected}",
                circles[y]
            );
        }
        assert!(
            circles[1] > 0.0 && circles[2] < 0.0,
            "behind blurs far, in front near: {circles:?}"
        );
        // Outside the region the lens's own focus holds.
        let outside = frame.coc(108, 64)[0];
        assert!(
            (outside - thin_lens(focus, 6.0)).abs() < 0.02 * thin_lens(focus, 6.0).abs().max(1.0)
        );
    }
}

#[test]
fn diopters_stack_across_many_regions() {
    let mut rig = Rig::new();
    // Three vertical thirds at 1.5, 3 and 9 m; where both diopters cover, the
    // later one (1.5 m) sets the focus.
    let scene = Scene::new(Scene::checker, |x, _| match x * 3 / SIZE {
        0 => 1.5,
        1 => 3.0,
        _ => 9.0,
    });
    let thirds = [
        // Covers x < 1/6 of the image height right of centre: left and middle.
        LensElementRow::diopter(
            3.0,
            LensRegion::HalfPlane {
                angle: std::f32::consts::PI,
                offset: -1.0 / 6.0,
            },
        )
        .with_feather(0.004),
        // Covers x < -1/6: the left third, which also has the first.
        LensElementRow::diopter(
            1.5,
            LensRegion::HalfPlane {
                angle: std::f32::consts::PI,
                offset: 1.0 / 6.0,
            },
        )
        .with_feather(0.004),
    ];
    let mut lens = lens();
    lens.focal_length_mm = 85.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 9.0;
    lens.element_count = 2;
    let frame = rig.record(&scene, Some(&[lens]), Some(&thirds));
    for x in [6, 20, 64, 90, 115, 124] {
        assert!(
            frame.coc(x, 64)[0].abs() < 0.1,
            "column {x}: circle {}",
            frame.coc(x, 64)[0]
        );
    }
    // Drop the second diopter: the left third is no longer in focus.
    lens.element_count = 1;
    let frame = rig.record(&scene, Some(&[lens]), Some(&thirds));
    assert!(frame.coc(20, 64)[0].abs() > 2.0, "{}", frame.coc(20, 64)[0]);
    assert!(frame.coc(64, 64)[0].abs() < 0.1);
}

#[test]
fn nested_regions_of_any_shape_focus_independently() {
    let mut rig = Rig::new();
    // A disc at 1.5 m inside a ring at 3 m inside a floor at 9 m.
    let radius = |x: u32, y: u32| {
        let q = [
            (x as f32 + 0.5) / SIZE as f32 - 0.5,
            (y as f32 + 0.5) / SIZE as f32 - 0.5,
        ];
        (q[0] * q[0] + q[1] * q[1]).sqrt()
    };
    let scene = Scene::new(Scene::checker, |x, y| match radius(x, y) {
        r if r < 0.15 => 1.5,
        r if r < 0.33 => 3.0,
        _ => 9.0,
    });
    let elements = [
        LensElementRow::diopter(
            3.0,
            LensRegion::Ellipse {
                center: [0.0; 2],
                radii: [0.33, 0.33],
                rotation: 0.0,
            },
        )
        .with_feather(0.004),
        LensElementRow::diopter(
            1.5,
            LensRegion::Polygon(&[[-0.15, -0.15], [0.15, -0.15], [0.15, 0.15], [-0.15, 0.15]]),
        )
        .with_feather(0.004),
    ];
    let mut lens = lens();
    lens.focal_length_mm = 85.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 9.0;
    lens.element_count = 2;
    let frame = rig.record(&scene, Some(&[lens]), Some(&elements));
    // The square covers the disc (and a little more) at the 1.5 m stop.
    for (x, y) in [(64, 64), (60, 70), (4, 4), (124, 64), (64, 100), (64, 30)] {
        let [coc, _, seam, _] = frame.coc(x, y);
        let r = radius(x, y);
        if (0.13..0.17).contains(&r) || (0.31..0.35).contains(&r) {
            continue;
        }
        assert!(
            coc.abs() < 0.1 && seam == 0.0,
            "({x}, {y}) r {r}: circle {coc}, seam {seam}"
        );
    }
}

#[test]
fn every_region_kind_covers_what_it_describes() {
    let mut rig = Rig::new();
    // In-focus plane; a sweet spot adds 10 px of defocus outside its region.
    let scene = Scene::new(Scene::checker, |_, _| 3.0);
    let mut lens = lens();
    lens.focus_distance_m = 3.0;
    lens.element_count = 1;
    let triangle = [[-0.2, -0.2], [0.3, -0.1], [0.0, 0.3]];
    let reversed = [[0.0, 0.3], [0.3, -0.1], [-0.2, -0.2]];
    // (region, a pixel inside, a pixel outside)
    type Case<'a> = (LensRegion<'a>, (u32, u32), (u32, u32));
    let cases: [Case; 7] = [
        (LensRegion::FullFrame, (64, 64), (5, 5)),
        (
            LensRegion::HalfPlane {
                angle: 0.0,
                offset: 0.0,
            },
            (100, 64),
            (20, 64),
        ),
        (
            LensRegion::Ellipse {
                center: [0.1, 0.0],
                radii: [0.3, 0.1],
                rotation: 0.0,
            },
            (70, 64),
            (70, 20),
        ),
        (LensRegion::Polygon(&triangle), (64, 70), (20, 20)),
        (LensRegion::Polygon(&reversed), (64, 70), (20, 20)),
        (
            LensRegion::RadialBand {
                r0: 0.2,
                r1: 0.35,
                center: [0.0; 2],
            },
            (64 + 33, 64),
            (64, 64),
        ),
        (LensRegion::Mask, (64, 64), (64, 64)),
    ];
    for (i, (region, inside, outside)) in cases.into_iter().enumerate() {
        let element = LensElementRow::sweet_spot(10.0, region).with_feather(0.0);
        let frame = rig.record(&scene, Some(&[lens]), Some(&[element]));
        let covered = i != 6;
        let at = |p: (u32, u32)| frame.coc(p.0, p.1)[0];
        if covered {
            assert!(at(inside).abs() < 0.1, "case {i}: inside {}", at(inside));
        }
        if i != 0 {
            assert!(
                (at(outside) - 10.0).abs() < 0.1,
                "case {i}: outside {}",
                at(outside)
            );
        }
    }
}

#[test]
fn tilt_and_field_curvature_shape_the_focus_field() {
    let mut rig = Rig::new();
    let scene = Scene::new(Scene::checker, |_, _| 1.0);
    let mut lens = lens();
    lens.focal_length_mm = 100.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 1.0;
    let flat = rig.record(&scene, Some(&[lens]), None);
    assert!(flat.coc(0, 0)[0].abs() < 0.05 && flat.coc(64, 64)[0].abs() < 0.05);

    // A tilted focal plane meets the wall nearer on one side: opposite
    // circles at the two edges, none on the axis.
    lens.tilt_angle = 0.6;
    lens.tilt_axis = 0.0;
    let tilted = rig.record(&scene, Some(&[lens]), None);
    assert!(
        tilted.coc(SIZE - 1, 64)[0] > 3.0,
        "{}",
        tilted.coc(SIZE - 1, 64)[0]
    );
    assert!(tilted.coc(0, 64)[0] < -3.0, "{}", tilted.coc(0, 64)[0]);
    assert!(tilted.coc(63, 64)[0].abs() < 0.8);
    // About a vertical axis the field varies vertically instead.
    lens.tilt_axis = std::f32::consts::FRAC_PI_2;
    let vertical = rig.record(&scene, Some(&[lens]), None);
    assert!(vertical.coc(64, 0)[0] > 3.0 || vertical.coc(64, 0)[0] < -3.0);
    assert!(vertical.coc(0, 64)[0].abs() < 1.0);

    // Field curvature shifts the focus with radius: sharp on the axis only.
    lens.tilt_angle = 0.0;
    lens.field_curvature = 1.0;
    let curved = rig.record(&scene, Some(&[lens]), None);
    assert!(curved.coc(64, 64)[0].abs() < 0.2);
    assert!(curved.coc(0, 0)[0].abs() > 5.0, "{}", curved.coc(0, 0)[0]);
}

#[test]
fn autofocus_tracks_the_depth_at_its_point_and_eases() {
    let mut rig = Rig::new();
    let mut lens = lens();
    lens.focus_mode = FOCUS_AUTO_POINT;
    lens.af_u = 0.25;
    lens.af_v = 0.5;
    // A wall at 5 m on the left, 2 m on the right: the point reads the left.
    let scene = Scene::new(Scene::checker, |x, _| if x < SIZE / 2 { 5.0 } else { 2.0 });
    rig.record(&scene, Some(&[lens]), None);
    let focus = rig.resolved().focus_distance_m;
    assert!((focus - 5.0).abs() < 0.05, "autofocus read {focus}");

    // Moving the point reads the other wall; with an ease rate it takes
    // time (in dioptres), and holds with no time passing.
    lens.af_u = 0.75;
    lens.af_speed = 10.0;
    rig.set_delta_time(0.0);
    rig.record(&scene, Some(&[lens]), None);
    assert!(
        (rig.resolved().focus_distance_m - 5.0).abs() < 0.05,
        "moved without time"
    );
    rig.set_delta_time(1.0 / 60.0);
    rig.record(&scene, Some(&[lens]), None);
    let eased = rig.resolved().focus_distance_m;
    // alpha = 1 - exp(-10 / 60), from 0.2 dpt toward 0.5 dpt.
    let alpha = 1.0 - (-10.0f32 / 60.0).exp();
    let expected = 1.0 / (0.2 + 0.3 * alpha);
    assert!(
        (eased - expected).abs() < 0.02,
        "eased to {eased}, expected {expected}"
    );
    // Without an ease rate the next read is immediate.
    lens.af_speed = 0.0;
    rig.record(&scene, Some(&[lens]), None);
    assert!((rig.resolved().focus_distance_m - 2.0).abs() < 0.02);

    // A point on the sky holds the last focus instead of racing to infinity.
    let sky = Scene::new(Scene::checker, |_, _| f32::INFINITY);
    rig.record(&sky, Some(&[lens]), None);
    assert!((rig.resolved().focus_distance_m - 2.0).abs() < 0.02);
}

#[test]
fn seam_darkening_dims_the_diopter_edge() {
    let mut rig = Rig::new();
    let scene = Scene::new(|_, _| [1.0; 3], |x, _| if x < SIZE / 2 { 2.0 } else { 6.0 });
    let diopter = LensElementRow::diopter(
        2.0,
        LensRegion::HalfPlane {
            angle: std::f32::consts::PI,
            offset: 0.0,
        },
    )
    .with_feather(0.1)
    .with_seam_darkening(0.5);
    let mut lens = lens();
    lens.focus_distance_m = 6.0;
    lens.element_count = 1;
    let frame = rig.record(&scene, Some(&[lens]), Some(&[diopter]));
    let [.., centre] = frame.coc(63, 64);
    assert!(
        (centre - 0.5).abs() < 0.08,
        "darkest at the seam centre: {centre}"
    );
    assert_eq!(frame.coc(10, 64)[3], 1.0);
    assert!(frame.pixel(63, 64)[0] < 0.6 && frame.pixel(10, 64)[0] > 0.99);
}

// ── Pupil function ───────────────────────────────────────────────────────────
//
// The gather averages the taps it lands on, so a lone point source is a
// handful of lucky taps. These tests measure the pupil through large,
// averaged quantities instead: the 10-90% width of a blurred edge (the
// pupil's projected width), the image of a small bright blob binned by
// radius, and the energy of the whole frame.

/// A lens whose circle of confusion is 28 px across, for a plane at infinity.
fn bokeh_lens() -> LensRow {
    let mut lens = lens();
    lens.focal_length_mm = 100.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 1.0;
    lens.max_coc_px = 64.0;
    lens.quality = 1;
    // A circle unless a test says otherwise.
    lens.aperture_blades = 0;
    lens.element_count = 1;
    lens
}

fn infinity() -> impl Fn(u32, u32) -> f32 {
    |_, _| f32::INFINITY
}

/// Bright (1.0) before `at` along x (a vertical edge) or y, dark after.
fn edge_scene(vertical: bool, at: u32) -> Scene {
    Scene::new(
        |x, y| {
            let t = if vertical { x } else { y };
            [if t < at { 1.0 } else { 0.0 }; 3]
        },
        infinity(),
    )
}

/// The 10-90% transition width in pixels of a blurred bright-to-dark edge,
/// from the profile averaged over `along` the edge.
fn edge_width(frame: &Frame, vertical: bool, along: std::ops::Range<u32>) -> f32 {
    edge_width_between(frame, vertical, along, 0.1, 0.9)
}

/// The width between two levels of the same profile.
fn edge_width_between(
    frame: &Frame,
    vertical: bool,
    along: std::ops::Range<u32>,
    low: f32,
    high: f32,
) -> f32 {
    let profile: Vec<f32> = (0..SIZE)
        .map(|t| {
            let sum: f32 = along
                .clone()
                .map(|s| {
                    if vertical {
                        frame.pixel(t, s)[1]
                    } else {
                        frame.pixel(s, t)[1]
                    }
                })
                .sum();
            sum / along.len() as f32
        })
        .collect();
    let cross = |level: f32| -> f32 {
        let i = profile
            .iter()
            .position(|&v| v < level)
            .expect("edge never falls below the level");
        let (a, b) = (profile[i - 1], profile[i]);
        (i - 1) as f32 + (a - level) / (a - b)
    };
    cross(low) - cross(high)
}

/// Widths of the middle of the transition (30-70%) of a vertical (x) and a
/// horizontal (y) edge, averaged over the whole frame. The middle slope is
/// the pupil's chord through its centre, which a polygon's tails hide.
fn core_widths(rig: &mut Rig, lens: LensRow, elements: &[LensElementRow]) -> (f32, f32) {
    let mut width = |vertical: bool| {
        let frame = rig.record(
            &edge_scene(vertical, 64),
            Some(&[lens]),
            (!elements.is_empty()).then_some(elements),
        );
        edge_width_between(&frame, vertical, 8..120, 0.3, 0.7)
    };
    (width(true), width(false))
}

/// Widths of a vertical (x) and a horizontal (y) edge through the centre.
fn edge_widths(rig: &mut Rig, lens: LensRow, elements: &[LensElementRow]) -> (f32, f32) {
    let mut width = |vertical: bool| {
        let frame = rig.record(
            &edge_scene(vertical, 64),
            Some(&[lens]),
            (!elements.is_empty()).then_some(elements),
        );
        edge_width(&frame, vertical, 52..76)
    };
    (width(true), width(false))
}

#[test]
fn blades_make_the_bokeh_polygonal() {
    let mut rig = Rig::new();
    let mut lens = bokeh_lens();
    lens.element_count = 0;
    // A hexagon with a vertex on +x has a chord of 1.73 R through its centre
    // along x and 2 R along y (a wider chord is a steeper edge, a narrower
    // transition); turned a twelfth of a turn the two swap. A circle is the same both ways.
    lens.aperture_blades = 0;
    let (cx, cy) = core_widths(&mut rig, lens, &[]);
    assert!((cx / cy - 1.0).abs() < 0.05, "circle {cx} x {cy}");
    lens.aperture_blades = 6;
    let (hx, hy) = core_widths(&mut rig, lens, &[]);
    lens.aperture_rotation = std::f32::consts::PI / 6.0;
    let (rx, ry) = core_widths(&mut rig, lens, &[]);
    println!("circle {cx} x {cy}; hexagon {hx} x {hy}; turned {rx} x {ry}");
    assert!(hx / hy > 1.04, "vertex-on-x hexagon {hx} x {hy}");
    assert!(ry / rx > 1.02, "turned hexagon {rx} x {ry}");
    // The circumscribed circle bounds the polygon.
    assert!(
        hx < cx * 1.05 && hy < cx * 0.95,
        "hexagon {hx} x {hy} within the circle {cx}"
    );
    // Fewer blades are less round: a triangle with a vertex on +x has chords of 1.15 R
    // along x and 1.5 R along y.
    lens.aperture_rotation = 0.0;
    lens.aperture_blades = 3;
    let (tx, ty) = core_widths(&mut rig, lens, &[]);
    println!("triangle {tx} x {ty}");
    assert!(tx / ty > 1.1, "triangle {tx} x {ty}");

    // Curved blades relax the hexagon back to a circle.
    lens.aperture_blades = 0;
    lens.element_count = 1;
    let round = LensElementRow::pupil(LensRegion::FullFrame).with_iris(6, 1.0, 0.0);
    let (ex, ey) = core_widths(&mut rig, lens, &[round]);
    assert!((ex / ey - 1.0).abs() < 0.05, "curvature 1: {ex} x {ey}");
    // Wide open is round (the f-number is far below the straight limit).
    let blades = LensElementRow::pupil(LensRegion::FullFrame)
        .with_iris(6, 0.0, 0.0)
        .with_f_number_rounding(8.0);
    let (wx, wy) = core_widths(&mut rig, lens, &[blades]);
    assert!((wx / wy - 1.0).abs() < 0.05, "wide open {wx} x {wy}");
    // With the straight limit at f/1 the same lens is stopped down: polygonal.
    let stopped = blades.with_f_number_rounding(1.0);
    let (sx, sy) = core_widths(&mut rig, lens, &[stopped]);
    println!("stopped down {sx} x {sy}");
    assert!(sx / sy > 1.05, "stopped down {sx} x {sy}");
}

#[test]
fn anamorphic_squeeze_ovals_the_bokeh() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    for squeeze in [1.33, 1.8, 2.0] {
        let element = LensElementRow::pupil(LensRegion::FullFrame).with_anamorphic(squeeze);
        let (width, height) = edge_widths(&mut rig, lens, &[element]);
        let ratio = height / width;
        println!("squeeze {squeeze}: {width} x {height}");
        assert!(
            (ratio / squeeze - 1.0).abs() < 0.1,
            "squeeze {squeeze}: {width} x {height} ({ratio})"
        );
    }
}

/// Radial and tangential widths at the middle of the right edge region: x is
/// radial on the row through the centre, y is tangential.
fn radial_and_tangential(rig: &mut Rig, lens: LensRow, elements: &[LensElementRow]) -> (f32, f32) {
    let radial = {
        let frame = rig.record(
            &edge_scene(true, 108),
            Some(&[lens]),
            (!elements.is_empty()).then_some(elements),
        );
        edge_width(&frame, true, 56..72)
    };
    let tangential = {
        let frame = rig.record(
            &edge_scene(false, 64),
            Some(&[lens]),
            (!elements.is_empty()).then_some(elements),
        );
        edge_width(&frame, false, 100..116)
    };
    (radial, tangential)
}

#[test]
fn cats_eye_clips_the_bokeh_radially_off_axis_only() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    let element = LensElementRow::pupil(LensRegion::FullFrame).with_cats_eye(1.5, 1.0);
    // On the axis the second circle sits on the first: round.
    let (x, y) = edge_widths(&mut rig, lens, &[element]);
    println!("on axis {x} x {y}");
    assert!((x / y - 1.0).abs() < 0.07, "on axis {x} x {y}");
    // Off to the side x is radial: its width shrinks, the tangential one
    // does not.
    let (radial, tangential) = radial_and_tangential(&mut rig, lens, &[element]);
    println!("off axis: radial {radial}, tangential {tangential}");
    assert!(
        radial < 0.85 * tangential,
        "off axis: radial {radial}, tangential {tangential}"
    );
    // Without the element the same place is round.
    let (radial, tangential) = radial_and_tangential(&mut rig, lens, &[]);
    assert!(
        (radial / tangential - 1.0).abs() < 0.08,
        "control: {radial} x {tangential}"
    );
}

#[test]
fn swirl_stretches_the_bokeh_tangentially_toward_the_edges() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    let element = LensElementRow::pupil(LensRegion::FullFrame).with_swirl(2.0);
    let (radial, tangential) = radial_and_tangential(&mut rig, lens, &[element]);
    println!("swirl: radial {radial}, tangential {tangential}");
    assert!(
        radial < 0.8 * tangential,
        "swirl: radial {radial}, tangential {tangential}"
    );
    // And the centre is untouched.
    let (cx, cy) = edge_widths(&mut rig, lens, &[element]);
    assert!((cx / cy - 1.0).abs() < 0.07, "centre {cx} x {cy}");
}

/// A bright 8 x 8 blob at the centre of a dark frame `depth` metres away;
/// the dark rest is at infinity.
fn blob_scene(depth: f32) -> Scene {
    let blob = |x: u32, y: u32| (60..68).contains(&x) && (60..68).contains(&y);
    Scene::new(
        |x, y| if blob(x, y) { [100.0; 3] } else { [0.0; 3] },
        move |x, y| if blob(x, y) { depth } else { f32::INFINITY },
    )
}

/// Mean of a channel over pixels whose distance from the frame centre is in
/// `[r0, r1)`.
fn ring_mean(frame: &Frame, channel: usize, r0: f32, r1: f32) -> f32 {
    let (mut sum, mut count) = (0.0, 0.0);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let r = ((x as f32 + 0.5 - 64.0).powi(2) + (y as f32 + 0.5 - 64.0).powi(2)).sqrt();
            if r >= r0 && r < r1 {
                sum += frame.pixel(x, y)[channel];
                count += 1.0;
            }
        }
    }
    sum / count
}

#[test]
fn spherical_aberration_trades_light_between_rim_and_centre() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    // A blob 8 px square, either behind the focus (infinity) or in front
    // of it (0.5 m: the same 28 px circle, negative).
    let far = blob_scene(f32::INFINITY);
    let near = blob_scene(0.5);
    let rim_and_centre = |rig: &mut Rig, scene: &Scene, amount: f32, asymmetry: f32| {
        let element = LensElementRow::pupil(LensRegion::FullFrame)
            .with_spherical_aberration(amount, asymmetry);
        let frame = rig.record(scene, Some(&[lens]), Some(&[element]));
        assert!(
            frame.coc(64, 64)[0].abs() > 25.0,
            "test geometry: circle {}",
            frame.coc(64, 64)[0]
        );
        let centre = ring_mean(&frame, 1, 0.0, 3.0);
        let rim = ring_mean(&frame, 1, 10.5, 13.0);
        println!("sa {amount} asym {asymmetry}: rim {rim}, centre {centre}");
        (rim, centre)
    };
    // The control: no aberration, a flat disc (rim and centre alike).
    let (rim, centre) = rim_and_centre(&mut rig, &far, 0.0, 0.0);
    assert!(
        rim > 0.1 && centre > 0.1 && (rim / centre - 1.0).abs() < 0.6,
        "flat: rim {rim}, centre {centre}"
    );
    // Soap bubble: a bright rim.
    let (rim, centre) = rim_and_centre(&mut rig, &far, 1.0, 0.0);
    assert!(
        rim > 2.5 * centre,
        "soap bubble: rim {rim}, centre {centre}"
    );
    // Soft: weighted to the centre.
    let (rim, centre) = rim_and_centre(&mut rig, &far, -1.0, 0.0);
    assert!(centre > 2.5 * rim, "soft: rim {rim}, centre {centre}");
    // Fully asymmetric: the profile flips between front and back focus.
    let (rim_far, centre_far) = rim_and_centre(&mut rig, &far, 1.0, 1.0);
    let (rim_near, centre_near) = rim_and_centre(&mut rig, &near, 1.0, 1.0);
    assert!(
        rim_far > 2.5 * centre_far,
        "behind the focus: rim {rim_far}, centre {centre_far}"
    );
    assert!(
        centre_near > 2.5 * rim_near,
        "in front of the focus: rim {rim_near}, centre {centre_near}"
    );
    // Without asymmetry the front matches the back.
    let (rim_near, centre_near) = rim_and_centre(&mut rig, &near, 1.0, 0.0);
    assert!(
        rim_near > 2.5 * centre_near,
        "symmetric, in front: rim {rim_near}, centre {centre_near}"
    );
}

#[test]
fn apodization_and_onion_rings_shape_the_profile() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    let far = blob_scene(f32::INFINITY);
    let apodized = LensElementRow::pupil(LensRegion::FullFrame).with_apodization(0.35);
    let frame = rig.record(&far, Some(&[lens]), Some(&[apodized]));
    let flat = rig.record(
        &far,
        Some(&[lens]),
        Some(&[LensElementRow::pupil(LensRegion::FullFrame)]),
    );
    let (rim, centre) = (
        ring_mean(&frame, 1, 10.5, 13.0),
        ring_mean(&frame, 1, 0.0, 3.0),
    );
    println!("apodized: rim {rim}, centre {centre}");
    assert!(centre > 3.0 * rim, "apodized: rim {rim}, centre {centre}");
    // The edge fades out smoothly instead of ending.
    let soft_edge = ring_mean(&frame, 1, 12.0, 14.0) / ring_mean(&frame, 1, 0.0, 3.0);
    let hard_edge = ring_mean(&flat, 1, 12.0, 14.0) / ring_mean(&flat, 1, 0.0, 3.0);
    assert!(
        soft_edge < 0.5 * hard_edge,
        "edge {soft_edge} vs {hard_edge}"
    );

    // Concentric ripples: the brightness alternates with radius.
    let onion = LensElementRow::pupil(LensRegion::FullFrame).with_onion_rings(0.9, 1.0);
    let frame = rig.record(&far, Some(&[lens]), Some(&[onion]));
    // cos(2 pi rho) peaks at the centre and the rim and troughs at half the
    // radius (7 px); the blob smears it by its own 4 px.
    let peak = ring_mean(&frame, 1, 0.0, 3.0);
    let trough = ring_mean(&frame, 1, 6.0, 8.0);
    println!("onion: peak {peak}, trough {trough}");
    assert!(
        peak > 1.5 * trough,
        "onion rings: peak {peak}, trough {trough}"
    );
}

#[test]
fn central_obstruction_leaves_a_dark_hole() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    let far = blob_scene(f32::INFINITY);
    let open = LensElementRow::pupil(LensRegion::FullFrame);
    let control = rig.record(&far, Some(&[lens]), Some(&[open]));
    let donut = LensElementRow::pupil(LensRegion::FullFrame).with_central_obstruction(0.6);
    let frame = rig.record(&far, Some(&[lens]), Some(&[donut]));
    let (centre, ring) = (
        ring_mean(&frame, 1, 0.0, 3.0),
        ring_mean(&frame, 1, 9.5, 13.0),
    );
    let flat_centre = ring_mean(&control, 1, 0.0, 3.0);
    println!("donut: centre {centre}, ring {ring}, open centre {flat_centre}");
    assert!(flat_centre > 0.3, "control centre {flat_centre}");
    assert!(centre < 0.1 * ring, "donut: centre {centre}, ring {ring}");
    assert!(
        centre < 0.2 * flat_centre,
        "donut centre {centre} vs flat {flat_centre}"
    );
}

#[test]
fn longitudinal_chromatic_aberration_fringes_by_depth() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    // Red and blue focus farther than green: foregrounds fringe purple,
    // backgrounds green.
    let ca = LensElementRow::longitudinal_ca(-0.6, 0.0, -0.6, LensRegion::FullFrame);
    let fringe = |rig: &mut Rig, depth: f32, band: (f32, f32), element: LensElementRow| {
        let frame = rig.record(&blob_scene(depth), Some(&[lens]), Some(&[element]));
        // The outer edge of the larger channels' discs, beyond the smaller
        // ones (the green circle is 14 px in radius, red and blue 8.5 more
        // in front and 8.5 less behind; the blob adds 4).
        [0, 1, 2].map(|c| ring_mean(&frame, c, band.0, band.1))
    };
    let near = fringe(&mut rig, 0.5, (20.0, 24.0), ca);
    println!("foreground fringe {near:?}");
    assert!(
        near[0] > 3.0 * near[1] && near[2] > 3.0 * near[1] && near[0] > 0.02,
        "foreground fringe r {} g {} b {}",
        near[0],
        near[1],
        near[2]
    );
    let far = fringe(&mut rig, f32::INFINITY, (16.0, 18.0), ca);
    println!("background fringe {far:?}");
    assert!(
        far[1] > 3.0 * far[0] && far[1] > 3.0 * far[2] && far[1] > 0.02,
        "background fringe r {} g {} b {}",
        far[0],
        far[1],
        far[2]
    );
    // No shift, no fringe.
    let none = LensElementRow::longitudinal_ca(0.0, 0.0, 0.0, LensRegion::FullFrame);
    let flat = fringe(&mut rig, 0.5, (20.0, 24.0), none);
    assert!(flat.iter().all(|&c| c < 0.02), "neutral: {flat:?}");
    // Region limited: outside it the channels agree.
    let left = LensElementRow::longitudinal_ca(
        -0.6,
        0.0,
        -0.6,
        LensRegion::HalfPlane {
            angle: std::f32::consts::PI,
            offset: 0.3,
        },
    );
    let outside = fringe(&mut rig, 0.5, (20.0, 24.0), left);
    assert!(
        outside.iter().all(|&c| c < 0.02),
        "outside the region: {outside:?}"
    );
}

#[test]
fn a_texture_replaces_the_iris() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    // Layer 0: open. Layer 1: a vertical bar a fifth of the width.
    const N: usize = 32;
    let open = vec![255u8; N * N];
    let bar: Vec<u8> = (0..N * N)
        .map(|i| if (13..19).contains(&(i % N)) { 255 } else { 0 })
        .collect();
    rig.shapes = Some(aperture_shape_array(
        &rig.device,
        &rig.queue,
        N as u32,
        &[&open, &bar],
    ));
    let measure = |rig: &mut Rig, layer: u32, amount: f32| {
        let element =
            LensElementRow::pupil(LensRegion::FullFrame).with_aperture_texture(layer, amount);
        edge_widths(rig, lens, &[element])
    };
    let (x, y) = measure(&mut rig, 0, 1.0);
    assert!((x / y - 1.0).abs() < 0.07, "open layer {x} x {y}");
    let (x, y) = measure(&mut rig, 1, 1.0);
    println!("bar layer {x} x {y}");
    assert!(y / x > 2.0, "bar layer {x} x {y}");
    // Half strength sits between the polygon and the texture.
    let (hx, hy) = measure(&mut rig, 1, 0.5);
    assert!(
        hy / hx > 1.05 && hy / hx < y / x,
        "half strength {hx} x {hy}"
    );
    // No texture bound: the neutral layer is open.
    rig.shapes = None;
    let (x, y) = measure(&mut rig, 1, 1.0);
    assert!((x / y - 1.0).abs() < 0.07, "neutral layer {x} x {y}");
}

#[test]
fn pupil_elements_are_region_limited() {
    let mut rig = Rig::new();
    let lens = bokeh_lens();
    // Anamorphic only in the right of the frame.
    let right = LensElementRow::pupil(LensRegion::HalfPlane {
        angle: 0.0,
        offset: 0.1,
    })
    .with_anamorphic(2.0);
    let covered = rig.record(&edge_scene(true, 100), Some(&[lens]), Some(&[right]));
    let uncovered = rig.record(&edge_scene(true, 28), Some(&[lens]), Some(&[right]));
    let covered_x = edge_width(&covered, true, 56..72);
    let uncovered_x = edge_width(&uncovered, true, 56..72);
    println!("covered {covered_x}, uncovered {uncovered_x}");
    assert!(
        (covered_x / uncovered_x - 0.5).abs() < 0.07,
        "covered {covered_x}, uncovered {uncovered_x}"
    );
}

#[test]
fn the_pupil_variants_preserve_energy() {
    let mut rig = Rig::new();
    // The phase-1 scene: patches on a dim floor, one plane far behind the
    // focus (a uniform circle of about six pixels).
    let scene = Scene::new(
        |x, y| match (x, y) {
            (48..80, 48..80) => [8.0, 6.0, 4.0],
            (20..28, 90..98) => [3.0, 3.0, 12.0],
            _ => [0.05, 0.06, 0.07],
        },
        infinity(),
    );
    let mut lens = lens();
    lens.focal_length_mm = 50.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 1.0;
    lens.element_count = 1;
    let pupil = || LensElementRow::pupil(LensRegion::FullFrame);
    let everything = pupil()
        .with_iris(7, 0.3, 0.2)
        .with_anamorphic(1.5)
        .with_cats_eye(0.8, 1.0)
        .with_swirl(1.0)
        .with_spherical_aberration(0.6, 0.5)
        .with_apodization(0.7)
        .with_central_obstruction(0.3);
    let chromatic = LensElementRow::longitudinal_ca(-0.4, 0.0, -0.4, LensRegion::FullFrame);
    let variants: Vec<(&str, u32, LensElementRow)> = vec![
        ("circle", 0, pupil()),
        ("hexagon", 6, pupil()),
        ("triangle", 3, pupil().with_iris(3, 0.0, 0.3)),
        ("curved blades", 8, pupil().with_iris(8, 0.5, 0.0)),
        ("anamorphic", 0, pupil().with_anamorphic(2.0)),
        ("cat's eye", 0, pupil().with_cats_eye(1.2, 1.0)),
        ("swirl", 0, pupil().with_swirl(2.0)),
        (
            "soap bubble",
            0,
            pupil().with_spherical_aberration(1.0, 0.0),
        ),
        ("soft", 0, pupil().with_spherical_aberration(-1.0, 0.0)),
        ("asymmetric", 0, pupil().with_spherical_aberration(1.0, 1.0)),
        ("onion", 0, pupil().with_onion_rings(0.8, 4.0)),
        ("apodized", 0, pupil().with_apodization(0.4)),
        ("donut", 0, pupil().with_central_obstruction(0.6)),
        ("everything", 7, everything),
        ("chromatic", 0, chromatic),
    ];
    for quality in [0, 1] {
        lens.quality = quality;
        for (name, blades, element) in &variants {
            lens.aperture_blades = *blades;
            let frame = rig.record(&scene, Some(&[lens]), Some(&[*element]));
            for channel in 0..3 {
                let sum = |pixel: &dyn Fn(u32, u32) -> f32| -> f32 {
                    (0..SIZE)
                        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
                        .map(|(x, y)| pixel(x, y))
                        .sum()
                };
                let before = sum(&|x, y| scene.color[(y * SIZE + x) as usize][channel]);
                let after = sum(&|x, y| frame.pixel(x, y)[channel]);
                println!("{name} q{quality} c{channel}: {:.4}", after / before);
                assert!(
                    (after / before - 1.0).abs() < 0.03,
                    "{name}, quality {quality}, channel {channel}: energy {before} -> {after}"
                );
            }
        }
    }
}

#[test]
fn pupil_element_builders_fill_the_documented_slots() {
    let e = LensElementRow::pupil(LensRegion::FullFrame)
        .with_iris(5, 0.25, 0.5)
        .with_f_number_rounding(8.0)
        .with_aperture_texture(3, 0.75)
        .with_anamorphic(1.8)
        .with_cats_eye(0.6, 1.2)
        .with_swirl(0.4)
        .with_spherical_aberration(-0.5, 1.0)
        .with_onion_rings(0.3, 5.0)
        .with_apodization(0.5)
        .with_central_obstruction(0.4);
    assert_eq!(e.kind, element_kind::PUPIL);
    assert_eq!(
        e.params,
        [5.0, 0.25, 0.5, 8.0, 3.0, 0.75, 1.8, 0.6, 1.2, 0.4, -0.5, 0.3, 5.0, 0.5, 0.4, 1.0]
    );
    let inherits = LensElementRow::pupil(LensRegion::FullFrame);
    assert!(inherits.params[pupil::BLADES] < 0.0 && inherits.params[pupil::SQUEEZE] == 1.0);
    let ca = LensElementRow::longitudinal_ca(-0.1, 0.0, -0.2, LensRegion::FullFrame);
    assert_eq!(
        (ca.kind, ca.params[..3].to_vec()),
        (element_kind::LONGITUDINAL_CA, vec![-0.1, 0.0, -0.2])
    );
}

mod aberration;
mod bundled;
mod optics;
mod overscan;
mod prescription;
