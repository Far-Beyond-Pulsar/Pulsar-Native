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
    resolve: Box<dyn SceneDerivation>,
    join: Box<dyn SceneDerivation>,
}

impl Join {
    /// Flush the scene and run the join over the frame's buffers, as the
    /// renderer does; returns the published buffers.
    fn run(&mut self, scene: &mut SceneDb) -> SceneBufferProjection {
        self.run_at(scene, None)
    }

    /// [`Self::run`] in a frame rebased to `origin` (world, f64), as a
    /// camera-relative frame is.
    fn run_at(&mut self, scene: &mut SceneDb, origin: Option<glam::DVec3>) -> SceneBufferProjection {
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

fn place(scene: &mut SceneDb, name: &str, position: [f64; 3]) -> Entity {
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


fn mesh_on(
    scene: &mut SceneDb,
    parent: Option<Entity>,
    transform: Transform,
) -> (Entity, Entity) {
    let object = scene
        .world
        .spawn_object(
            SpawnObject::new("o")
                .with_parent(parent)
                .with_transform(transform),
        )
        .unwrap();
    let instance = pulsar_world_registry::attach_value(
        &mut scene.world,
        object,
        mesh(ObjectMovability::Movable),
    )
    .unwrap();
    (object, instance)
}

fn position_of(join: &Join, out: &SceneBufferProjection, instance: Entity) -> [f32; 3] {
    join.mesh_rows(out, instance)[0].transform[3][..3]
        .try_into()
        .unwrap()
}

/// Objects are placed under their parents on the GPU: moving a folder is one
/// write to the folder, and everything in it follows, frame after frame.
#[test]
fn moving_a_folder_moves_the_meshes_inside_it() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let folder = place(&mut scene, "folder", [0.0, 0.0, 0.0]);
    let (_, outsider) = mesh_on(
        &mut scene,
        None,
        Transform {
            position: [100.0, 0.0, 0.0],
            ..Transform::default()
        },
    );
    let children: Vec<Entity> = (0..3)
        .map(|n| {
            mesh_on(
                &mut scene,
                Some(folder),
                Transform {
                    position: [3.0 + f64::from(n), 0.0, 0.0],
                    ..Transform::default()
                },
            )
            .1
        })
        .collect();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        resolve: engine_backend::scene::hierarchy_resolve(&device),
        join: engine_backend::scene::scene_join(&device, true),
        device,
        queue,
    };
    let out = join.run(&mut scene);
    assert_eq!(position_of(&join, &out, children[0]), [3.0, 0.0, 0.0]);

    for frame in 1..=4 {
        let x = f64::from(frame) * 2.5;
        // One write, to the folder alone.
        scene.world.get_mut::<Transform>(folder).unwrap().position = [x, 0.0, 0.0];
        let out = join.run(&mut scene);
        for (n, child) in children.iter().enumerate() {
            assert_eq!(
                position_of(&join, &out, *child),
                [(x + 3.0 + n as f64) as f32, 0.0, 0.0],
                "child {n} followed its folder on frame {frame}"
            );
        }
        assert_eq!(
            position_of(&join, &out, outsider),
            [100.0, 0.0, 0.0],
            "an object outside the folder stays put"
        );
    }
}

/// The GPU composes a chain of rotated, scaled and offset parents to the same
/// model matrix the CPU arithmetic gives (gizmos and picking rely on it).
#[test]
fn the_gpu_composes_a_parent_chain_like_the_cpu() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let root = place(&mut scene, "root", [0.0; 3]);
    {
        let mut t = scene.world.get_mut::<Transform>(root).unwrap();
        t.position = [5.0, -2.0, 1.0];
        t.rotation = [10.0, 35.0, -20.0];
        t.scale = [2.0, 2.0, 2.0];
    }
    let middle = scene
        .world
        .spawn_object(
            SpawnObject::new("middle")
                .with_parent(Some(root))
                .with_transform(Transform {
                    position: [1.0, 2.0, 3.0],
                    rotation: [-15.0, 50.0, 25.0],
                    scale: [0.5, 0.5, 0.5],
                }),
        )
        .unwrap();
    let (leaf, instance) = mesh_on(
        &mut scene,
        Some(middle),
        Transform {
            position: [-1.0, 0.5, 2.0],
            rotation: [5.0, -70.0, 15.0],
            scale: [1.0, 3.0, 2.0],
        },
    );

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        resolve: engine_backend::scene::hierarchy_resolve(&device),
        join: engine_backend::scene::scene_join(&device, true),
        device,
        queue,
    };
    let out = join.run(&mut scene);
    let model = join.mesh_rows(&out, instance)[0].transform;
    let expected = engine_backend::scene::hierarchy_transform::world_matrix_of(&scene.world, leaf)
        .unwrap()
        .as_mat4()
        .to_cols_array_2d();
    for (c, (got, want)) in model.iter().zip(expected).enumerate() {
        for r in 0..3 {
            assert!(
                (got[r] - want[r]).abs() < 1e-3,
                "model column {c} row {r}: gpu {got:?} cpu {want:?}"
            );
        }
    }
}

/// On a planet: a mesh under a turned folder 6_371 km from the world's
/// origin, where an f32 position steps by half a metre. The scene keeps its
/// f64 position, the GPU composes the chain in f32 pairs, and a frame
/// rebased near it places it within a tenth of a millimetre of the CPU's
/// f64 answer, frame after frame as the folder moves by millimetres.
#[test]
fn a_mesh_on_a_planet_keeps_its_millimetres() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let folder = place(&mut scene, "folder", [1000.0, 6_371_002.371, -300.0]);
    scene.world.get_mut::<Transform>(folder).unwrap().rotation = [0.0, 90.0, 0.0];
    let (leaf, instance) = mesh_on(
        &mut scene,
        Some(folder),
        Transform {
            position: [0.25, 0.5, 0.125],
            ..Transform::default()
        },
    );

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        resolve: engine_backend::scene::hierarchy_resolve(&device),
        join: engine_backend::scene::scene_join(&device, true),
        device,
        queue,
    };
    // The frame origin as the renderer snaps it: on a 1024 m cell.
    let origin = glam::DVec3::new(1024.0, 6_371_328.0, 0.0);
    for frame in 0..4 {
        let step = f64::from(frame) * 0.001;
        scene.world.get_mut::<Transform>(folder).unwrap().position =
            [1000.0 + step, 6_371_002.371, -300.0 - step];
        let out = join.run_at(&mut scene, Some(origin));
        let got = glam::Vec3::from_slice(&position_of(&join, &out, instance));
        let world = engine_backend::scene::hierarchy_transform::world_matrix_of(&scene.world, leaf)
            .unwrap()
            .w_axis
            .truncate();
        let want = (world - origin).as_vec3();
        assert!(
            (got - want).abs().max_element() < 1e-4,
            "frame {frame}: gpu {got} cpu {want}"
        );
    }
}
