//! Moving things must cost in proportion to what moved, not to the size of the
//! scene. This drives the engine's own rows through the real GPU mirror and
//! scene join with N mesh objects and moves M of them per frame, timing each
//! stage on its own so a stage that grows with N shows up by name.
//!
//! Run the report with:
//! `cargo test -p engine_backend --release --test movement_scaling -- --nocapture --ignored`
//!
//! Needs a GPU adapter (lavapipe works); skips without one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform};
use helio_component::components::{MeshAssetPath, ObjectMovability, StaticMeshComponent};
use helio_default_graphs::scene_join::{
    BufferHandle, SceneBufferProjection, SceneDerivationContext, ENTITY_GENERATIONS_KEY,
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

fn mesh() -> StaticMeshComponent {
    let vertex = |x: f32, y: f32| helio::PackedVertex {
        position: [x, y, 0.0],
        ..Default::default()
    };
    StaticMeshComponent {
        mesh_asset: MeshAssetPath::new(""),
        vertices: vec![vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(0.0, 1.0)],
        indices: vec![0, 1, 2],
        mesh_sections: vec![helio_component::mesh_cache::MeshSection {
            first_index: 0,
            index_count: 3,
            material_slot: 0,
        }],
        bounds_local: [0.5, 0.5, 0.0, 0.8],
        movability: ObjectMovability::Movable,
        ..Default::default()
    }
}

#[derive(Default, Clone, Copy)]
struct Stages {
    mutate: Duration,
    flush: Duration,
    derive: Duration,
    gpu: Duration,
}

impl Stages {
    fn cpu(&self) -> Duration {
        self.mutate + self.flush + self.derive
    }
    fn total(&self) -> Duration {
        self.cpu() + self.gpu
    }
}

struct Bench {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    scene: SceneDb,
    join: Box<dyn helio_default_graphs::scene_join::SceneDerivation>,
    objects: Vec<Entity>,
}

impl Bench {
    fn new(count: usize) -> Option<Self> {
        let (device, queue) = device()?;
        let mut scene = SceneDb::new();
        let mut objects = Vec::with_capacity(count);
        for i in 0..count {
            let object = scene
                .world
                .spawn_object(SpawnObject::new(format!("o{i}")))
                .expect("spawn");
            scene.world.get_mut::<Transform>(object).unwrap().position =
                [(i % 100) as f32, (i / 100 % 100) as f32, (i / 10_000) as f32];
            pulsar_world_registry::attach_value(&mut scene.world, object, mesh()).unwrap();
            objects.push(object);
        }
        engine_backend::scene::ensure_gpu_mirror(
            &mut scene,
            Arc::clone(&device),
            Arc::clone(&queue),
        );
        let join = engine_backend::scene::scene_join(&device, false);
        let mut bench = Self {
            device,
            queue,
            scene,
            join,
            objects,
        };
        // The first frames build everything and settle caches.
        for _ in 0..3 {
            bench.frame(0, 0.0);
        }
        Some(bench)
    }

    /// One frame: move `moved` objects, flush the mirror, run the join, wait
    /// for the GPU.
    fn frame(&mut self, moved: usize, phase: f32) -> Stages {
        let mut stages = Stages::default();

        let start = Instant::now();
        let count = self.objects.len();
        for k in 0..moved {
            // A spread-out set, not a run of neighbours.
            let object = self.objects[(k * 7919) % count];
            self.scene.world.get_mut::<Transform>(object).unwrap().position[1] = phase + k as f32;
        }
        stages.mutate = start.elapsed();

        let start = Instant::now();
        self.scene.step();
        stages.flush = start.elapsed();

        let start = Instant::now();
        let mirror = self.scene.world.gpu_mirror().expect("mirror").clone();
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
        self.join.derive(
            &SceneDerivationContext {
                device: &self.device,
                queue: &self.queue,
                inputs: &inputs,
                world_origin: None,
            },
            &mut encoder,
        );
        self.queue.submit([encoder.finish()]);
        stages.derive = start.elapsed();

        let start = Instant::now();
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        stages.gpu = start.elapsed();
        stages
    }

    /// Median of `frames` frames, stage by stage.
    fn measure(&mut self, moved: usize, frames: usize) -> Stages {
        let mut samples: Vec<Stages> = (0..frames)
            .map(|frame| self.frame(moved, frame as f32 * 0.37))
            .collect();
        let median = |select: fn(&Stages) -> Duration, samples: &mut Vec<Stages>| {
            samples.sort_by_key(|s| select(s));
            select(&samples[samples.len() / 2])
        };
        Stages {
            mutate: median(|s| s.mutate, &mut samples),
            flush: median(|s| s.flush, &mut samples),
            derive: median(|s| s.derive, &mut samples),
            gpu: median(|s| s.gpu, &mut samples),
        }
    }
}

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

/// The scaling report: per-stage microseconds for every (N, M).
#[test]
#[ignore = "measurement; run with --release --ignored --nocapture"]
fn movement_scaling_report() {
    let sizes = [1_000usize, 5_000, 20_000, 50_000];
    let moves = [1usize, 100, 1_000];
    println!(
        "{:>7} {:>6} | {:>9} {:>9} {:>9} {:>9} | {:>9}",
        "N", "M", "mutate", "flush", "derive", "gpu", "total (us)"
    );
    for &n in &sizes {
        let Some(mut bench) = Bench::new(n) else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        for &m in &moves {
            if m > n {
                continue;
            }
            let s = bench.measure(m, 15);
            println!(
                "{:>7} {:>6} | {:>9.0} {:>9.0} {:>9.0} {:>9.0} | {:>9.0}",
                n,
                m,
                micros(s.mutate),
                micros(s.flush),
                micros(s.derive),
                micros(s.gpu),
                micros(s.total())
            );
        }
    }
}

/// The CPU cost of moving one object must not grow with how many others the
/// scene holds: mutation, mirror flush and join encoding are all about the
/// moved row. (GPU time is reported by the report above; the join's kernel
/// still visits every row, which costs bandwidth, not CPU.)
#[test]
fn moving_one_object_costs_the_same_in_a_small_and_a_large_scene() {
    let (Some(mut small), Some(mut large)) = (Bench::new(1_000), Bench::new(20_000)) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let small = small.measure(1, 31).cpu();
    let large = large.measure(1, 31).cpu();
    println!("cpu to move one object: 1k scene {small:?}, 20k scene {large:?}");
    // Generous: 20x the objects may cost noise and cache effects, not 20x.
    assert!(
        large < small * 6 + Duration::from_micros(300),
        "moving one object costs {large:?} in a 20k scene but {small:?} in a 1k scene"
    );
}

/// What is moved sets the CPU cost: a thousand moved objects cost about a
/// thousand times one, not a thousand times the scene, and the same thousand
/// cost the same in a small scene as in a large one.
#[test]
fn cpu_cost_follows_the_moved_count_not_the_scene_size() {
    let (Some(mut small), Some(mut large)) = (Bench::new(2_000), Bench::new(50_000)) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let small_many = small.measure(1_000, 21).cpu();
    let large_many = large.measure(1_000, 21).cpu();
    let large_one = large.measure(1, 21).cpu();
    println!(
        "cpu: 1000 moved in 2k scene {small_many:?}; 1000 moved in 50k scene {large_many:?}; \
         1 moved in 50k scene {large_one:?}"
    );
    assert!(
        large_many < small_many * 4 + Duration::from_micros(500),
        "1000 moved objects cost {large_many:?} in a 50k scene but {small_many:?} in a 2k one"
    );
    assert!(
        large_one * 2 < large_many,
        "one moved object ({large_one:?}) should cost well under a thousand ({large_many:?})"
    );
}
