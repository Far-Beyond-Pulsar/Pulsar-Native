//! Helio's environment join over this engine's own rows (Pulsar-Native#1035,
//! Phase 4): fog volumes, post-process volumes and camera post-process
//! components, authored as component instances, reach the rows the
//! volumetric fog and post-process passes read. Placement and gating are
//! read back from the join's outputs. Needs a GPU adapter (lavapipe works);
//! skips without one.

use std::sync::Arc;

use engine_backend::scene::{attachments, SceneWorldExt, SpawnObject, Transform, Visibility};
use helio_component::components::{
    CameraPostProcessComponent, FoliageComponent, GlobalFogComponent, LocalFogVolumeComponent,
    PostProcessVolumeComponent, WaterVolumeComponent,
};
use helio_default_graphs::environment_join::{
    BufferHandle, BufferKey, SceneBufferProjection, SceneDerivation, SceneDerivationContext,
};
use helio_default_graphs::scene_join::ENTITY_GENERATIONS_KEY;
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

fn at(position: [f32; 3]) -> Transform {
    Transform {
        position,
        ..Transform::default()
    }
}

type GlobalRow = helio_pass_volumetric_fog::GlobalFogComponent;
type LocalRow = helio_pass_volumetric_fog::LocalFogVolumeComponent;
type VolumeRow = helio_pass_postprocess::PostProcessVolumeComponent;
type CameraRow = helio_pass_postprocess::CameraPostProcessComponent;
type WaterRow = helio_pass_water_sim::GpuWaterVolume;

fn close(a: [f32; 4], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
}

#[test]
fn the_environment_join_follows_the_engines_authored_rows() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    // Populated before the mirror attaches: SceneDB replays every row.
    let sky = place(&mut scene, "sky", at([0.0; 3]));
    let mut fog = GlobalFogComponent::default();
    fog.medium.extinction = 0.25;
    let global = pulsar_world_registry::attach_value(&mut scene.world, sky, fog).unwrap();

    // A 2 x 4 x 6 m local fog box on an object scaled by 2 and turned 90°
    // about Y: its world extent is 12 x 8 x 4, centred on the owner.
    let mist = place(
        &mut scene,
        "mist",
        Transform {
            position: [10.0, 0.0, 0.0],
            rotation: [0.0, 90.0, 0.0],
            scale: [2.0, 2.0, 2.0],
        },
    );
    let mut local = LocalFogVolumeComponent::default();
    local.size = [2.0, 4.0, 6.0];
    local.edge_fade = 0.5;
    let local = pulsar_world_registry::attach_value(&mut scene.world, mist, local).unwrap();

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
    let row: GlobalRow = join.row(&out, "global_fog_media", global);
    assert_eq!(row.enabled, 1, "global fog placed");
    assert_eq!(row.extinction, 0.25);

    let row: LocalRow = join.row(&out, "local_fog_media", local);
    assert!(
        close(row.bounds_min, [4.0, -4.0, -2.0]) && close(row.bounds_max, [16.0, 4.0, 2.0]),
        "the owner's scale and rotation place the box: {:?} {:?}",
        row.bounds_min,
        row.bounds_max
    );
    assert_eq!(row.medium().enabled, 1);
    assert_eq!(row.edge_fade, 0.5);

    let row: VolumeRow = join.row(&out, "post_process_volumes", volume);
    assert!(close(row.bounds_min, [-2.0, 3.0, -2.0]) && close(row.bounds_max, [2.0, 7.0, 2.0]));
    assert_eq!((row.blend_weight, row.priority), (0.75, 3.0));

    let row: CameraRow = join.row(&out, "camera_postprocess", camera);
    assert_eq!((row.view_id, row.enabled), (0, 1), "camera baseline placed");

    // Hidden owners turn volumes off; a camera baseline is not visual.
    for object in [sky, mist, grading, camera_object] {
        scene.world.get_mut::<Visibility>(object).unwrap().visible = false;
    }
    let out = join.run(&mut scene);
    assert_eq!(
        join.row::<GlobalRow>(&out, "global_fog_media", global)
            .enabled,
        0
    );
    assert_eq!(
        join.row::<LocalRow>(&out, "local_fog_media", local)
            .medium()
            .enabled,
        0
    );
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
    for object in [sky, mist, grading, camera_object] {
        scene.world.get_mut::<Visibility>(object).unwrap().visible = true;
    }

    // Disabled instances and disabled components write inert rows; moving
    // an owner moves its volume.
    attachments::set_enabled(&mut scene.world, global, false);
    attachments::set_enabled(&mut scene.world, camera, false);
    scene
        .world
        .get_mut::<PostProcessVolumeComponent>(volume)
        .unwrap()
        .enabled = false;
    scene.world.get_mut::<Transform>(mist).unwrap().position = [0.0, 0.0, 0.0];
    let out = join.run(&mut scene);
    assert_eq!(
        join.row::<GlobalRow>(&out, "global_fog_media", global)
            .enabled,
        0
    );
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
    let row: LocalRow = join.row(&out, "local_fog_media", local);
    assert!(
        close(row.bounds_min, [-6.0, -4.0, -2.0]),
        "moved: {:?}",
        row.bounds_min
    );

    // Removed: nothing left.
    attachments::detach(&mut scene.world, local);
    let out = join.run(&mut scene);
    assert_eq!(
        join.row::<LocalRow>(&out, "local_fog_media", local)
            .medium()
            .enabled,
        0
    );
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
    assert!(lake.index() >= helio_default_graphs::environment_join::MAX_WATER_VOLUMES);

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
        helio_default_graphs::environment_join::MAX_WATER_VOLUMES as usize
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

