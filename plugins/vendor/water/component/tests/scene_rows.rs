//! The water plugin's join over the engine's rows: water volumes, the
//! lights' directional light, the foliage plugin's global wind and the
//! bodies physics marks as pushing water reach the rows the water passes
//! read. Placement and gating are read back from the join's outputs. Needs
//! a GPU adapter (lavapipe works); skips without one.

// The join harness is the engine's; not every test here uses every helper.
#![allow(dead_code)]

use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility, attachments};
use helio_core::ENTITY_GENERATIONS_KEY;
use helio_scene_join::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
};
use pulsar_scenedb::{Entity, SceneDb};
use water_component::WaterVolumeComponent;

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

type WaterRow = helio_pass_water_sim::GpuWaterVolume;

fn close(a: [f32; 4], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
}

#[test]
fn water_volumes_are_packed_into_the_leading_rows() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    // Entities ahead of the water, so its instance rows sit well past the
    // eight rows the water passes read.
    for i in 0..12 {
        place(&mut scene, &format!("filler {i}"), at([0.0; 3]));
    }
    let water = |surface: f32| {
        let mut water = WaterVolumeComponent::default();
        water.size = [10.0, 4.0, 20.0];
        water.surface_height_offset = surface;
        water
    };
    let lake = place(&mut scene, "lake", at([0.0, 5.0, 0.0]));
    let lake = pulsar_world_registry::attach_value(&mut scene.world, lake, water(1.0)).unwrap();
    let pond = place(
        &mut scene,
        "pond",
        Transform {
            position: [100.0, 0.0, 0.0],
            rotation: [0.0; 3],
            scale: [2.0, 2.0, 2.0],
        },
    );
    let pond_instance =
        pulsar_world_registry::attach_value(&mut scene.world, pond, water(0.5)).unwrap();
    assert!(lake.index() >= helio_pass_water_sim::scene_rows::MAX_WATER_VOLUMES);

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };

    let out = join.run(&mut scene);
    let rows: Vec<WaterRow> = join.read(&out, "water_volumes");
    assert_eq!(
        rows.len(),
        helio_pass_water_sim::scene_rows::MAX_WATER_VOLUMES as usize
    );
    let close3 = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3);
    assert!(
        close3(rows[0].bounds_min, [-5.0, 3.0, -10.0, 0.0])
            && close3(rows[0].bounds_max, [5.0, 7.0, 10.0, 6.0]),
        "the first volume takes row 0, its surface at the owner's Y plus the offset: {:?} {:?}",
        rows[0].bounds_min,
        rows[0].bounds_max
    );
    assert!(
        close3(rows[1].bounds_min, [90.0, -4.0, -20.0, 0.0])
            && close3(rows[1].bounds_max, [110.0, 4.0, 20.0, 1.0]),
        "the second takes row 1, scaled with its owner: {:?} {:?}",
        rows[1].bounds_min,
        rows[1].bounds_max
    );
    assert_eq!(
        rows[0].wave_params,
        WaterVolumeComponent::default().local_gpu().wave_params
    );
    assert!(rows[2..].iter().all(|row| row.bounds_max == [0.0; 4]));

    // A disabled or hidden volume gives up its row; the next one moves up.
    attachments::set_enabled(&mut scene.world, lake, false);
    let out = join.run(&mut scene);
    let rows: Vec<WaterRow> = join.read(&out, "water_volumes");
    assert!(
        close3(rows[0].bounds_min, [90.0, -4.0, -20.0, 0.0]),
        "{:?}",
        rows[0].bounds_min
    );
    assert_eq!(rows[1].bounds_max, [0.0; 4]);
    attachments::set_enabled(&mut scene.world, lake, true);
    scene.world.get_mut::<Visibility>(pond).unwrap().visible = false;
    scene
        .world
        .get_mut::<WaterVolumeComponent>(lake)
        .unwrap()
        .enabled = false;
    let out = join.run(&mut scene);
    let rows: Vec<WaterRow> = join.read(&out, "water_volumes");
    assert!(
        rows.iter().all(|row| row.bounds_max == [0.0; 4]),
        "nothing placed"
    );

    scene.world.get_mut::<Visibility>(pond).unwrap().visible = true;
    attachments::detach(&mut scene.world, pond_instance);
    let out = join.run(&mut scene);
    let rows: Vec<WaterRow> = join.read(&out, "water_volumes");
    assert!(
        rows.iter().all(|row| row.bounds_max == [0.0; 4]),
        "detached"
    );
}

