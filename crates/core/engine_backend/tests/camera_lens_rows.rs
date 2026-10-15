//! A camera lens authored as a component instance reaches the buffers the
//! lens pass reads: the environment join gates its source row and publishes
//! it as `camera_lenses`, and its elements sit in a compact block of
//! `lens_elements`. Edits, presets, the instance switch and removal rewrite
//! them. Needs a GPU adapter (lavapipe works); skips without one.

use std::sync::Arc;

use engine_backend::scene::{attachments, SceneWorldExt, SpawnObject};
use helio_component::components::{
    camera_lens_view_overscan, CameraLensComponent, LensElementKind, LensElementProps, LensPreset,
    LensProjectionModel, LensRegionShape, LENS_ELEMENT_SLOTS,
};
use helio_default_graphs::environment_join::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
};
use helio_default_graphs::scene_join::ENTITY_GENERATIONS_KEY;
use helio_pass_lens::{element_kind, LensComponent, LensElementComponent};
use pulsar_scenedb::{Entity, SceneDb};

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        ..Default::default()
    }))
    .ok()?;
    Some((Arc::new(device), Arc::new(queue)))
}

struct Gpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    join: Box<dyn SceneDerivation>,
}

impl Gpu {
    /// Steps the scene and runs the environment join, as the renderer does.
    fn run(&mut self, scene: &mut SceneDb) -> SceneBufferProjection {
        scene.step();
        let mirror = scene.world.gpu_mirror().expect("mirror attached").clone();
        let mut inputs = SceneBufferProjection::from_store_all(mirror.store());
        let mut generations = None;
        mirror
            .generations()
            .with_buffer(&mut |buffer| generations = Some(buffer.clone()));
        inputs.insert(
            ENTITY_GENERATIONS_KEY,
            BufferHandle {
                buffer: generations.unwrap(),
                epoch: mirror.generations().epoch(),
                row_bytes: 4,
                content_generation: 0,
            },
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let output = self.join.derive(
            &SceneDerivationContext {
                device: &self.device,
                queue: &self.queue,
                inputs: &inputs,
                world_origin: None,
            },
            &mut encoder,
        );
        self.queue.submit([encoder.finish()]);
        for (key, handle) in output.buffers {
            inputs.insert(key, handle);
        }
        inputs
    }

    fn read<T: bytemuck::Pod>(&self, out: &SceneBufferProjection, key: &'static str) -> Vec<T> {
        let buffer = &out
            .get(BufferKey::of(key))
            .unwrap_or_else(|| panic!("{key} is published"))
            .buffer;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: buffer.size(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
        self.queue.submit([encoder.finish()]);
        staging.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let out = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
        staging.unmap();
        out
    }

    fn lens(&mut self, scene: &mut SceneDb, instance: Entity) -> LensComponent {
        let out = self.run(scene);
        self.read::<LensComponent>(&out, "camera_lenses")[instance.index() as usize]
    }

    fn elements(&mut self, scene: &mut SceneDb, first: u32) -> Vec<LensElementComponent> {
        let out = self.run(scene);
        self.read::<LensElementComponent>(&out, "lens_elements")
            [first as usize..first as usize + LENS_ELEMENT_SLOTS]
            .to_vec()
    }
}

fn camera(scene: &mut SceneDb, lens: CameraLensComponent) -> Entity {
    let object = scene
        .world
        .spawn_object(SpawnObject::new("camera"))
        .expect("spawn object");
    pulsar_world_registry::attach_value(&mut scene.world, object, lens).unwrap()
}

#[test]
fn an_authored_lens_reaches_the_passes_buffers_and_follows_edits_and_removal() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    // Authored before the mirror attaches: SceneDB replays it.
    let mut portrait = CameraLensComponent::default();
    portrait.view_id = 4;
    portrait.preset = LensPreset::Portrait85mmF1_4;
    let first = camera(&mut scene, portrait);
    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut gpu = Gpu {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };

    let row = gpu.lens(&mut scene, first);
    assert_eq!((row.enabled, row.view_id), (1, 4));
    assert_eq!((row.focal_length_mm, row.f_number), (85.0, 1.4));
    assert_eq!(row.element_count, 0);

    // Authored after: a 3-way split diopter, in the first element block.
    let mut split = CameraLensComponent::default();
    split.view_id = 0;
    split.preset = LensPreset::SplitDiopter3Way;
    let second = camera(&mut scene, split);
    let row = gpu.lens(&mut scene, second);
    assert_eq!((row.enabled, row.view_id, row.element_count), (1, 0, 2));
    let elements = gpu.elements(&mut scene, row.element_first);
    assert_eq!(elements[0].kind, element_kind::DIOPTER);
    assert_eq!(elements[0].params[0], 0.5);
    assert_eq!(elements[1].params[0], 1.1);
    assert!(elements[2..].iter().all(|e| e.kind == element_kind::NONE));
    let block = row.element_first;

    // An edit rewrites the row: Custom with one fewer element.
    {
        let mut lens = scene.world.get_mut::<CameraLensComponent>(second).unwrap();
        lens.bake_preset(LensPreset::SplitDiopter3Way);
        lens.elements.truncate(1);
        lens.f_number = 8.0;
    }
    let row = gpu.lens(&mut scene, second);
    assert_eq!(
        (row.f_number, row.element_count, row.element_first),
        (8.0, 1, block)
    );
    let elements = gpu.elements(&mut scene, block);
    assert_eq!(elements[0].kind, element_kind::DIOPTER);
    assert_eq!(
        elements[1].kind,
        element_kind::NONE,
        "the dropped element is zeroed"
    );

    // A third lens with a longer stack keeps to its own block.
    let mut third = CameraLensComponent::default();
    third.elements = vec![LensElementProps::default(); 3];
    let third = camera(&mut scene, third);
    let third_row = gpu.lens(&mut scene, third);
    assert_ne!(third_row.element_first, block);
    assert_eq!(third_row.element_count, 3);

    // The instance's enabled switch (the inspector's) turns the lens off
    // and back on, without touching the authored value.
    attachments::set_enabled(&mut scene.world, third, false);
    assert_eq!(gpu.lens(&mut scene, third).enabled, 0);
    attachments::set_enabled(&mut scene.world, third, true);
    assert_eq!(gpu.lens(&mut scene, third).enabled, 1);

    // The component's own switch does too.
    scene
        .world
        .get_mut::<CameraLensComponent>(third)
        .unwrap()
        .enabled = false;
    assert_eq!(gpu.lens(&mut scene, third).enabled, 0);

    // Removal zeroes the lens and its block (a stale row would keep a
    // deleted lens alive: the shader scans capacity).
    attachments::detach(&mut scene.world, second);
    let row = gpu.lens(&mut scene, second);
    assert_eq!((row.enabled, row.element_count), (0, 0));
    assert!(gpu
        .elements(&mut scene, block)
        .iter()
        .all(|e| e.kind == element_kind::NONE));
}

/// A 35 mm lens with an equidistant fisheye projection over the frame: a map
/// that looks a little past it, by an amount that depends on the aspect.
fn fisheye_35mm() -> CameraLensComponent {
    let mut lens = CameraLensComponent::default();
    lens.focal_length_mm = 35.0;
    lens.elements = vec![LensElementProps {
        kind: LensElementKind::Projection,
        shape: LensRegionShape::FullFrame,
        projection_model: LensProjectionModel::Equidistant,
        ..LensElementProps::default()
    }];
    lens
}

#[test]
fn the_cameras_overscan_is_the_one_the_lens_row_carries() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let lens = camera(&mut scene, fisheye_35mm());
    // A lens of another view and the neutral 50 mm lens.
    let mut other = fisheye_35mm();
    other.view_id = 3;
    camera(&mut scene, other);
    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut gpu = Gpu {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };

    let overscan = camera_lens_view_overscan(&scene.world, 0, 1.5);
    assert!(overscan > 1.05 && overscan <= 2.0, "{overscan}");
    let row = gpu.lens(&mut scene, lens);
    assert_eq!(row.overscan, overscan, "the pass crops by what the camera widened");
    // The camera widens its tangent extent by exactly that.
    let plain = helio::Camera::perspective_look_at(
        glam::Vec3::ZERO,
        glam::Vec3::NEG_Z,
        glam::Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        1.5,
        0.1,
        100.0,
    );
    let wide = plain.clone().with_overscan(overscan);
    assert!((plain.proj.y_axis.y / wide.proj.y_axis.y - overscan).abs() < 1e-5);

    // Another aspect is another view of the same lens: the row follows it.
    let wider = camera_lens_view_overscan(&scene.world, 0, 16.0 / 9.0);
    assert_ne!(wider, overscan);
    assert_eq!(gpu.lens(&mut scene, lens).overscan, wider);
    assert_eq!(camera_lens_view_overscan(&scene.world, 0, 16.0 / 9.0), wider, "cached");

    // An edit of the lens re-derives the row at the aspect last given.
    scene.world.get_mut::<CameraLensComponent>(lens).unwrap().focal_length_mm = 50.0;
    let edited = camera_lens_view_overscan(&scene.world, 0, 16.0 / 9.0);
    assert!(edited < wider, "a longer lens looks less far past the frame");
    assert_eq!(gpu.lens(&mut scene, lens).overscan, edited);

    // A view without a lens, a disabled lens and a lens that is off: no overscan.
    assert_eq!(camera_lens_view_overscan(&scene.world, 9, 1.5), 1.0);
    attachments::set_enabled(&mut scene.world, lens, false);
    assert_eq!(camera_lens_view_overscan(&scene.world, 0, 1.5), 1.0);
    attachments::set_enabled(&mut scene.world, lens, true);
    scene.world.get_mut::<CameraLensComponent>(lens).unwrap().enabled = false;
    assert_eq!(camera_lens_view_overscan(&scene.world, 0, 1.5), 1.0);
    // A world without a GPU mirror has no lens pass to crop.
    let mut bare = SceneDb::new();
    camera(&mut bare, fisheye_35mm());
    assert_eq!(camera_lens_view_overscan(&bare.world, 0, 1.5), 1.0);
}
