//! The meshes' and lights' scene joins on the GPU: handcrafted input rows
//! in, object/material/light/billboard rows out, compared against the CPU math the join
//! replaces (`StaticObjectComponent::new` for meshes, the owner's rotation
//! of -Y for lights). Needs a GPU adapter (lavapipe works); skips without.

use helio_mesh_host::StaticObjectComponent;
use std::sync::Arc;

use helio_pass_light_cull::scene_rows::{LightJoin, LightJoinKeys};
use helio_pass_object_batch::scene_join::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
    SceneDerivationOutput,
};
use helio_pass_object_batch::scene_join::{SceneJoin, SceneJoinKeys};

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let limits = helio::required_wgpu_limits(adapter.limits());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: limits,
        ..Default::default()
    }))
    .ok()?;
    Some((Arc::new(device), Arc::new(queue)))
}

const KEYS: SceneJoinKeys = SceneJoinKeys {
    owners: BufferKey::of("t_owners"),
    generations: BufferKey::of("t_generations"),
    hidden: BufferKey::of("t_hidden"),
    transforms: BufferKey::of("t_transforms"),
    vertex_handles: BufferKey::of("t_vertex_handles"),
    index_handles: BufferKey::of("t_index_handles"),
    mesh_bounds: BufferKey::of("t_bounds"),
    mesh_flags: BufferKey::of("t_flags"),
    section_handles: BufferKey::of("t_section_handles"),
    skin_handles: BufferKey::of("t_skin_handles"),
    palette_handles: BufferKey::of("t_palette_handles"),
    mesh_sections: BufferKey::of("t_sections"),
};

const LIGHT_KEYS: LightJoinKeys = LightJoinKeys {
    owners: KEYS.owners,
    generations: KEYS.generations,
    hidden: KEYS.hidden,
    transforms: KEYS.transforms,
    light_sources: BufferKey::of("t_lights"),
};

/// The meshes' join and the lights' (`billboards`: the lights' billboards
/// too).
struct Joins {
    meshes: SceneJoin,
    lights: LightJoin,
}

fn joins(device: &wgpu::Device, billboards: bool) -> Joins {
    Joins {
        meshes: SceneJoin::new(device, KEYS),
        lights: LightJoin::new(device, LIGHT_KEYS, billboards),
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Owner {
    index: u32,
    generation: u32,
    enabled: u32,
}

/// The frontend's `TransformRow`: the f64 position as an f32 pair.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Transform {
    position_hi: [f32; 3],
    position_lo: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
}

impl Transform {
    /// At `position`, split as the frontend splits it: the nearest f32,
    /// then what that drops.
    fn at(position: [f64; 3], rotation: [f32; 3], scale: [f32; 3]) -> Self {
        let p = glam::DVec3::from_array(position);
        let hi = p.as_vec3();
        let lo = (p - hi.as_dvec3()).as_vec3();
        Self {
            position_hi: hi.to_array(),
            position_lo: lo.to_array(),
            rotation,
            scale,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Section {
    first_index: u32,
    index_count: u32,
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    pad: [u32; 3],
    material: helio::GpuMaterial,
}

struct Scene {
    buffers: Vec<(BufferKey, Vec<u8>, u64)>,
}

impl Scene {
    fn put<T: bytemuck::Pod>(&mut self, key: BufferKey, rows: &[T]) {
        self.buffers.retain(|(k, ..)| *k != key);
        self.buffers.push((
            key,
            bytemuck::cast_slice(rows).to_vec(),
            std::mem::size_of::<T>() as u64,
        ));
    }

    fn projection(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        generation: u64,
    ) -> SceneBufferProjection {
        SceneBufferProjection::from_handles(self.buffers.iter().map(|(key, bytes, row_bytes)| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (bytes.len() as u64).max(16),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buffer, 0, bytes);
            (
                *key,
                BufferHandle {
                    buffer,
                    epoch: 0,
                    row_bytes: *row_bytes,
                    content_generation: generation,
                },
            )
        }))
    }
}

fn read<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
) -> Vec<T> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: buffer.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let out = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
    staging.unmap();
    out
}

/// Run the join once over `scene`; returns the published buffers.
fn run(
    join: &mut Joins,
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    scene: &Scene,
    generation: u64,
) -> SceneBufferProjection {
    run_at(join, device, queue, scene, generation, glam::DVec3::ZERO)
}