/// Water (#1065): every placed volume takes the lights' directional light
/// (oriented by its owner) as its sun, in place of an authored one,
/// and the level's global wind unless it opts out; its own spring, damping
/// and wave scale reach its row.
#[test]
fn water_takes_the_scenes_sun_and_the_global_wind() {
    use foliage_component::WindComponent;
    use lights_component::{LightComponent, LightType};
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let water = |use_global_wind: bool| WaterVolumeComponent {
        size: [10.0, 4.0, 10.0],
        wave_spring: 1.4,
        wave_damping: 0.95,
        wave_scale: 2.0,
        use_global_wind,
        wind_direction_x: 0.0,
        wind_direction_z: 1.0,
        wind_strength: 3.0,
        ..Default::default()
    };
    let lake = place(&mut scene, "lake", at([0.0; 3]));
    pulsar_world_registry::attach_value(&mut scene.world, lake, water(true)).unwrap();
    let pond = place(&mut scene, "pond", at([50.0, 0.0, 0.0]));
    let pond = pulsar_world_registry::attach_value(&mut scene.world, pond, water(false)).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut scene_join: Box<dyn SceneDerivation> =
        engine_backend::scene::scene_join(&device, false);
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };
    let water_rows =
        |join: &mut Join, scene: &mut SceneDb, scene_join: &mut Box<dyn SceneDerivation>| {
            let out = join.run_after(scene, Some(scene_join));
            let rows: Vec<WaterRow> = join.read(&out, "water_volumes");
            (rows, out)
        };

    let (rows, _) = water_rows(&mut join, &mut scene, &mut scene_join);
    assert_eq!(
        rows[0].sim_dynamics,
        [1.4, 0.95, 2.0, 0.0],
        "its own dynamics"
    );
    assert_eq!(
        rows[0].sun_direction,
        [0.0, 1.0, 0.0, 0.0],
        "no sun: straight up"
    );
    assert_eq!(
        rows[0].wind_params,
        [0.0, 1.0, 3.0, 0.0],
        "no global wind: its own"
    );
    assert_eq!(rows[1].wind_params, [0.0, 1.0, 3.0, 1.0]);

    // A directional light, tilted by its owner, is the water's sun.
    let sun = place(
        &mut scene,
        "sun",
        Transform {
            rotation: [-60.0, 30.0, 0.0],
            ..Transform::default()
        },
    );
    let mut light = LightComponent::default();
    light.general.enabled = true;
    light.general.light_type = LightType::Directional;
    let light = pulsar_world_registry::attach_value(&mut scene.world, sun, light).unwrap();
    let (rows, out) = water_rows(&mut join, &mut scene, &mut scene_join);
    let lit: helio_light_api::GpuLight = join
        .read::<helio_light_api::GpuLight>(&out, helio_light_api::LIGHT_LIST)
        [light.index() as usize];
    let toward = -glam::Vec3::from_slice(&lit.direction_outer[..3]).normalize();
    assert!(
        toward.y > 0.1 && toward.x.abs() > 0.1,
        "a tilted sun: {toward:?}"
    );
    for row in &rows[..2] {
        assert!(
            close(row.sun_direction, toward.to_array()) && row.sun_direction[3] == 1.0,
            "the water's sun is the directional light: {:?} vs {toward:?}",
            row.sun_direction
        );
    }

    // The global wind blows over the lake, not over the pond that opts out.
    let breeze = place(&mut scene, "wind", at([0.0; 3]));
    let wind = WindComponent {
        direction: [3.0, 0.0, 0.0],
        speed: 6.0,
        ..Default::default()
    };
    let wind = pulsar_world_registry::attach_value(&mut scene.world, breeze, wind).unwrap();
    let (rows, _) = water_rows(&mut join, &mut scene, &mut scene_join);
    let strength = 6.0 * helio_pass_water_sim::scene_rows::WATER_WIND_STRENGTH_PER_SPEED;
    assert_eq!(
        rows[0].wind_params,
        [1.0, 0.0, strength, 0.0],
        "the global wind"
    );
    assert_eq!(rows[1].wind_params, [0.0, 1.0, 3.0, 1.0], "its own wind");
    assert!(
        rows[2..].iter().all(|row| row.wind_params == [0.0; 4]),
        "unplaced rows stay zero"
    );

    // A calm global wind calms the lake; without one it has its own again.
    scene.world.get_mut::<WindComponent>(wind).unwrap().speed = 0.0;
    let (rows, _) = water_rows(&mut join, &mut scene, &mut scene_join);
    assert_eq!(rows[0].wind_params[2], 0.0, "a calm global wind");
    attachments::set_enabled(&mut scene.world, wind, false);
    let (rows, _) = water_rows(&mut join, &mut scene, &mut scene_join);
    assert_eq!(
        rows[0].wind_params,
        [0.0, 1.0, 3.0, 0.0],
        "a disabled global wind"
    );

    // The pond's own wind follows its edits; switching the light off leaves
    // the water without a sun.
    scene
        .world
        .get_mut::<WaterVolumeComponent>(pond)
        .unwrap()
        .wind_strength = 7.0;
    attachments::set_enabled(&mut scene.world, light, false);
    let (rows, _) = water_rows(&mut join, &mut scene, &mut scene_join);
    assert_eq!(rows[1].wind_params[2], 7.0);
    assert_eq!(rows[0].sun_direction, [0.0, 1.0, 0.0, 0.0]);
}

