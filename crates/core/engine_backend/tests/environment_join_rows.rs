//! Helio's environment join over this engine's own rows (Pulsar-Native#1035,
//! Phase 4): fog volumes, post-process volumes and camera post-process
//! components, authored as component instances, reach the rows the
//! volumetric fog and post-process passes read. Placement and gating are
//! read back from the join's outputs. Needs a GPU adapter (lavapipe works);
//! skips without one.

use std::sync::Arc;

use engine_backend::scene::{attachments, SceneWorldExt, SpawnObject, Transform, Visibility};
use helio_component::components::{
    CameraPostProcessComponent, GlobalFogComponent, LocalFogVolumeComponent,
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
