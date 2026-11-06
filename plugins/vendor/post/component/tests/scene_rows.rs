//! The post-process plugin's join over the engine's rows: post-process
//! volumes and camera post-process components, authored as component
//! instances, reach the rows the volume blend reads. Placement and gating are
//! read back from the join's output. Needs a GPU adapter (lavapipe works);
//! skips without one.

use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility, attachments};
use helio_core::ENTITY_GENERATIONS_KEY;
use helio_scene_join::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
};
use post_component::{CameraPostProcessComponent, PostProcessVolumeComponent};
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

struct Join {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    join: Box<dyn SceneDerivation>,
}

impl Join {
    fn run(&mut self, scene: &mut SceneDb) -> SceneBufferProjection {
        self.run_after(scene, None)
    }

    /// Run the join after `before` (the scene join, whose lights the water
    /// rows read), as the renderer orders them.
    fn run_after(
        &mut self,
        scene: &mut SceneDb,
        before: Option<&mut Box<dyn SceneDerivation>>,
    ) -> SceneBufferProjection {
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
        if let Some(before) = before {
            let output = before.derive(
                &SceneDerivationContext {
                    device: &self.device,
                    queue: &self.queue,
                    inputs: &inputs,
                    world_origin: glam::DVec3::ZERO,
                },
                &mut encoder,
            );
            for (key, handle) in output.buffers {
                inputs.insert(key, handle);
            }
        }
        let output = self.join.derive(
            &SceneDerivationContext {
                device: &self.device,
                queue: &self.queue,
                inputs: &inputs,
                world_origin: glam::DVec3::ZERO,
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
        let buffer = &out.get(BufferKey::of(key)).expect("published").buffer;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: buffer.size(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
        self.queue.submit([encoder.finish()]);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let out = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
        staging.unmap();
        out
    }

    fn row<T: bytemuck::Pod>(
        &self,
        out: &SceneBufferProjection,
        key: &'static str,
        instance: Entity,
    ) -> T {
        self.read::<T>(out, key)[instance.index() as usize]
    }
}

fn place(scene: &mut SceneDb, name: &str, transform: Transform) -> Entity {
    let object = scene
        .world
        .spawn_object(SpawnObject::new(name))
        .expect("spawn object");
    *scene.world.get_mut::<Transform>(object).unwrap() = transform;
    object
}

fn at(position: [f64; 3]) -> Transform {
    Transform {
        position,
        ..Transform::default()
    }
}

type VolumeRow = helio_pass_post_volumes::PostProcessVolumeRow;
type CameraRow = helio_pass_post_volumes::CameraPostProcessRow;

fn close(a: [f32; 4], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
}

#[test]
fn the_environment_join_places_post_process_rows() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    // Populated before the mirror attaches: SceneDB replays every row.
    let grading = place(&mut scene, "grading", at([0.0, 5.0, 0.0]));
    let mut volume = PostProcessVolumeComponent::default();
    volume.size = [4.0, 4.0, 4.0];
    volume.blend_weight = 0.75;
    volume.priority = 3.0;
    let volume = pulsar_world_registry::attach_value(&mut scene.world, grading, volume).unwrap();

    let camera_object = place(&mut scene, "camera", at([0.0; 3]));
    let mut camera = CameraPostProcessComponent::default();
    camera.view_id = 0;
    let camera =
        pulsar_world_registry::attach_value(&mut scene.world, camera_object, camera).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };

    let out = join.run(&mut scene);
    let row: VolumeRow = join.row(&out, "post_process_volumes", volume);
    assert!(close(row.bounds_min, [-2.0, 3.0, -2.0]) && close(row.bounds_max, [2.0, 7.0, 2.0]));
    assert_eq!((row.blend_weight, row.priority), (0.75, 3.0));

    let row: CameraRow = join.row(&out, "camera_postprocess", camera);
    assert_eq!((row.view_id, row.enabled), (0, 1), "camera baseline placed");

    // Hidden owners turn volumes off; a camera baseline is not visual.
    for object in [grading, camera_object] {
        scene.world.get_mut::<Visibility>(object).unwrap().visible = false;
    }
    let out = join.run(&mut scene);
    assert_eq!(
        join.row::<VolumeRow>(&out, "post_process_volumes", volume)
            .blend_weight,
        0.0
    );
    assert_eq!(
        join.row::<CameraRow>(&out, "camera_postprocess", camera)
            .enabled,
        1
    );
    for object in [grading, camera_object] {
        scene.world.get_mut::<Visibility>(object).unwrap().visible = true;
    }

    // Disabled instances and disabled components write inert rows.
    attachments::set_enabled(&mut scene.world, camera, false);
    scene
        .world
        .get_mut::<PostProcessVolumeComponent>(volume)
        .unwrap()
        .enabled = false;
    let out = join.run(&mut scene);
    assert_eq!(
        join.row::<CameraRow>(&out, "camera_postprocess", camera)
            .enabled,
        0
    );
    assert_eq!(
        join.row::<VolumeRow>(&out, "post_process_volumes", volume)
            .blend_weight,
        0.0
    );
}