/// [`run`] in a frame with `world_origin`.
fn run_at(
    join: &mut Joins,
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    scene: &Scene,
    generation: u64,
    world_origin: glam::DVec3,
) -> SceneBufferProjection {
    let mut projection = scene.projection(device, queue, generation);
    let mut encoder = device.create_command_encoder(&Default::default());
    let context = SceneDerivationContext {
        device,
        queue,
        inputs: &projection,
        world_origin,
    };
    let mut buffers = join.meshes.derive(&context, &mut encoder).buffers;
    buffers.extend(join.lights.derive(&context, &mut encoder).buffers);
    queue.submit([encoder.finish()]);
    for (key, handle) in buffers {
        projection.insert(key, handle);
    }
    projection
}

fn material(base: f32) -> helio::GpuMaterial {
    helio::GpuMaterial {
        base_color: [base, 0.5, 0.25, 1.0],
        emissive: [0.0; 4],
        roughness_metallic: [0.5, 0.0, 1.5, 0.5],
        tex_base_color: u32::MAX,
        tex_normal: u32::MAX,
        tex_roughness: u32::MAX,
        tex_emissive: u32::MAX,
        tex_occlusion: u32::MAX,
        workflow: 0,
        flags: 0,
        material_class: 0,
        class_params: [0.0; 4],
    }
}

