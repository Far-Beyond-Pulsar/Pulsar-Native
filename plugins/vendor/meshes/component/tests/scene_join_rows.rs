//! Helio's scene join over this engine's own rows (Pulsar-Native#1035,
//! Phase 2): the buffers `ensure_gpu_mirror` and the components register,
//! under the keys `scene_join_keys` names, joined on the GPU. Covers what a
//! rendered frame cannot show directly (the movable flag a static/movable
//! edit sets, the row a light gets) and a world populated before the mirror
//! attached. Needs a GPU adapter (lavapipe works); skips without one.

use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility, attachments};
use helio_component::components::ObjectMovability;
use helio_pass_object_batch::scene_join::{
    BufferHandle, BufferKey, ENTITY_GENERATIONS_KEY, SceneBufferProjection, SceneDerivation,
    SceneDerivationContext,
};
use meshes_component::components::{MeshAssetPath, StaticMeshComponent};
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
            meshes_component::mesh_cache::MeshSection {
                first_index: 0,
                index_count: 3,
                material_slot: 0,
            },
            meshes_component::mesh_cache::MeshSection {
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
        // The meshes' material instances are a scene subsystem's.
        pulsar_world_registry::install_scene_subsystems(scene);
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
        self.read::<helio_mesh_api::StaticObjectRow>(out, helio_mesh_api::STATIC_OBJECTS.name())
            .into_iter()
            .filter(|row| row.mesh_generation != 0 && row.mesh_slot == instance.index())
            .collect()
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

fn mesh_on(scene: &mut SceneDb, parent: Option<Entity>, transform: Transform) -> (Entity, Entity) {
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

fn join_for(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> Join {
    Join {
        resolve: engine_backend::scene::hierarchy_resolve(device),
        join: engine_backend::scene::scene_join(device, true),
        device: Arc::clone(device),
        queue: Arc::clone(queue),
    }
}

fn row_model(join: &Join, out: &SceneBufferProjection, instance: Entity) -> (glam::Mat4, u32) {
    let row = join.mesh_rows(out, instance)[0];
    (
        glam::Mat4::from_cols_array_2d(&row.transform),
        (row.flags >> 8) & 0xFF,
    )
}

fn assert_close(got: glam::Mat4, want: glam::Mat4, what: &str) {
    let (g, w) = (got.to_cols_array(), want.to_cols_array());
    assert!(
        g.iter().zip(&w).all(|(a, b)| (a - b).abs() < 2e-3),
        "{what}: got {g:?}, want {w:?}"
    );
}

fn at_x(x: f64) -> Transform {
    Transform {
        position: [x, 0.0, 0.0],
        ..Transform::default()
    }
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
        let out = join.run_at(&mut scene, origin);
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

/// Far from the origin, a parent's children keep the precision of their
/// offset: the chain is composed in f32 pairs and the join takes the frame's
/// origin off pair-wise, not in absolute f32 (whose step at this distance is
/// half a metre).
#[test]
fn nested_objects_far_from_the_origin_keep_their_offsets() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    const FAR: f64 = 6_400_000.0;
    let origin = glam::DVec3::new(FAR, 0.0, 0.0);
    let mut scene = SceneDb::new();
    let folder = place(&mut scene, "folder", [FAR, 0.0, 0.0]);
    let (_, near) = mesh_on(
        &mut scene,
        Some(folder),
        Transform {
            position: [0.25, 0.0, 0.0],
            ..Transform::default()
        },
    );
    // An object with no parent shares the buffer: it must not be offset twice.
    let (_, root) = mesh_on(
        &mut scene,
        None,
        Transform {
            position: [FAR + 3.0, 0.0, 0.0],
            ..Transform::default()
        },
    );
    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = join_for(&device, &queue);
    let out = join.run_at(&mut scene, origin);
    assert_eq!(
        position_of(&join, &out, near),
        [0.25, 0.0, 0.0],
        "the child kept its quarter metre (absolute f32 would round it to 0 or 0.5)"
    );
    assert_eq!(
        position_of(&join, &out, root),
        [3.0, 0.0, 0.0],
        "an unparented object is offset by the origin exactly once"
    );
}

/// A flat scene (no hierarchy resolve runs) is offset by the joins the same
/// way.
#[test]
fn a_flat_scene_is_still_made_origin_relative_by_the_join() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    const FAR: f64 = 6_400_000.0;
    let mut scene = SceneDb::new();
    let (_, flat) = mesh_on(
        &mut scene,
        None,
        Transform {
            position: [FAR + 4.0, 0.0, 0.0],
            ..Transform::default()
        },
    );
    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = join_for(&device, &queue);
    let out = join.run_at(&mut scene, glam::DVec3::new(FAR, 0.0, 0.0));
    assert_eq!(position_of(&join, &out, flat), [4.0, 0.0, 0.0]);
}

/// In a scene with a hierarchy only what moved is redone, and the join takes
/// its moved-owner path: moving one child, then a whole folder, then
/// reparenting, each lands exactly and leaves everything else alone.
#[test]
fn hierarchical_scenes_follow_only_what_moved() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let folder = place(&mut scene, "folder", [0.0; 3]);
    let at = |x: f64| Transform {
        position: [x, 0.0, 0.0],
        ..Transform::default()
    };
    let (inner_object, inner) = mesh_on(&mut scene, Some(folder), at(1.0));
    let (_, other_inner) = mesh_on(&mut scene, Some(folder), at(2.0));
    let (loose_object, loose) = mesh_on(&mut scene, None, at(50.0));
    // Enough unrelated objects that a small move is well under the "most of
    // the scene moved" share.
    for n in 0..16 {
        mesh_on(&mut scene, None, at(100.0 + n as f64));
    }
    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = join_for(&device, &queue);
    let out = join.run(&mut scene);
    assert_eq!(position_of(&join, &out, inner), [1.0, 0.0, 0.0]);

    let moved_generation = |out: &SceneBufferProjection| {
        out.get(helio_pass_object_batch::scene_join::OBJECTS_MOVED_KEY)
            .map_or(0, |h| h.content_generation)
    };
    let mut last_moved_generation = 0;
    for frame in 1..=4u32 {
        let step = frame as f32;
        // One child moves.
        scene
            .world
            .get_mut::<Transform>(inner_object)
            .unwrap()
            .position[1] = f64::from(step);
        let out = join.run(&mut scene);
        assert_eq!(
            position_of(&join, &out, inner),
            [1.0, step, 0.0],
            "child, frame {frame}"
        );
        assert_eq!(
            position_of(&join, &out, other_inner),
            [2.0, 0.0, 0.0],
            "sibling untouched"
        );
        assert_eq!(
            position_of(&join, &out, loose),
            [50.0, 0.0, 0.0],
            "loose untouched"
        );
        last_moved_generation = moved_generation(&out);
    }
    assert_ne!(
        last_moved_generation, 0,
        "the join took its moved-owner path in a hierarchical scene"
    );

    // The whole folder moves: its contents follow, loose objects do not.
    scene.world.get_mut::<Transform>(folder).unwrap().position = [10.0, 0.0, 0.0];
    let out = join.run(&mut scene);
    assert_eq!(position_of(&join, &out, inner), [11.0, 4.0, 0.0]);
    assert_eq!(position_of(&join, &out, other_inner), [12.0, 0.0, 0.0]);
    assert_eq!(position_of(&join, &out, loose), [50.0, 0.0, 0.0]);

    // A loose object is put in the folder (keeping its world pose).
    engine_backend::scene::hierarchy_transform::reparent_keeping_world(
        &mut scene.world,
        loose_object,
        Some(folder),
    )
    .unwrap();
    let out = join.run(&mut scene);
    assert_eq!(
        position_of(&join, &out, loose),
        [50.0, 0.0, 0.0],
        "same place after reparenting"
    );
    // Now it moves with the folder.
    scene.world.get_mut::<Transform>(folder).unwrap().position = [20.0, 0.0, 0.0];
    let out = join.run(&mut scene);
    assert_eq!(position_of(&join, &out, loose), [60.0, 0.0, 0.0]);
    assert_eq!(position_of(&join, &out, inner), [21.0, 4.0, 0.0]);
}

/// A mesh inside rigid frames is placed relative to its nearest frame and
/// tagged with that frame's space; the frame table finishes the job. Moving a
/// frame changes the table and none of its members' rows.
#[test]
fn frame_members_are_placed_in_their_frame_and_follow_the_table() {
    use engine_backend::scene::frames::{frame_table, make_frame};
    use engine_backend::scene::hierarchy_transform::world_matrix_of;
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let ship = place(&mut scene, "ship", [100.0, 0.0, 0.0]);
    scene.world.get_mut::<Transform>(ship).unwrap().rotation = [0.0, 30.0, 0.0];
    let room = scene
        .world
        .spawn_object(
            SpawnObject::new("room")
                .with_parent(Some(ship))
                .with_transform(Transform {
                    position: [0.0, 2.0, 5.0],
                    rotation: [0.0, 10.0, 0.0],
                    ..Transform::default()
                }),
        )
        .unwrap();
    let (leaf, leaf_mesh) = mesh_on(
        &mut scene,
        Some(room),
        Transform {
            position: [3.0, 0.5, -1.0],
            scale: [1.0, 2.0, 1.0],
            ..Transform::default()
        },
    );
    let (loose, loose_mesh) = mesh_on(&mut scene, None, at_x(7.0));
    make_frame(&mut scene.world, ship).unwrap();
    make_frame(&mut scene.world, room).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = join_for(&device, &queue);
    let out = join.run(&mut scene);

    let table = frame_table(&scene.world, glam::DVec3::ZERO);
    let (leaf_row, leaf_space) = row_model(&join, &out, leaf_mesh);
    assert_eq!(leaf_space, 2, "tagged with its nearest frame (the room)");
    assert_close(
        leaf_row,
        glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::new(1.0, 2.0, 1.0),
            glam::Quat::IDENTITY,
            glam::Vec3::new(3.0, 0.5, -1.0),
        ),
        "the leaf's row is relative to the room, not the world",
    );
    assert_close(
        table[leaf_space as usize] * leaf_row,
        world_matrix_of(&scene.world, leaf).unwrap().as_mat4(),
        "table * row is the leaf's world matrix",
    );
    let (loose_row, loose_space) = row_model(&join, &out, loose_mesh);
    assert_eq!(loose_space, 0, "an unparented object is in the world");
    assert_close(
        loose_row,
        world_matrix_of(&scene.world, loose).unwrap().as_mat4(),
        "loose",
    );

    // The ship moves and turns: only the table changes.
    {
        let mut t = scene.world.get_mut::<Transform>(ship).unwrap();
        t.position = [250.0, 10.0, -40.0];
        t.rotation = [0.0, 75.0, 0.0];
    }
    let out = join.run(&mut scene);
    let (after_row, after_space) = row_model(&join, &out, leaf_mesh);
    assert_eq!(after_space, 2);
    assert_close(
        after_row,
        leaf_row,
        "a frame moving rewrites none of its members' rows",
    );
    let table = frame_table(&scene.world, glam::DVec3::ZERO);
    assert_close(
        table[after_space as usize] * after_row,
        world_matrix_of(&scene.world, leaf).unwrap().as_mat4(),
        "table * row follows the moved ship",
    );
    let _ = loose;
}