/// Water interaction (#1080): a body on the `WaterSim` collision channel
/// becomes a water hitbox bounded by its owner's meshes at the owner's
/// transform; a body off the channel, or one with no mesh, does not. Its
/// old bounds are an empty box at its centre (the simulation tracks where
/// it was) and its row carries its identity.
#[test]
fn bodies_on_the_water_sim_channel_become_water_hitboxes() {
    use helio_component::components::StaticMeshComponent;
    use pulsar_physics::{CollisionChannel, PhysicsComponent};
    type HitboxRow = helio_pass_water_sim::GpuWaterHitbox;
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mesh = StaticMeshComponent {
        indices: vec![0, 1, 2],
        bounds_local: [0.0, 0.5, 0.0, 1.0],
        ..Default::default()
    };
    let body = |water: bool| {
        let mut physics = PhysicsComponent::default();
        if water {
            physics.collision.collision_channel |= u64::from(CollisionChannel::WaterSim);
        }
        physics
    };
    let mut scene = SceneDb::new();
    // Off the channel, with a mesh: no hitbox.
    let crate_box = place(&mut scene, "crate", at([-10.0, 0.0, 0.0]));
    pulsar_world_registry::attach_value(&mut scene.world, crate_box, mesh.clone()).unwrap();
    pulsar_world_registry::attach_value(&mut scene.world, crate_box, body(false)).unwrap();
    // On the channel, with no mesh: nothing to bound.
    let ghost = place(&mut scene, "ghost", at([20.0, 0.0, 0.0]));
    pulsar_world_registry::attach_value(&mut scene.world, ghost, body(true)).unwrap();
    // On the channel, with a mesh, scaled 2x.
    let boat = place(
        &mut scene,
        "boat",
        Transform {
            position: [4.0, 1.0, -2.0],
            rotation: [0.0; 3],
            scale: [2.0, 2.0, 2.0],
        },
    );
    pulsar_world_registry::attach_value(&mut scene.world, boat, mesh.clone()).unwrap();
    let hull = pulsar_world_registry::attach_value(&mut scene.world, boat, body(true)).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };
    let close3 = |a: [f32; 4], b: [f32; 3]| a[..3].iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4);

    let out = join.run(&mut scene);
    let rows: Vec<HitboxRow> = join.read(&out, "water_hitboxes");
    assert_eq!(
        rows.len(),
        helio_pass_water_sim::MAX_WATER_HITBOXES as usize
    );
    // The mesh's sphere (centre 1 above the owner after scaling, radius 2).
    assert!(
        close3(rows[0].new_min, [2.0, 0.0, -4.0]) && close3(rows[0].new_max, [6.0, 4.0, 0.0]),
        "the boat is bounded by its mesh at its owner: {:?} {:?}",
        rows[0].new_min,
        rows[0].new_max
    );
    assert!(close3(rows[0].old_min, [4.0, 2.0, -2.0]) && rows[0].old_min == rows[0].old_max);
    assert_eq!(
        rows[0].params,
        [
            pulsar_physics::WATER_HITBOX_EDGE_SOFTNESS,
            pulsar_physics::WATER_HITBOX_STRENGTH,
            (hull.index() + 1) as f32,
            0.0
        ]
    );
    assert!(
        rows[1..].iter().all(|row| row.params == [0.0; 4]),
        "no hitbox off the channel or without a mesh"
    );

    // The hitbox follows its owner; a hidden body still pushes water.
    scene.world.get_mut::<Transform>(boat).unwrap().position = [4.0, -1.0, -2.0];
    scene.world.get_mut::<Visibility>(boat).unwrap().visible = false;
    let out = join.run(&mut scene);
    let rows: Vec<HitboxRow> = join.read(&out, "water_hitboxes");
    assert!(
        close3(rows[0].new_min, [2.0, -2.0, -4.0]),
        "{:?}",
        rows[0].new_min
    );

    // Off the channel, it stops.
    scene
        .world
        .get_mut::<PhysicsComponent>(hull)
        .unwrap()
        .collision
        .collision_channel = 0;
    let out = join.run(&mut scene);
    let rows: Vec<HitboxRow> = join.read(&out, "water_hitboxes");
    assert!(
        rows.iter().all(|row| row.params == [0.0; 4]),
        "off the channel"
    );
}