#[test]
fn foliage_types_layers_and_wind_are_packed() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    type TypeRow = helio_pass_foliage_place::components::FoliageTypeComponent;
    type LayerRow = helio_pass_foliage_place::components::FoliageLayerComponent;
    type WindRow = helio_pass_foliage_place::components::FoliageWindComponent;

    let mut scene = SceneDb::new();
    let bystander = place(&mut scene, "bystander", at([0.0; 3]));
    let grass = |density: f32, extent: f32, wind_speed: f32| {
        let mut foliage = FoliageComponent::default();
        foliage.general.enabled = true;
        foliage.general.density = density;
        foliage.placement.layer_extent = extent;
        foliage.placement.altitude_min = -5.0;
        foliage.placement.altitude_max = 50.0;
        foliage.wind.wind_enabled = true;
        foliage.wind.wind_speed = wind_speed;
        foliage
    };
    let meadow = place(&mut scene, "meadow", at([10.0, 3.0, -20.0]));
    let meadow =
        pulsar_world_registry::attach_value(&mut scene.world, meadow, grass(8.0, 25.0, 2.0))
            .unwrap();
    let lawn_owner = place(
        &mut scene,
        "lawn",
        Transform {
            position: [-100.0, 0.0, 0.0],
            rotation: [0.0; 3],
            scale: [2.0, 1.0, 2.0],
        },
    );
    let lawn =
        pulsar_world_registry::attach_value(&mut scene.world, lawn_owner, grass(4.0, 5.0, 7.0))
            .unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };

    let out = join.run(&mut scene);
    let types: Vec<TypeRow> = join.read(&out, "foliage_types");
    assert_eq!(
        types.len(),
        helio_default_graphs::environment_join::MAX_FOLIAGE_TYPES as usize
    );
    assert_eq!(
        (types[0].density, types[1].density, types[2].density),
        (8.0, 4.0, 0.0)
    );
    let layers: Vec<LayerRow> = join.read(&out, "foliage_layers");
    assert_eq!(layers[0].bounds_min, [-15.0, -5.0, -45.0, 0.0]);
    assert_eq!(layers[0].bounds_max, [35.0, 50.0, 5.0, 0.0]);
    assert_eq!(
        (layers[1].bounds_min[0], layers[1].bounds_max[0]),
        (-110.0, -90.0),
        "the lawn's square takes its owner's scale"
    );
    assert_eq!(layers[2].bounds_max, [0.0; 4]);
    let wind: Vec<WindRow> = join.read(&out, "foliage_wind");
    assert_eq!(wind.len(), 1);
    assert_eq!(
        wind[0].direction_speed[3], 2.0,
        "the first component's wind"
    );

    // Moving an unrelated object re-derives nothing the foliage passes key
    // their tiles on.
    let types_generation = out
        .get(BufferKey::of("foliage_types"))
        .unwrap()
        .content_generation;
    scene
        .world
        .get_mut::<Transform>(bystander)
        .unwrap()
        .position = [1.0, 0.0, 0.0];
    let out = join.run(&mut scene);
    assert_eq!(
        out.get(BufferKey::of("foliage_types"))
            .unwrap()
            .content_generation,
        types_generation
    );

    // Disabled or hidden foliage gives up its rows.
    attachments::set_enabled(&mut scene.world, meadow, false);
    let out = join.run(&mut scene);
    let types: Vec<TypeRow> = join.read(&out, "foliage_types");
    assert_eq!((types[0].density, types[1].density), (4.0, 0.0));
    let wind: Vec<WindRow> = join.read(&out, "foliage_wind");
    assert_eq!(wind[0].direction_speed[3], 7.0, "the lawn's wind now");
    scene
        .world
        .get_mut::<Visibility>(lawn_owner)
        .unwrap()
        .visible = false;
    let out = join.run(&mut scene);
    let types: Vec<TypeRow> = join.read(&out, "foliage_types");
    assert!(types.iter().all(|row| row.density == 0.0), "nothing placed");
    let _ = lawn;
}