fn model_of(t: &Transform) -> glam::Mat4 {
    glam::Mat4::from_scale_rotation_translation(
        glam::Vec3::from_array(t.scale),
        glam::Quat::from_euler(
            glam::EulerRot::YXZ,
            t.rotation[1].to_radians(),
            t.rotation[0].to_radians(),
            t.rotation[2].to_radians(),
        ),
        glam::Vec3::from_array(t.position_hi) + glam::Vec3::from_array(t.position_lo),
    )
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

/// Entities: 0 = object (transform, generation 3), 1 = mesh instance of 0
/// with two sections, 2 = light instance of 0, 3 = a disabled mesh
/// instance of 0.
fn base_scene() -> Scene {
    let mut scene = Scene {
        buffers: Vec::new(),
    };
    let transform = Transform::at([1.0, 2.0, 3.0], [30.0, 45.0, 10.0], [2.0, 1.0, 0.5]);
    scene.put(
        KEYS.transforms,
        &[
            transform,
            Transform::zeroed_row(),
            Transform::zeroed_row(),
            Transform::zeroed_row(),
        ],
    );
    scene.put(KEYS.generations, &[3u32, 5, 7, 9]);
    scene.put(KEYS.hidden, &[0u32, 0, 0, 0]);
    let owner = |enabled| Owner {
        index: 0,
        generation: 3,
        enabled,
    };
    scene.put(
        KEYS.owners,
        &[
            Owner {
                index: 0,
                generation: 0,
                enabled: 0,
            },
            owner(1),
            owner(1),
            owner(0),
        ],
    );
    // Instances 1 and 3: vertices at 100.., indices at 200.., sections at
    // pool slots 0..2 and 2..3.
    scene.put(
        KEYS.vertex_handles,
        &[[0u32, 0], [100, 24], [0, 0], [100, 24]],
    );
    scene.put(
        KEYS.index_handles,
        &[[0u32, 0], [200, 36], [0, 0], [200, 36]],
    );
    scene.put(
        KEYS.mesh_bounds,
        &[
            [0.0f32; 4],
            [0.5, 0.0, 0.0, 1.5],
            [0.0; 4],
            [0.0, 0.0, 0.0, 1.0],
        ],
    );
    scene.put(
        KEYS.mesh_flags,
        &[0u32, helio_mesh_api::INSTANCE_FLAG_MOVABLE, 0, 0],
    );
    scene.put(KEYS.section_handles, &[[0u32, 0], [0, 2], [0, 0], [2, 1]]);
    let section = |first_index, index_count, base| Section {
        first_index,
        index_count,
        material_class: 0,
        graph_hash_lo: 0,
        graph_hash_hi: 0,
        pad: [0; 3],
        material: material(base),
    };
    scene.put(
        KEYS.mesh_sections,
        &[
            section(0, 12, 0.1),
            section(12, 24, 0.2),
            section(0, 36, 0.3),
        ],
    );
    let mut light = helio_light_api::GpuLight {
        position_range: [0.0, 0.0, 0.0, 10.0],
        direction_outer: [0.0, -1.0, 0.0, 0.5],
        color_intensity: [1.0, 0.5, 0.25, 100.0],
        ..Default::default()
    };
    // Enabled (bit 31) plus a shadow policy (priority 1.0, in sixteenths).
    light._pad = (1 << 31) | (16 << 8);
    scene.put(
        LIGHT_KEYS.light_sources,
        &[
            helio_light_api::GpuLight::default(),
            helio_light_api::GpuLight::default(),
            light,
            helio_light_api::GpuLight::default(),
        ],
    );
    scene
}

trait ZeroRow {
    fn zeroed_row() -> Self;
}

impl ZeroRow for Transform {
    fn zeroed_row() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

#[test]
fn placed_instances_are_joined_with_their_owner_and_nothing_else_is() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut join = joins(&device, true);
    let scene = base_scene();
    let out = run(&mut join, &device, &queue, &scene, 1);

    let objects: Vec<helio_mesh_host::StaticObjectComponent> = read(
        &device,
        &queue,
        &out.get(BufferKey::of("static_objects")).unwrap().buffer,
    );
    let materials: Vec<helio::GpuMaterial> = read(
        &device,
        &queue,
        &out.get(BufferKey::of("materials")).unwrap().buffer,
    );
    let transform = Transform::at([1.0, 2.0, 3.0], [30.0, 45.0, 10.0], [2.0, 1.0, 0.5]);
    let model = model_of(&transform);
    let center = model.transform_point3(glam::Vec3::new(0.5, 0.0, 0.0));
    for (slot, (first, count, base)) in [(0u32, (0u32, 12u32, 0.1f32)), (1, (12, 24, 0.2))] {
        let expected = StaticObjectComponent::from(helio_mesh_api::StaticObjectRow::new(
            1,
            5 + 1,
            slot,
            5 + 1,
            model,
            [center.x, center.y, center.z, 1.5 * 2.0],
            count,
            200 + first,
            100,
            0,
            0,
            helio_mesh_api::INSTANCE_FLAG_MOVABLE,
        ));
        let got = objects[slot as usize];
        assert_eq!(
            (
                got.mesh_slot,
                got.mesh_generation,
                got.material_slot,
                got.material_generation
            ),
            (1, 6, slot, 6),
            "section {slot} identity"
        );
        assert_eq!(
            (
                got.index_count,
                got.first_index,
                got.vertex_offset,
                got.flags
            ),
            (
                expected.index_count,
                expected.first_index,
                expected.vertex_offset,
                expected.flags
            ),
            "section {slot} draw range"
        );
        assert!(
            close(
                bytemuck::cast_slice(&got.transform),
                bytemuck::cast_slice(&expected.transform)
            ),
            "model matrix"
        );
        assert!(
            close(
                bytemuck::cast_slice(&got.normal_mat),
                bytemuck::cast_slice(&expected.normal_mat)
            ),
            "normal matrix"
        );
        assert!(
            close(&got.bounds, &expected.bounds),
            "bounds {:?} vs {:?}",
            got.bounds,
            expected.bounds
        );
        assert_eq!(
            materials[slot as usize].base_color[0], base,
            "section {slot} material"
        );
    }
    assert_eq!(
        objects[2].mesh_generation, 0,
        "a disabled instance draws nothing"
    );

    let lights: Vec<helio_light_api::GpuLight> = read(
        &device,
        &queue,
        &out.get(BufferKey::of(helio_light_api::LIGHT_LIST))
            .unwrap()
            .buffer,
    );
    let light = lights[2];
    assert!(close(&light.position_range, &[1.0, 2.0, 3.0, 10.0]));
    let direction = glam::Quat::from_euler(
        glam::EulerRot::YXZ,
        45f32.to_radians(),
        30f32.to_radians(),
        10f32.to_radians(),
    ) * -glam::Vec3::Y;
    assert!(
        close(&light.direction_outer[..3], &direction.to_array()),
        "light direction"
    );
    assert_eq!(
        light._pad,
        16 << 8,
        "the enabled flag does not leak into the output; the shadow policy does"
    );
    assert_eq!(
        lights[1].color_intensity, [0.0; 4],
        "rows without a light stay dark"
    );
    let billboards: Vec<[f32; 12]> = read(
        &device,
        &queue,
        &out.get(BufferKey::of("billboard_instances"))
            .unwrap()
            .buffer,
    );
    assert!(
        close(&billboards[2][..3], &[1.0, 2.0, 3.0]),
        "billboard at the light"
    );
}

/// Positions come out relative to the frame's world origin, exactly: at
/// Earth radius an f32 world position steps by 0.5 m, so a world-space model
/// matrix would snap every vertex to that grid (Pulsar-Native#1127). A
/// light's billboard is in the frame's space with it.
#[test]
fn camera_relative_frames_get_positions_relative_to_the_world_origin() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = base_scene();
    // On a planet: 6_371_002.371 is no f32 (0.5 m steps up there), nor is
    // the origin.
    let far_position = [1.0, 6_371_002.371, 3.0];
    let far = Transform::at(far_position, [30.0, 45.0, 10.0], [2.0, 1.0, 0.5]);
    scene.put(
        KEYS.transforms,
        &[
            far,
            Transform::zeroed_row(),
            Transform::zeroed_row(),
            Transform::zeroed_row(),
        ],
    );
    let origin = glam::DVec3::new(0.75, 6_371_000.125, -2.0);
    let mut join = joins(&device, true);
    let out = run_at(&mut join, &device, &queue, &scene, 1, origin);

    // The f64 offset to the f32 rounding of a small number (a millimetre's
    // millionths), where an f32 world position would be 0.5 m out.
    let offset = (glam::DVec3::from_array(far_position) - origin).as_vec3();
    assert!(
        (offset - glam::Vec3::new(0.25, 2.246, 5.0))
            .abs()
            .max_element()
            < 1e-6,
        "{offset}"
    );
    let objects: Vec<helio_mesh_host::StaticObjectComponent> = read(
        &device,
        &queue,
        &out.get(BufferKey::of("static_objects")).unwrap().buffer,
    );
    let model = glam::Mat4::from_cols_array_2d(&objects[0].transform);
    assert!(
        (model.w_axis.truncate() - offset).abs().max_element() < 1e-6,
        "model translation {}",
        model.w_axis
    );
    let relative = Transform::at(offset.as_dvec3().to_array(), far.rotation, far.scale);
    let center = model_of(&relative).transform_point3(glam::Vec3::new(0.5, 0.0, 0.0));
    assert!(
        close(&objects[0].bounds[..3], &center.to_array()),
        "bounds centre"
    );

    let lights: Vec<helio_light_api::GpuLight> = read(
        &device,
        &queue,
        &out.get(BufferKey::of(helio_light_api::LIGHT_LIST))
            .unwrap()
            .buffer,
    );
    assert!(
        (glam::Vec3::from_slice(&lights[2].position_range[..3]) - offset)
            .abs()
            .max_element()
            < 1e-6,
        "light position"
    );
    let billboards: Vec<[f32; 12]> = read(
        &device,
        &queue,
        &out.get(BufferKey::of("billboard_instances"))
            .unwrap()
            .buffer,
    );
    assert!(
        (glam::Vec3::from_slice(&billboards[2][..3]) - offset)
            .abs()
            .max_element()
            < 1e-6,
        "billboard position"
    );

    // A moved origin moves every position, though no input row changed.
    let moved = origin + glam::DVec3::new(0.0, 1.0, 0.0);
    let out = run_at(&mut join, &device, &queue, &scene, 1, moved);
    let objects: Vec<helio_mesh_host::StaticObjectComponent> = read(
        &device,
        &queue,
        &out.get(BufferKey::of("static_objects")).unwrap().buffer,
    );
    let model = glam::Mat4::from_cols_array_2d(&objects[0].transform);
    assert!(
        (model.w_axis.y - (offset.y - 1.0)).abs() < 1e-6,
        "{}",
        model.w_axis.y
    );
}