/// A fixed (Static) mesh inside a rigid frame is fixed only relative to the
/// frame, which moves: the join marks it movable so world-keyed caches (static
/// shadow casters, ray-tracing instances) do not go stale. Outside a frame it
/// stays static.
#[test]
fn static_meshes_inside_a_frame_are_movable_in_the_world() {
    use engine_backend::scene::frames::make_frame;
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    const MOVABLE: u32 = 1 << 3;
    let mut scene = SceneDb::new();
    let ship = place(&mut scene, "ship", [0.0; 3]);
    let object_with = |scene: &mut SceneDb, parent: Option<Entity>| {
        let object = scene
            .world
            .spawn_object(SpawnObject::new("o").with_parent(parent))
            .unwrap();
        pulsar_world_registry::attach_value(
            &mut scene.world,
            object,
            mesh(ObjectMovability::Static),
        )
        .unwrap()
    };
    let inside = object_with(&mut scene, Some(ship));
    let outside = object_with(&mut scene, None);
    make_frame(&mut scene.world, ship).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = join_for(&device, &queue);
    let out = join.run(&mut scene);
    assert_ne!(
        join.mesh_rows(&out, inside)[0].flags & MOVABLE,
        0,
        "static in a frame"
    );
    assert_eq!(
        join.mesh_rows(&out, outside)[0].flags & MOVABLE,
        0,
        "static in the world"
    );
}