type AtmosphereRow = helio_pass_sky::AtmosphereComponent;

#[test]
fn atmospheres_follow_their_owner_and_their_enabled_state() {
    use helio_component::components::{AtmosphereComponent, AtmospherePlacement};
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
    let planet = pulsar_world_registry::attach_value(&mut scene.world, world_object, planet).unwrap();
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
    scene.world.get_mut::<Visibility>(world_object).unwrap().visible = false;
    scene.world.get_mut::<Transform>(world_object).unwrap().position = [-1.0, 2.0, -3.0];
    let out = join.run(&mut scene);
    let row: AtmosphereRow = join.row(&out, "atmospheres", planet);
    assert_eq!(row.enabled, 1);
    assert_eq!(row.center, [-1.0, 2.0, -3.0], "follows its owner");

    // A disabled instance or component leaves an inert row; removal clears it.
    attachments::set_enabled(&mut scene.world, planet, false);
    scene.world.get_mut::<AtmosphereComponent>(ground).unwrap().enabled = false;
    let out = join.run(&mut scene);
    assert_eq!(join.row::<AtmosphereRow>(&out, "atmospheres", planet).enabled, 0);
    assert_eq!(join.row::<AtmosphereRow>(&out, "atmospheres", ground).enabled, 0);
    attachments::set_enabled(&mut scene.world, planet, true);
    attachments::detach(&mut scene.world, planet);
    let out = join.run(&mut scene);
    assert_eq!(join.row::<AtmosphereRow>(&out, "atmospheres", planet).enabled, 0);
}