#[test]
fn hiding_the_owner_a_stale_generation_or_a_disabled_light_removes_the_rows() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut join = joins(&device, false);
    let mut scene = base_scene();
    scene.put(KEYS.hidden, &[1u32, 0, 0, 0]);
    let out = run(&mut join, &device, &queue, &scene, 1);
    let objects: Vec<helio_mesh_host::StaticObjectComponent> = read(
        &device,
        &queue,
        &out.get(BufferKey::of("static_objects")).unwrap().buffer,
    );
    assert!(
        objects.iter().all(|o| o.mesh_generation == 0),
        "hidden owner"
    );
    let lights: Vec<helio_light_api::GpuLight> = read(
        &device,
        &queue,
        &out.get(BufferKey::of(helio_light_api::LIGHT_LIST))
            .unwrap()
            .buffer,
    );
    assert!(
        lights.iter().all(|l| l.color_intensity[3] == 0.0),
        "hidden owner"
    );
    assert!(
        out.get(BufferKey::of("billboard_instances")).is_none(),
        "billboards off"
    );

    let mut scene = base_scene();
    scene.put(KEYS.generations, &[4u32, 5, 7, 9]);
    let out = run(&mut join, &device, &queue, &scene, 2);
    let objects: Vec<helio_mesh_host::StaticObjectComponent> = read(
        &device,
        &queue,
        &out.get(BufferKey::of("static_objects")).unwrap().buffer,
    );
    assert!(
        objects.iter().all(|o| o.mesh_generation == 0),
        "stale owner generation"
    );

    let mut scene = base_scene();
    let mut light = helio_light_api::GpuLight {
        color_intensity: [1.0; 4],
        ..Default::default()
    };
    light._pad = 0;
    scene.put(LIGHT_KEYS.light_sources, &[light, light, light, light]);
    let out = run(&mut join, &device, &queue, &scene, 3);
    let lights: Vec<helio_light_api::GpuLight> = read(
        &device,
        &queue,
        &out.get(BufferKey::of(helio_light_api::LIGHT_LIST))
            .unwrap()
            .buffer,
    );
    assert!(
        lights.iter().all(|l| l.color_intensity[3] == 0.0),
        "disabled light"
    );
}

