//! The plugin's join over the engine's rows: 2D sprites, authored as
//! component instances, reach the rows the plugin's passes read. Placement
//! and gating are read back from the join's outputs. Needs a GPU adapter
//! (lavapipe works); skips without one.

use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility};
use helio_core::ENTITY_GENERATIONS_KEY;
use helio_scene_join::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
};
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

/// 2D sprites (#1060): the join places a sprite at its owner's X and Y,
/// turned by the owner's roll and scaled by its X and Y scale (the owner's
/// Z does not apply), keeps its Z index as the sort depth, and drops it
/// while its owner is hidden or it is disabled.
#[test]
fn sprites_are_placed_in_2d_by_their_owner() {
    use sprites_component::SpriteComponent;
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let owner = place(
        &mut scene,
        "sprite",
        Transform {
            position: [120.0, -40.0, 999.0],
            rotation: [0.0, 0.0, 90.0],
            scale: [2.0, 0.5, 1.0],
        },
    );
    let sprite = SpriteComponent {
        tint: [0.0, 1.0, 0.0, 1.0],
        width: 30.0,
        height: 10.0,
        z_index: 4,
        ..Default::default()
    };
    let instance = pulsar_world_registry::attach_value(&mut scene.world, owner, sprite).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };
    // `helio_pass_sprite_batch::SpriteRow`: position, size, rotation,
    // depth, padding, UV rectangle, tint, then the texture slot.
    let row = |join: &Join, out: &SceneBufferProjection| -> [f32; 20] {
        join.row::<[f32; 20]>(out, "sprite_instances", instance)
    };

    let out = join.run(&mut scene);
    let placed = row(&join, &out);
    assert_eq!(
        placed[..4],
        [120.0, -40.0, 60.0, 5.0],
        "owner X/Y and X/Y scale"
    );
    assert!(
        (placed[4] - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
        "owner roll"
    );
    assert_eq!(placed[5], 4.0, "the Z index is the sort depth");
    assert_eq!(placed[8..12], [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(placed[12..16], [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(placed[16].to_bits(), u32::MAX, "no image");

    scene.world.get_mut::<Visibility>(owner).unwrap().visible = false;
    let out = join.run(&mut scene);
    assert_eq!(row(&join, &out), [0.0; 20], "a hidden owner hides it");
    scene.world.get_mut::<Visibility>(owner).unwrap().visible = true;
    scene
        .world
        .get_mut::<SpriteComponent>(instance)
        .unwrap()
        .enabled = false;
    let out = join.run(&mut scene);
    assert_eq!(row(&join, &out), [0.0; 20], "disabled");
}