/// Decals (#1058): an enabled decal of a visible owner becomes a decal pass
/// row, packed into the leading rows, whose transform maps the owner-placed
/// box onto -1..1; hiding the owner, disabling the instance or a zero
/// opacity's row (kept, inert) follow.
#[test]
fn decals_are_placed_in_their_owners_box() {
    use helio_component::components::DecalComponent;
    type DecalRow = helio_pass_decal::DecalComponent;
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

/// Particle emitters (#1059): an enabled emitter of a visible owner becomes
/// a Corona emitter row, packed into the leading rows, placed at its owner
/// and given its own range of the shared particle pool, sized by its
/// `max_particles`; the ranges are disjoint and aligned, a full pool clamps
/// the last ones, and hidden owners or disabled instances give theirs up.
#[test]
fn particle_emitters_take_disjoint_ranges_of_the_particle_pool() {
    use helio_component::components::{CoronaEmitterSourceRow, ParticleEmitterComponent};
    use helio_default_graphs::environment_join::{
        CORONA_POOL_PARTICLES, CORONA_RANGE_ALIGNMENT, MAX_CORONA_EMITTERS,
    };
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let emitter = |scene: &mut SceneDb, name: &str, x: f32, max_particles: u32| {
        let owner = place(scene, name, at([x, 2.0, -1.0]));
        let emitter = ParticleEmitterComponent {
            max_particles,
            ..Default::default()
        };
        let instance =
            pulsar_world_registry::attach_value(&mut scene.world, owner, emitter).unwrap();
        (owner, instance)
    };
    let (a, _) = emitter(&mut scene, "a", 1.0, 300);
    let (_, b) = emitter(&mut scene, "b", 2.0, 1000);
    let (_, c) = emitter(&mut scene, "c", 3.0, 1);

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };
    // Placed rows by their owner's X: (offset, count).
    let ranges = |join: &Join, out: &SceneBufferProjection| {
        let rows: Vec<CoronaEmitterSourceRow> = join.read(out, "corona_emitters");
        assert_eq!(rows.len(), MAX_CORONA_EMITTERS as usize);
        let mut placed = Vec::new();
        for (slot, row) in rows.iter().enumerate() {
            if row.motion[11] == 0.0 {
                assert!(
                    rows[slot..].iter().all(|row| row.motion[11] == 0.0),
                    "placed rows are packed into the leading rows"
                );
                break;
            }
            // Placed at the owner: its translation and scale.
            assert_eq!(&row.transform[12..16], &[row.transform[12], 2.0, -1.0, 1.0]);
            assert_eq!(row.transform[0], 1.0);
            // The emitter's identity: its source row + 1.
            assert_ne!(row.range[3], 0);
            assert!(
                rows[..slot]
                    .iter()
                    .all(|other| other.range[3] != row.range[3]),
                "identities are distinct"
            );
            placed.push((row.transform[12], row.range[1], row.range[2]));
        }
        placed
    };
    let disjoint = |placed: &[(f32, u32, u32)]| {
        for (i, &(_, offset, count)) in placed.iter().enumerate() {
            assert_eq!(offset % CORONA_RANGE_ALIGNMENT, 0, "aligned: {placed:?}");
            assert!(
                offset + count <= CORONA_POOL_PARTICLES,
                "in the pool: {placed:?}"
            );
            for &(_, other, other_count) in &placed[i + 1..] {
                assert!(
                    offset + count <= other || other + other_count <= offset,
                    "overlapping ranges: {placed:?}"
                );
            }
        }
    };

    let out = join.run(&mut scene);
    let placed = ranges(&join, &out);
    assert_eq!(
        placed,
        vec![(1.0, 0, 300), (2.0, 512, 1000), (3.0, 1536, 1)]
    );
    disjoint(&placed);

    // A hidden owner and a disabled instance give their ranges up; the
    // others pack down.
    scene.world.get_mut::<Visibility>(a).unwrap().visible = false;
    attachments::set_enabled(&mut scene.world, c, false);
    let out = join.run(&mut scene);
    assert_eq!(ranges(&join, &out), vec![(2.0, 0, 1000)]);
    attachments::set_enabled(&mut scene.world, b, false);
    let out = join.run(&mut scene);
    assert_eq!(ranges(&join, &out), Vec::new());
    scene.world.get_mut::<Visibility>(a).unwrap().visible = true;
    attachments::set_enabled(&mut scene.world, b, true);
    attachments::set_enabled(&mut scene.world, c, true);

    // A full pool: the most one emitter may request, five times over
    // (with the three above), clamps the last ranges to what is left.
    let largest = 262_144; // helio_pass_corona::CORONA_MAX_PARTICLES_PER_EMITTER
    for x in 4..9 {
        emitter(&mut scene, "large", x as f32, largest);
    }
    let out = join.run(&mut scene);
    let placed = ranges(&join, &out);
    assert_eq!(placed.len(), 8);
    disjoint(&placed);
    let total: u32 = placed.iter().map(|range| range.2).sum();
    assert!(total <= CORONA_POOL_PARTICLES);
    assert_eq!(placed[3].2, largest);
    let last = placed[7];
    assert_eq!(
        last.2, 0,
        "nothing is left for the last emitter: {placed:?}"
    );
    assert!(
        placed[6].2 < largest && placed[6].2 > 0,
        "the one that reaches the end is clamped to it: {placed:?}"
    );
}