#[test]
fn unchanged_inputs_record_nothing_and_keep_the_outputs() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut join = SceneJoin::new(&device, KEYS);
    let scene = base_scene();
    let projection = scene.projection(&device, &queue, 7);
    let mut derive = || {
        let mut encoder = device.create_command_encoder(&Default::default());
        let output = join.derive(
            &SceneDerivationContext {
                device: &device,
                queue: &queue,
                inputs: &projection,
                world_origin: glam::DVec3::ZERO,
            },
            &mut encoder,
        );
        queue.submit([encoder.finish()]);
        output
    };
    let first = derive();
    assert!(first.recorded);
    let second = derive();
    assert!(!second.recorded, "same inputs: nothing to record");
    let generation = |output: &SceneDerivationOutput| {
        output
            .buffers
            .iter()
            .find(|(key, _)| *key == BufferKey::of("static_objects"))
            .map(|(_, h)| (h.epoch, h.content_generation))
    };
    assert_eq!(generation(&first), generation(&second), "outputs unchanged");
}

/// Moving objects must not read as a change to what the objects are. The
/// object batch re-sorts the whole scene whenever the materials or the
/// structure marker move, and the transform column moves every frame an
/// object is dragged.
#[test]
fn moving_objects_moves_the_objects_but_not_the_structure() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let scene = base_scene();
    let mut join = joins(&device, false);
    let generation = |projection: &SceneBufferProjection, key: &'static str| {
        projection
            .get(BufferKey::of(key))
            .unwrap()
            .content_generation
    };
    let first = run(&mut join, &device, &queue, &scene, 1);
    let (objects, materials, structure) = (
        generation(&first, "static_objects"),
        generation(&first, "materials"),
        generation(&first, "scene_objects_structure"),
    );

    // The same scene, the transform column alone reporting new contents.
    let derive = |join: &mut Joins, mut projection: SceneBufferProjection| {
        let mut encoder = device.create_command_encoder(&Default::default());
        let output = join.meshes.derive(
            &SceneDerivationContext {
                device: &device,
                queue: &queue,
                inputs: &projection,
                world_origin: glam::DVec3::ZERO,
            },
            &mut encoder,
        );
        queue.submit([encoder.finish()]);
        for (key, handle) in output.buffers {
            projection.insert(key, handle);
        }
        projection
    };
    let bump = |key: BufferKey| {
        let mut projection = scene.projection(&device, &queue, 1);
        let handle = projection.get(key).unwrap().clone();
        projection.insert(
            key,
            BufferHandle {
                content_generation: 2,
                ..handle
            },
        );
        projection
    };
    // Same buffers as the first run would be ideal, but the harness makes
    // fresh ones per projection; what matters is the join's own signature of
    // them, so the unchanged columns keep generation 1 and the same size.
    let moved = derive(&mut join, bump(KEYS.transforms));
    assert!(
        generation(&moved, "static_objects") > objects,
        "the objects hold placements, so they changed"
    );
    assert_eq!(
        generation(&moved, "materials"),
        materials,
        "materials follow the structure"
    );
    assert_eq!(
        generation(&moved, "scene_objects_structure"),
        structure,
        "moving objects is not a structural change"
    );

    // And a change to what an object is does move all three.
    let retargeted = derive(&mut join, bump(KEYS.mesh_flags));
    assert!(generation(&retargeted, "static_objects") > generation(&moved, "static_objects"));
    assert!(generation(&retargeted, "materials") > materials);
    assert!(generation(&retargeted, "scene_objects_structure") > structure);
}