/// Parent and frame rows are keyed by entity index. An index freed by a
/// despawn and reused by an unparented object must not inherit the old parent
/// (or frame) from a row the despawn left behind.
#[test]
fn a_reused_entity_index_does_not_inherit_a_stale_parent() {
    use engine_backend::scene::frames::make_frame;
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let ship = place(&mut scene, "ship", [100.0, 0.0, 0.0]);
    make_frame(&mut scene.world, ship).unwrap();
    // Children of the frame, so their indices carry parent rows.
    let doomed: Vec<Entity> = (0..4)
        .map(|n| {
            scene
                .world
                .spawn_object(SpawnObject::new(format!("d{n}")).with_parent(Some(ship)))
                .unwrap()
        })
        .collect();
    let (_, keeper) = mesh_on(&mut scene, None, at_x(7.0));
    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = join_for(&device, &queue);
    join.run(&mut scene);

    let freed: Vec<u32> = doomed.iter().map(|e| e.index()).collect();
    for entity in doomed {
        scene.world.despawn_tree(entity);
    }
    join.run(&mut scene);

    // New unparented meshes, until one lands on a freed index.
    let mut reused = None;
    for n in 0..16 {
        let (object, instance) = mesh_on(&mut scene, None, at_x(20.0 + n as f64));
        if freed.contains(&object.index()) {
            reused = Some((instance, 20.0 + n as f64));
            break;
        }
    }
    let (instance, x) = reused.expect("a spawned object reused a freed index");
    let out = join.run(&mut scene);
    assert_eq!(
        position_of(&join, &out, instance),
        [x as f32, 0.0, 0.0],
        "placed in the world, not under the despawned object's parent"
    );
    let (_, space) = row_model(&join, &out, instance);
    assert_eq!(space, 0, "and not in the old parent's frame");
    assert_eq!(position_of(&join, &out, keeper), [7.0, 0.0, 0.0]);
}
