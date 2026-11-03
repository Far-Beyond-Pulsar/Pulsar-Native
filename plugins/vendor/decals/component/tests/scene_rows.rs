//! The plugin's join over the engine's rows: decals, authored as
//! component instances, reach the rows the plugin's passes read. Placement
//! and gating are read back from the join's outputs. Needs a GPU adapter
//! (lavapipe works); skips without one.

use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility, attachments};
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
}

fn place(scene: &mut SceneDb, name: &str, transform: Transform) -> Entity {
    let object = scene
        .world
        .spawn_object(SpawnObject::new(name))
        .expect("spawn object");
    *scene.world.get_mut::<Transform>(object).unwrap() = transform;
    object
}

/// Decals (#1058): an enabled decal of a visible owner becomes a decal pass
/// row, packed into the leading rows, whose transform maps the owner-placed
/// box onto -1..1; hiding the owner, disabling the instance or a zero
/// opacity's row (kept, inert) follow.
#[test]
fn decals_are_placed_in_their_owners_box() {
    use decals_component::DecalComponent;
    type DecalRow = helio_pass_decal::DecalRow;
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let owner = place(
        &mut scene,
        "decal",
        Transform {
            position: [10.0, 2.0, -4.0],
            rotation: [0.0, 90.0, 0.0],
            scale: [2.0, 1.0, 1.0],
        },
    );
    let decal = DecalComponent {
        size: [4.0, 2.0, 6.0],
        color: [0.0, 0.5, 1.0],
        opacity: 0.75,
        ..Default::default()
    };
    let instance = pulsar_world_registry::attach_value(&mut scene.world, owner, decal).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };

    let out = join.run(&mut scene);
    let rows: Vec<DecalRow> = join.read(&out, "decals");
    assert_eq!(rows.len(), helio_pass_decal::MAX_DECALS as usize);
    let row = rows[0];
    assert_eq!(row.color, [0.0, 0.5, 1.0, 0.75]);
    assert_eq!(row.fade_time, 0.0, "authored decals are permanent");
    // The box: owner at (10, 2, -4), yawed 90 degrees (local X along world
    // -Z, local Z along world +X), half extents (4, 1, 3) after the scale.
    let m = glam::Mat4::from_cols_array(&row.transform);
    let local = |world: [f32; 3]| m.transform_point3(glam::Vec3::from(world));
    assert!(local([10.0, 2.0, -4.0]).abs_diff_eq(glam::Vec3::ZERO, 1e-4));
    assert!(local([10.0, 2.0, -8.0]).abs_diff_eq(glam::Vec3::X, 1e-4));
    assert!(local([10.0, 3.0, -4.0]).abs_diff_eq(glam::Vec3::Y, 1e-4));
    assert!(local([13.0, 2.0, -4.0]).abs_diff_eq(glam::Vec3::Z, 1e-4));
    assert!(
        rows[1..].iter().all(|row| row.color[3] == 0.0),
        "one row placed"
    );

    scene.world.get_mut::<Visibility>(owner).unwrap().visible = false;
    let out = join.run(&mut scene);
    assert_eq!(
        join.read::<DecalRow>(&out, "decals")[0].color[3],
        0.0,
        "hidden owner"
    );
    scene.world.get_mut::<Visibility>(owner).unwrap().visible = true;
    attachments::set_enabled(&mut scene.world, instance, false);
    let out = join.run(&mut scene);
    assert_eq!(
        join.read::<DecalRow>(&out, "decals")[0].color[3],
        0.0,
        "disabled instance"
    );
}
