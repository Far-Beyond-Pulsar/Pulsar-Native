//! The foliage plugin's join over the engine's rows: foliage types, layers
//! and the level's wind, authored as component instances, reach the rows
//! the foliage passes read. Placement and gating are read back from the
//! join's outputs. Needs a GPU adapter (lavapipe works); skips without one.

// The join harness is the engine's; not every test here uses every helper.
#![allow(dead_code)]

use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility, attachments};
use foliage_component::FoliageComponent;
use helio_core::ENTITY_GENERATIONS_KEY;
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

fn close(a: [f32; 4], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
}

#[test]
fn foliage_types_layers_and_wind_are_packed() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    type TypeRow = helio_pass_foliage_place::components::FoliageTypeRow;
    type LayerRow = helio_pass_foliage_place::components::FoliageLayerRow;
    type WindRow = helio_pass_foliage_place::components::FoliageWindRow;

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
        helio_pass_foliage_place::scene_rows::MAX_FOLIAGE_TYPES as usize
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

/// The foliage passes' one wind row (#1123): the first foliage component's
/// own wind without a global wind; the level's global wind (a
/// `WindComponent`) over it, even a calm one, whatever the owner's
/// visibility; a component that opts out of the global wind over both; a
/// disabled global wind gives the components their own wind back.
#[test]
fn foliage_sways_in_the_global_wind_unless_it_opts_out() {
    use foliage_component::WindComponent;
    type WindRow = helio_pass_foliage_place::components::FoliageWindRow;
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
