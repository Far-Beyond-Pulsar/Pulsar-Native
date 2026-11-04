//! Hi-Z occlusion culling under a camera that never moves.
//!
//! The max pyramid is reused while the view holds still, so it must only ever
//! be reused once it was built from depth the current scene actually drew --
//! not from the empty depth of the frames before the first draws arrive, and
//! not from depth drawn before the last scene change.

#[path = "../v3_demo_common.rs"]
#[allow(dead_code)]
mod v3_demo_common;

use glam::{Mat4, Vec3};
use helio::{Camera, RendererBuilder, RendererConfig};
use pulsar_scenedb::SceneDb;
use std::sync::Arc;
use v3_demo_common::*;

const SIZE: u32 = 64;

struct Harness {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    renderer: helio::Renderer,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    staging: wgpu::Buffer,
    camera: Camera,
}

impl Harness {
    /// `None` when no GPU adapter is available.
    fn new(populate: impl FnOnce(&mut SceneDb)) -> Option<(Self, SceneDb)> {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: helio::required_wgpu_features(
                adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY,
            ),
            required_limits: helio::required_wgpu_limits(adapter.limits()),
            experimental_features: helio::required_experimental_features(adapter.features()),
            ..Default::default()
        }))
        .expect("device");
        let (device, queue) = (Arc::new(device), Arc::new(queue));

        // Everything is in the scene before the renderer exists.
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        populate(&mut scene_db);

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = RendererBuilder::new(
            RendererConfig::new(SIZE, SIZE, format).with_render_scale(1.0),
            scene_db_handle(&scene_db),
        )
        .with_external_device()
        .with_pass_build_context(Box::new(
            helio_mesh_host::presets::deferred,
        ))
        .build(device.clone(), queue.clone(), SIZE, SIZE, format);
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
        renderer.set_feature_live(helio::AmbientLight {
        color: [0.0; 3],
        intensity: 0.0,
    });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (SIZE * SIZE * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Fixed for the whole test.
        let camera = Camera::perspective_look_at(
            Vec3::new(0.0, 0.0, 8.0),
            Vec3::ZERO,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            1.0,
            0.1,
            100.0,
        );
        Some((
            Self {
                device,
                queue,
                renderer,
                target,
                view,
                staging,
                camera,
            },
            scene_db,
        ))
    }

    /// Renders `frames` frames and returns the centre pixel's RGB.
    fn render(&mut self, scene_db: &SceneDb, frames: usize) -> [u8; 3] {
        for _ in 0..frames {
            flush_scene_db(scene_db, &self.queue);
            self.renderer.render(&self.camera, &self.view).unwrap();
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 4),
                    rows_per_image: None,
                },
            },
            self.target.size(),
        );
        self.queue.submit([encoder.finish()]);
        self.staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let pixels = self.staging.slice(..).get_mapped_range().unwrap().to_vec();
        self.staging.unmap();
        let i = (((SIZE / 2) * SIZE + SIZE / 2) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    }
}

fn spawn_emissive_box(
    scene_db: &mut SceneDb,
    color: [f32; 3],
    half_extents: [f32; 3],
    translation: Vec3,
) -> pulsar_scenedb::Entity {
    let material = spawn_material(
        &mut scene_db.world,
        make_material([1.0; 4], 1.0, 0.0, color, 20.0),
    );
    let mesh = spawn_mesh(&mut scene_db.world, box_mesh([0.0; 3], half_extents));
    let radius = Vec3::from_array(half_extents).length();
    spawn_object(
        &mut scene_db.world,
        mesh,
        material,
        Mat4::from_translation(translation),
        radius,
    )
    .unwrap()
}

/// Objects present before the first frame, camera never moves: the object
/// must be drawn once its draws arrive, and stay drawn while the pyramid is
/// reused frame after frame.
#[test]
fn objects_present_before_the_first_frame_are_drawn_under_a_static_camera() {
    let Some((mut h, scene_db)) = Harness::new(|scene_db| {
        spawn_emissive_box(scene_db, [1.0; 3], [1.0; 3], Vec3::ZERO);
    }) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };

    let [r, g, b] = h.render(&scene_db, 12);
    assert!(
        r > 100 && g > 100 && b > 100,
        "object not drawn after 12 frames: {r} {g} {b}"
    );
    // Long after the pyramid settled into reuse.
    let [r, g, b] = h.render(&scene_db, 48);
    assert!(
        r > 100 && g > 100 && b > 100,
        "object vanished under a reused pyramid: {r} {g} {b}"
    );
}

/// A wall in front of an object, both present before the first frame, camera
/// never moves. Removing the wall must reveal the object: a pyramid still
/// holding the wall's depth would keep culling it.
#[test]
fn removing_an_occluder_under_a_static_camera_reveals_what_it_hid() {
    let mut wall = None;
    let Some((mut h, mut scene_db)) = Harness::new(|scene_db| {
        // Green object at the origin; a red wall filling the view in front.
        spawn_emissive_box(scene_db, [0.0, 1.0, 0.0], [0.5; 3], Vec3::ZERO);
        wall = Some(spawn_emissive_box(
            scene_db,
            [1.0, 0.0, 0.0],
            [4.0, 4.0, 0.1],
            Vec3::new(0.0, 0.0, 3.0),
        ));
    }) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };

    // The wall is drawn, and the object behind it is (correctly) hidden
    // long enough for the pyramid to settle into reuse. Colours are judged
    // by their dominant channel: the camera's tone curve shifts a saturated
    // emissive's hue (AgX lifts a bright red's green).
    let [r, g, _] = h.render(&scene_db, 24);
    assert!(
        r > 100 && g < r / 2,
        "wall not drawn in front of the object: r {r} g {g}"
    );

    despawn_object(&mut scene_db.world, &mut h.renderer, wall.unwrap()).unwrap();
    let [r, g, _] = h.render(&scene_db, 12);
    assert!(
        g > 100 && r < g / 2,
        "object behind the removed wall never appeared: r {r} g {g}"
    );
    let [r, g, _] = h.render(&scene_db, 24);
    assert!(
        g > 100 && r < g / 2,
        "revealed object vanished under a reused pyramid: r {r} g {g}"
    );
}
