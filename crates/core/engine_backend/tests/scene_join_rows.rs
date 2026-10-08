//! Helio's scene join over this engine's own rows (Pulsar-Native#1035,
//! Phase 2): the buffers `ensure_gpu_mirror` and the components register,
//! under the keys `scene_join_keys` names, joined on the GPU. Covers what a
//! rendered frame cannot show directly (the movable flag a static/movable
//! edit sets, the row a light gets) and a world populated before the mirror
//! attached. Needs a GPU adapter (lavapipe works); skips without one.

use std::sync::Arc;

use engine_backend::scene::{attachments, SceneWorldExt, SpawnObject, Transform, Visibility};
use helio_component::components::{
    LightComponent, MeshAssetPath, ObjectMovability, StaticMeshComponent,
};
use helio_default_graphs::scene_join::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
    ENTITY_GENERATIONS_KEY,
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

/// A two-triangle mesh in two material sections.
fn mesh(movability: ObjectMovability) -> StaticMeshComponent {
    let vertex = |x: f32, y: f32| helio::PackedVertex {
        position: [x, y, 0.0],
        ..Default::default()
    };
    StaticMeshComponent {
        mesh_asset: MeshAssetPath::new(""),
        vertices: vec![
            vertex(0.0, 0.0),
            vertex(1.0, 0.0),
            vertex(0.0, 1.0),
            vertex(1.0, 1.0),
        ],
        indices: vec![0, 1, 2, 2, 1, 3],
        mesh_sections: vec![
            helio_component::mesh_cache::MeshSection {
                first_index: 0,
                index_count: 3,
                material_slot: 0,
            },
            helio_component::mesh_cache::MeshSection {
                first_index: 3,
                index_count: 3,
                material_slot: 0,
            },
        ],
        bounds_local: [0.5, 0.5, 0.0, 0.8],
        movability,
        ..Default::default()
    }
}

struct Join {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    join: Box<dyn SceneDerivation>,
}

impl Join {
    /// Flush the scene and run the join over the frame's buffers, as the
    /// renderer does; returns the published buffers.
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

    /// The live object rows of mesh instance `instance`.
    fn mesh_rows(
        &self,
        out: &SceneBufferProjection,
        instance: Entity,
    ) -> Vec<helio_pass_gbuffer::StaticObjectComponent> {
        self.read::<helio_pass_gbuffer::StaticObjectComponent>(out, "static_objects")
            .into_iter()
            .filter(|row| row.mesh_generation != 0 && row.mesh_slot == instance.index())
            .collect()
    }

    fn light_row(&self, out: &SceneBufferProjection, instance: Entity) -> helio::GpuLight {
        self.read::<helio::GpuLight>(out, "scene_lights")[instance.index() as usize]
    }
}

fn place(scene: &mut SceneDb, name: &str, position: [f32; 3]) -> Entity {
    let object = scene
        .world
        .spawn_object(SpawnObject::new(name))
        .expect("spawn object");
    scene.world.get_mut::<Transform>(object).unwrap().position = position;
    object
}

#[test]
fn the_join_follows_the_engines_authored_rows() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    // Populated before the mirror attaches: SceneDB replays every row,
    // derived ones included.
    let mut scene = SceneDb::new();
    let object = place(&mut scene, "mesh", [3.0, 0.0, 0.0]);
    let instance = pulsar_world_registry::attach_value(
        &mut scene.world,
        object,
        mesh(ObjectMovability::Static),
    )
    .unwrap();
    let lamp = place(&mut scene, "lamp", [0.0, 5.0, 2.0]);
    let mut light = LightComponent::default();
    light.general.enabled = true;
    let light_instance =
        pulsar_world_registry::attach_value(&mut scene.world, lamp, light).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::scene_join(&device, true),
        device,
        queue,
    };

    let out = join.run(&mut scene);
    let rows = join.mesh_rows(&out, instance);
    assert_eq!(rows.len(), 2, "one object row per section");
    assert!(
        rows.iter()
            .all(|row| row.flags & helio::INSTANCE_FLAG_MOVABLE == 0),
        "static"
    );
    assert_eq!(
        rows[0].transform[3][..3],
        [3.0, 0.0, 0.0],
        "placed by its owner"
    );
    let lit = join.light_row(&out, light_instance);
    assert_eq!(
        lit.position_range[..3],
        [0.0, 5.0, 2.0],
        "the light is at its owner"
    );
    assert!(lit.color_intensity[3] > 0.0);

    // A movability edit through the reflected property path.
    pulsar_world_registry::set_world_component_property(
        "StaticMeshComponent",
        &mut scene.world,
        instance,
        "movability",
        Box::new(ObjectMovability::Movable),
    )
    .unwrap();
    let out = join.run(&mut scene);
    let rows = join.mesh_rows(&out, instance);
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.flags & helio::INSTANCE_FLAG_MOVABLE != 0),
        "movable"
    );

    scene.world.get_mut::<Transform>(object).unwrap().position = [-1.0, 2.0, 0.0];
    let out = join.run(&mut scene);
    assert_eq!(
        join.mesh_rows(&out, instance)[0].transform[3][..3],
        [-1.0, 2.0, 0.0],
        "moved"
    );

    attachments::set_enabled(&mut scene.world, instance, false);
    attachments::set_enabled(&mut scene.world, light_instance, false);
    let out = join.run(&mut scene);
    assert!(join.mesh_rows(&out, instance).is_empty(), "disabled mesh");
    assert_eq!(
        join.light_row(&out, light_instance).color_intensity,
        [0.0; 4],
        "disabled light"
    );
    attachments::set_enabled(&mut scene.world, instance, true);
    attachments::set_enabled(&mut scene.world, light_instance, true);

    scene.world.get_mut::<Visibility>(lamp).unwrap().visible = false;
    let out = join.run(&mut scene);
    assert_eq!(join.mesh_rows(&out, instance).len(), 2, "re-enabled");
    assert_eq!(
        join.light_row(&out, light_instance).color_intensity,
        [0.0; 4],
        "hidden owner"
    );

    scene
        .world
        .get_mut::<LightComponent>(light_instance)
        .unwrap()
        .general
        .enabled = false;
    scene.world.get_mut::<Visibility>(lamp).unwrap().visible = true;
    let out = join.run(&mut scene);
    assert_eq!(
        join.light_row(&out, light_instance).color_intensity,
        [0.0; 4],
        "light switched off"
    );

    attachments::detach(&mut scene.world, instance);
    let out = join.run(&mut scene);
    assert!(join.mesh_rows(&out, instance).is_empty(), "removed");
}
