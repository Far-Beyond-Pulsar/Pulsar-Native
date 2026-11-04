//! The engine's object joins with the lights' installed: the light list
//! follows the authored light instances and their owners (moved from
//! engine_backend's scene_join_rows with the light component).
use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility, attachments};
use helio_component::components::{MeshAssetPath, ObjectMovability, StaticMeshComponent};
use helio_pass_object_batch::scene_join::{
    BufferHandle, BufferKey, ENTITY_GENERATIONS_KEY, SceneBufferProjection, SceneDerivation,
    SceneDerivationContext,
};
use lights_component::LightComponent;
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
    let vertex = |x: f32, y: f32| helio_mesh_api::PackedVertex {
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
    resolve: Box<dyn SceneDerivation>,
    join: Box<dyn SceneDerivation>,
}

impl Join {
    /// Flush the scene and run the join over the frame's buffers, as the
    /// renderer does; returns the published buffers.
    fn run(&mut self, scene: &mut SceneDb) -> SceneBufferProjection {
        self.run_at(scene, glam::DVec3::ZERO)
    }

    /// [`Self::run`] in a frame whose world origin is `origin`.
    fn run_at(&mut self, scene: &mut SceneDb, origin: glam::DVec3) -> SceneBufferProjection {
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
        let resolved = self.resolve.derive(
            &SceneDerivationContext {
                device: &self.device,
                queue: &self.queue,
                inputs: &inputs,
                world_origin: origin,
            },
            &mut encoder,
        );
        for (key, handle) in resolved.buffers {
            inputs.insert(key, handle);
        }
        for (key, rows) in resolved.flushed {
            inputs.set_flushed_rows(key, rows);
        }
        let output = self.join.derive(
            &SceneDerivationContext {
                device: &self.device,
                queue: &self.queue,
                inputs: &inputs,
                world_origin: origin,
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
    ) -> Vec<helio_mesh_api::StaticObjectRow> {
        self.read::<helio_mesh_api::StaticObjectRow>(out, "static_objects")
            .into_iter()
            .filter(|row| row.mesh_generation != 0 && row.mesh_slot == instance.index())
            .collect()
    }

    fn light_row(
        &self,
        out: &SceneBufferProjection,
        instance: Entity,
    ) -> helio_light_api::GpuLight {
        self.read::<helio_light_api::GpuLight>(out, helio_light_api::LIGHT_LIST)
            [instance.index() as usize]
    }
}

fn place(scene: &mut SceneDb, name: &str, position: [f64; 3]) -> Entity {
    let object = scene
        .world
        .spawn_object(SpawnObject::new(name))
        .expect("spawn object");
    scene.world.get_mut::<Transform>(object).unwrap().position = position;
    object
}

fn join_for(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> Join {
    Join {
        resolve: engine_backend::scene::hierarchy_resolve(device),
        join: engine_backend::scene::scene_join(device, true),
        device: Arc::clone(device),
        queue: Arc::clone(queue),
    }
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
        resolve: engine_backend::scene::hierarchy_resolve(&device),
        join: engine_backend::scene::scene_join(&device, true),
        device,
        queue,
    };

    let out = join.run(&mut scene);
    let rows = join.mesh_rows(&out, instance);
    assert_eq!(rows.len(), 2, "one object row per section");
    assert!(
        rows.iter()
            .all(|row| row.flags & helio_mesh_api::INSTANCE_FLAG_MOVABLE == 0),
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
            .all(|row| row.flags & helio_mesh_api::INSTANCE_FLAG_MOVABLE != 0),
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

/// Instance lifecycle (Pulsar-Native#1035 acceptance, #1081): several light
/// instances on one object, edited through the reflected property path, a
/// script component reference and a duplicate, each land in their own
/// joined light row, equal to a typed insert of the same value. A slot
/// recycled for a new instance is a new instance: the old handle no longer
/// resolves, and the row carries the new value.
#[test]
fn every_path_addresses_one_instance_and_a_reused_index_is_a_new_one() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let light = |intensity: f32| {
        let mut light = LightComponent::default();
        light.general.enabled = true;
        light.intensity.intensity = intensity;
        light
    };
    let mut scene = SceneDb::new();
    let lamp = place(&mut scene, "lamps", [0.0, 5.0, 2.0]);
    let reference = place(&mut scene, "reference", [0.0, 5.0, 2.0]);
    let first = pulsar_world_registry::attach_value(&mut scene.world, lamp, light(10.0)).unwrap();
    let second = pulsar_world_registry::attach_value(&mut scene.world, lamp, light(20.0)).unwrap();
    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        resolve: engine_backend::scene::hierarchy_resolve(&device),
        join: engine_backend::scene::scene_join(&device, true),
        device,
        queue,
    };

    // The reflected property path (properties panel, AI tools).
    pulsar_world_registry::set_world_component_property(
        "LightComponent",
        &mut scene.world,
        second,
        "intensity",
        Box::new(40.0_f32),
    )
    .unwrap();
    // A script's reference to the lamp's first light.
    let script_ref = pulsar_script_object_model::ActorRef::new(lamp).component("LightComponent", 0);
    assert_eq!(script_ref.instance(&scene.world), Some(first));
    script_ref
        .set_property(&mut scene.world, "intensity", Box::new(30.0_f32))
        .unwrap();
    // A duplicate is its own instance with its own value.
    let copy =
        pulsar_world_registry::duplicate_instance(&mut scene.world, second, lamp, None).unwrap();
    scene
        .world
        .get_mut::<LightComponent>(copy)
        .unwrap()
        .intensity
        .intensity = 50.0;
    assert_ne!(
        attachments::meta(&scene.world, copy).unwrap().id,
        attachments::meta(&scene.world, second).unwrap().id,
        "a duplicate has its own identity"
    );

    let references: Vec<Entity> = [30.0, 40.0, 50.0]
        .map(|intensity| {
            pulsar_world_registry::attach_value(&mut scene.world, reference, light(intensity))
                .unwrap()
        })
        .to_vec();
    let out = join.run(&mut scene);
    let row = |instance: Entity| bytemuck::bytes_of(&join.light_row(&out, instance)).to_vec();
    for (instance, typed) in [first, second, copy].into_iter().zip(&references) {
        assert_eq!(
            row(instance),
            row(*typed),
            "{instance:?} matches a typed insert"
        );
    }
    assert_ne!(row(first), row(second));
    assert_ne!(row(second), row(copy));

    // Recycle the first light's slot.
    attachments::detach(&mut scene.world, first);
    let reused = pulsar_world_registry::attach_value(&mut scene.world, lamp, light(70.0)).unwrap();
    assert_eq!(reused.index(), first.index(), "the freed slot is reused");
    assert_ne!(reused, first, "with a new generation");
    assert!(scene.world.get::<LightComponent>(first).is_none());
    assert!(
        pulsar_world_registry::set_world_component_property(
            "LightComponent",
            &mut scene.world,
            first,
            "intensity",
            Box::new(1.0_f32),
        )
        .is_err(),
        "the stale handle writes nothing"
    );
    let typed =
        pulsar_world_registry::attach_value(&mut scene.world, reference, light(70.0)).unwrap();
    let out = join.run(&mut scene);
    let row = |instance: Entity| bytemuck::bytes_of(&join.light_row(&out, instance)).to_vec();
    assert_eq!(
        row(reused),
        row(typed),
        "the recycled row carries the new instance"
    );
    assert_eq!(
        scene
            .world
            .get::<LightComponent>(reused)
            .unwrap()
            .intensity
            .intensity,
        70.0
    );
}

/// A light (and its editor icon) under a parent far from the origin is offset
/// by the frame's origin exactly once: the hierarchy resolve publishes world rows, and the join
/// takes the origin off them as it does for a flat scene. (When the resolve
/// published origin-relative rows, a light parented on a planet was offset
/// twice and lit from 6 400 km away.)
#[test]
fn parented_lights_are_offset_by_the_origin_once() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    const FAR: f64 = 6_400_000.0;
    let mut scene = SceneDb::new();
    let folder = place(&mut scene, "folder", [FAR, 0.0, 0.0]);
    let lamp = scene
        .world
        .spawn_object(
            SpawnObject::new("lamp")
                .with_parent(Some(folder))
                .with_transform(Transform {
                    position: [0.25, 2.0, 0.0],
                    ..Transform::default()
                }),
        )
        .unwrap();
    let mut light = LightComponent::default();
    light.general.enabled = true;
    let light = pulsar_world_registry::attach_value(&mut scene.world, lamp, light).unwrap();
    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = join_for(&device, &queue);
    let out = join.run_at(&mut scene, glam::DVec3::new(FAR, 0.0, 0.0));
    assert_eq!(
        &join.light_row(&out, light).position_range[..3],
        &[0.25, 2.0, 0.0]
    );
    // Its editor icon sits with it, in the frame's space.
    let billboard = join.read::<[f32; 12]>(&out, "billboard_instances")[light.index() as usize];
    assert_eq!(&billboard[..3], &[0.25, 2.0, 0.0]);
}
