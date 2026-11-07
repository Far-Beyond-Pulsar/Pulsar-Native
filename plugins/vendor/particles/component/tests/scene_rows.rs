//! The plugin's join over the engine's rows: particle emitters, authored as
//! component instances, reach the rows the plugin's passes read. Placement
//! and gating are read back from the join's outputs. Needs a GPU adapter
//! (lavapipe works); skips without one.

use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform, Visibility, attachments};
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

/// Particle emitters (#1059): an enabled emitter of a visible owner becomes
/// a Corona emitter row, packed into the leading rows, placed at its owner
/// and given its own range of the shared particle pool, sized by its
/// `max_particles`; the ranges are disjoint and aligned, a full pool clamps
/// the last ones, and hidden owners or disabled instances give theirs up.
#[test]
fn particle_emitters_take_disjoint_ranges_of_the_particle_pool() {
    use helio_pass_corona::{
        CORONA_MAX_EMITTERS as MAX_CORONA_EMITTERS, CORONA_MAX_PARTICLES as CORONA_POOL_PARTICLES,
        CORONA_RANGE_ALIGNMENT,
    };
    use particles_component::{CoronaEmitterSourceRow, ParticleEmitterComponent};
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut scene = SceneDb::new();
    let emitter = |scene: &mut SceneDb, name: &str, x: f64, max_particles: u32| {
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
        emitter(&mut scene, "large", f64::from(x), largest);
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
