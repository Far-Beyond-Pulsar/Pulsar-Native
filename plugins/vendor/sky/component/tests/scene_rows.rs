//! The sky plugin's join over the engine's rows (moved from engine_backend's
//! environment join test): atmospheres, authored as component instances,
//! reach the rows the sky reads. Placement and gating are read back from
//! the join's outputs. Needs a GPU adapter (lavapipe works); skips without
//! one.

use std::sync::Arc;

use engine_backend::scene::{attachments, SceneWorldExt, SpawnObject, Transform, Visibility};
use helio_pass_object_batch::scene_join::ENTITY_GENERATIONS_KEY;
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

    /// Run the join after `before` (the scene join, whose lights some
    /// kinds' rows read), as the renderer orders them.
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

type AtmosphereRow = helio_pass_sky::AtmosphereRow;

#[test]
fn atmospheres_follow_their_owner_and_their_enabled_state() {
    use sky_component::{AtmosphereComponent, AtmospherePlacement};
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    // A planet centred on its owner, and flat ground's air elsewhere (its
    // planet stays below the world origin wherever its owner is).
    let world_object = place(&mut scene, "planet", at([10.0, -20.0, 30.0]));
    let planet = AtmosphereComponent {
        placement: AtmospherePlacement::PlanetAtOwner,
        planet_radius_km: 1737.0,
        ..Default::default()
    };
    let planet =
        pulsar_world_registry::attach_value(&mut scene.world, world_object, planet).unwrap();
    let ground_object = place(&mut scene, "ground", at([5.0, 5.0, 5.0]));
    let ground = pulsar_world_registry::attach_value(
        &mut scene.world,
        ground_object,
        AtmosphereComponent::default(),
    )
    .unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };

    let out = join.run(&mut scene);
    let row: AtmosphereRow = join.row(&out, "atmospheres", planet);
    assert_eq!(row.enabled, 1, "planet placed");
    assert_eq!(row.center, [10.0, -20.0, 30.0], "centred on its owner");
    assert_eq!(row.placement, helio_pass_sky::atmosphere::placement::CENTER);
    assert_eq!(row.bottom_radius, 1737.0);
    let row: AtmosphereRow = join.row(&out, "atmospheres", ground);
    assert_eq!(row.enabled, 1, "ground air placed");
    assert_eq!(row.center, [0.0; 3], "the ground is at the world origin");

    // The air is not a visual of its owner: hiding the owner keeps it.
    scene
        .world
        .get_mut::<Visibility>(world_object)
        .unwrap()
        .visible = false;
    scene
        .world
        .get_mut::<Transform>(world_object)
        .unwrap()
        .position = [-1.0, 2.0, -3.0];
    let out = join.run(&mut scene);
    let row: AtmosphereRow = join.row(&out, "atmospheres", planet);
    assert_eq!(row.enabled, 1);
    assert_eq!(row.center, [-1.0, 2.0, -3.0], "follows its owner");

    // A disabled instance or component leaves an inert row; removal clears it.
    attachments::set_enabled(&mut scene.world, planet, false);
    scene
        .world
        .get_mut::<AtmosphereComponent>(ground)
        .unwrap()
        .enabled = false;
    let out = join.run(&mut scene);
    assert_eq!(
        join.row::<AtmosphereRow>(&out, "atmospheres", planet)
            .enabled,
        0
    );
    assert_eq!(
        join.row::<AtmosphereRow>(&out, "atmospheres", ground)
            .enabled,
        0
    );
    attachments::set_enabled(&mut scene.world, planet, true);
    attachments::detach(&mut scene.world, planet);
    let out = join.run(&mut scene);
    assert_eq!(
        join.row::<AtmosphereRow>(&out, "atmospheres", planet)
            .enabled,
        0
    );
}