/// The foliage passes' one wind row (#1123): the first foliage component's
/// own wind without a global wind; the level's global wind (a
/// `WindComponent`) over it, even a calm one, whatever the owner's
/// visibility; a component that opts out of the global wind over both; a
/// disabled global wind gives the components their own wind back.
#[test]
fn foliage_sways_in_the_global_wind_unless_it_opts_out() {
    use helio_component::components::WindComponent;
    type WindRow = helio_pass_foliage_place::components::FoliageWindComponent;
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let grass = |wind_speed: f32| {
        let mut foliage = FoliageComponent::default();
        foliage.general.enabled = true;
        foliage.wind.wind_speed = wind_speed;
        foliage
    };
    let meadow = place(&mut scene, "meadow", at([0.0; 3]));
    pulsar_world_registry::attach_value(&mut scene.world, meadow, grass(2.0)).unwrap();
    let lawn = place(&mut scene, "lawn", at([50.0, 0.0, 0.0]));
    let lawn = pulsar_world_registry::attach_value(&mut scene.world, lawn, grass(5.0)).unwrap();

    engine_backend::scene::ensure_gpu_mirror(&mut scene, Arc::clone(&device), Arc::clone(&queue));
    let mut join = Join {
        join: engine_backend::scene::environment_join(&device),
        device,
        queue,
    };
    let speed = |join: &Join, out: &SceneBufferProjection| {
        let rows: Vec<WindRow> = join.read(out, "foliage_wind");
        assert_eq!(rows.len(), 1);
        rows[0].direction_speed
    };

    let out = join.run(&mut scene);
    assert_eq!(
        speed(&join, &out)[3],
        2.0,
        "no global wind: the first own wind"
    );

    let breeze = place(&mut scene, "wind", at([0.0; 3]));
    let global = WindComponent {
        direction: [0.0, 0.0, -3.0],
        speed: 9.0,
        ..Default::default()
    };
    let wind = pulsar_world_registry::attach_value(&mut scene.world, breeze, global).unwrap();
    let out = join.run(&mut scene);
    assert_eq!(speed(&join, &out), [0.0, 0.0, -1.0, 9.0], "the global wind");
    scene.world.get_mut::<Visibility>(breeze).unwrap().visible = false;
    let out = join.run(&mut scene);
    assert_eq!(speed(&join, &out)[3], 9.0, "visibility does not apply");
    scene.world.get_mut::<WindComponent>(wind).unwrap().speed = 0.0;
    let out = join.run(&mut scene);
    assert_eq!(
        speed(&join, &out)[3],
        0.0,
        "a calm global wind is still the wind"
    );

    scene
        .world
        .get_mut::<FoliageComponent>(lawn)
        .unwrap()
        .wind
        .use_global_wind = false;
    let out = join.run(&mut scene);
    assert_eq!(
        speed(&join, &out)[3],
        5.0,
        "an opted-out component's own wind"
    );
    scene
        .world
        .get_mut::<FoliageComponent>(lawn)
        .unwrap()
        .wind
        .use_global_wind = true;

    attachments::set_enabled(&mut scene.world, wind, false);
    let out = join.run(&mut scene);
    assert_eq!(speed(&join, &out)[3], 2.0, "a disabled global wind");
}

/// Water (#1065): every placed volume takes the scene's sun (its first
/// directional light, oriented by its owner) in place of an authored one,
/// and the level's global wind unless it opts out; its own spring, damping
/// and wave scale reach its row.
#[test]
fn water_takes_the_scenes_sun_and_the_global_wind() {
    use helio_component::components::{LightComponent, LightType, WindComponent};
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
    let lit: helio::GpuLight =
        join.read::<helio::GpuLight>(&out, "scene_lights")[light.index() as usize];
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
    let strength = 6.0 * helio_default_graphs::environment_join::WATER_WIND_STRENGTH_PER_SPEED;
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
