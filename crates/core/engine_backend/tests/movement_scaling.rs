//! Moving things must cost in proportion to what moved, not to the size of the
//! scene. This drives the engine's own rows through the real GPU mirror, scene
//! join and object batch with N mesh objects and moves M of them per frame,
//! timing each stage on its own so a stage that grows with N shows up by name.
//! CPU stages are wall-clock; the join and the object batch also get GPU time
//! from timestamp queries (when the adapter has them), which is what shows the
//! GPU following M and not N.
//!
//! Run the report with:
//! `cargo test -p engine_backend --release --test movement_scaling -- --nocapture --ignored`
//!
//! Needs a GPU adapter (lavapipe works); skips without one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform};
use helio_component::components::{MeshAssetPath, ObjectMovability, StaticMeshComponent};
use helio_core::{GpuCameraUniforms, PrepareContext, ResourceRegistry};
use helio_default_graphs::scene_join::{
    BufferHandle, SceneBufferProjection, SceneDerivation, ENTITY_GENERATIONS_KEY, MATERIALS_KEY,
    OBJECTS_KEY,
};
use helio_pass_object_batch::{BatchRun, ObjectBatchPass};
use pulsar_scenedb::{Entity, SceneDb};

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let timestamps =
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: adapter.features() & timestamps,
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

/// Timestamps around the join and the object batch of one frame.
struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period: f32,
}

const STAMPS: u32 = 3;

impl GpuTimer {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        let needed =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        if !device.features().contains(needed) {
            return None;
        }
        let size = u64::from(STAMPS) * 8;
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("movement timer"),
                ty: wgpu::QueryType::Timestamp,
                count: STAMPS,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("movement timer resolve"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("movement timer readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            period: queue.get_timestamp_period(),
        })
    }

    /// `(join, batch)` GPU time of the submitted frame.
    fn read(&self, device: &wgpu::Device) -> (Duration, Duration) {
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map timer"));
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let stamps: Vec<u64> = {
            let data = slice.get_mapped_range().expect("mapped timer");
            bytemuck::cast_slice::<u8, u64>(&data).to_vec()
        };
        self.readback.unmap();
        let span = |a: u64, b: u64| {
            Duration::from_nanos((b.saturating_sub(a) as f64 * f64::from(self.period)) as u64)
        };
        (span(stamps[0], stamps[1]), span(stamps[1], stamps[2]))
    }
}

#[derive(Default, Clone, Copy)]
struct Stages {
    mutate: Duration,
    flush: Duration,
    derive: Duration,
    /// Wall clock spent waiting for the GPU after submit.
    wait: Duration,
    /// GPU time of the join and the object batch (zero without timestamps).
    join_gpu: Duration,
    batch_gpu: Duration,
}

impl Stages {
    fn cpu(&self) -> Duration {
        self.mutate + self.flush + self.derive
    }
    fn total(&self) -> Duration {
        self.cpu() + self.wait
    }
}

struct Bench {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    scene: SceneDb,
    joins: Vec<Box<dyn SceneDerivation>>,
    batch: ObjectBatchPass,
    camera: wgpu::Buffer,
    timer: Option<GpuTimer>,
    objects: Vec<Entity>,
    /// What the last frame's batch recorded.
    last_run: BatchRun,
    /// The last frame's join inputs, before and after the join added its
    /// outputs.
    last_raw: SceneBufferProjection,
    last_inputs: SceneBufferProjection,
    frames: u32,
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
                [(i % 100) as f64, (i / 100 % 100) as f64, (i / 10_000) as f64];
            pulsar_world_registry::attach_value(&mut scene.world, object, mesh()).unwrap();
            objects.push(object);
        }
        engine_backend::scene::ensure_gpu_mirror(
            &mut scene,
            Arc::clone(&device),
            Arc::clone(&queue),
        );
        let joins: Vec<Box<dyn SceneDerivation>> =
            vec![engine_backend::scene::scene_join(&device, false)];
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("movement camera"),
            size: std::mem::size_of::<GpuCameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut bench = Self {
            timer: GpuTimer::new(&device, &queue),
            batch: ObjectBatchPass::new(&device),
            device,
            queue,
            scene,
            joins,
            camera,
            objects,
            last_run: BatchRun::Skipped,
            last_raw: SceneBufferProjection::empty(),
            last_inputs: SceneBufferProjection::empty(),
            frames: 0,
        };
        // The first frames build everything and settle caches.
        for _ in 0..3 {
            bench.frame(0, 0.0);
        }
        Some(bench)
    }

    /// One frame: move `moved` objects, flush the mirror, run the join and the
    /// object batch, wait for the GPU.
    fn frame(&mut self, moved: usize, phase: f32) -> Stages {
        let mut stages = Stages::default();

        let start = Instant::now();
        let count = self.objects.len();
        for k in 0..moved {
            // A spread-out set, not a run of neighbours.
            let object = self.objects[(k * 7919) % count];
            self.scene.world.get_mut::<Transform>(object).unwrap().position[1] = f64::from(phase + k as f32);
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
        self.last_raw = inputs.clone();
        let derived = helio_core::run_scene_derivations(
            &mut self.joins,
            &self.device,
            &self.queue,
            &mut inputs,
            None,
        );

        let mut head = self.device.create_command_encoder(&Default::default());
        if let Some(timer) = &self.timer {
            head.write_timestamp(&timer.queries, 0);
        }
        let mut batch = self.device.create_command_encoder(&Default::default());
        if let Some(timer) = &self.timer {
            batch.write_timestamp(&timer.queries, 1);
        }
        let registry = ResourceRegistry::empty();
        let camera_data: GpuCameraUniforms = bytemuck::Zeroable::zeroed();
        self.last_run = self.batch.frame_for_testing(
            &PrepareContext {
                device: &self.device,
                queue: &self.queue,
                frame_num: u64::from(self.frames),
                camera: &self.camera,
                camera_data: &camera_data,
                camera_generation: 0,
                scene_buffers: &inputs,
                registry: &registry,
                resize: false,
                width: 64,
                height: 64,
                delta_time: 0.016,
                time: 0.0,
                time_delta: 0.016,
                world_origin: None,
                owns_device: true,
            },
            &mut batch,
        );
        self.frames += 1;
        if let Some(timer) = &self.timer {
            batch.write_timestamp(&timer.queries, 2);
            batch.resolve_query_set(&timer.queries, 0..STAMPS, &timer.resolve, 0);
            batch.copy_buffer_to_buffer(
                &timer.resolve,
                0,
                &timer.readback,
                0,
                u64::from(STAMPS) * 8,
            );
        }
        let mut commands = vec![head.finish()];
        commands.extend(derived);
        commands.push(batch.finish());
        self.queue.submit(commands);
        stages.derive = start.elapsed();
        self.last_inputs = inputs;

        let start = Instant::now();
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        stages.wait = start.elapsed();
        if let Some(timer) = &self.timer {
            (stages.join_gpu, stages.batch_gpu) = timer.read(&self.device);
        }
        stages
    }

    /// Median of `frames` frames, stage by stage.
    fn measure(&mut self, moved: usize, frames: usize) -> Stages {
        let mut samples: Vec<Stages> = (0..frames)
            .map(|frame| self.frame(moved, (frame + 1) as f32 * 0.37))
            .collect();
        let median = |select: fn(&Stages) -> Duration, samples: &mut Vec<Stages>| {
            samples.sort_by_key(|s| select(s));
            select(&samples[samples.len() / 2])
        };
        Stages {
            mutate: median(|s| s.mutate, &mut samples),
            flush: median(|s| s.flush, &mut samples),
            derive: median(|s| s.derive, &mut samples),
            wait: median(|s| s.wait, &mut samples),
            join_gpu: median(|s| s.join_gpu, &mut samples),
            batch_gpu: median(|s| s.batch_gpu, &mut samples),
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
    let sizes = [1_000usize, 5_000, 20_000, 50_000, 100_000];
    let moves = [1usize, 100, 1_000];
    println!(
        "{:>7} {:>6} | {:>8} {:>8} {:>8} | {:>9} {:>9} | {:>9} {:>9} | batch run",
        "N", "M", "mutate", "flush", "derive", "join gpu", "batch gpu", "wait", "total"
    );
    for &n in &sizes {
        let Some(mut bench) = Bench::new(n) else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        if bench.timer.is_none() {
            eprintln!("note: adapter has no timestamp queries; GPU columns read 0");
        }
        for &m in &moves {
            if m > n {
                continue;
            }
            let s = bench.measure(m, 15);
            println!(
                "{:>7} {:>6} | {:>8.0} {:>8.0} {:>8.0} | {:>9.1} {:>9.1} | {:>9.0} {:>9.0} | {:?}",
                n,
                m,
                micros(s.mutate),
                micros(s.flush),
                micros(s.derive),
                micros(s.join_gpu),
                micros(s.batch_gpu),
                micros(s.wait),
                micros(s.total()),
                bench.last_run,
            );
        }
    }
}

fn read_buffer(device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::Buffer) -> Vec<u8> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: src.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(src, 0, &staging, 0, src.size());
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().expect("mapped").to_vec();
    staging.unmap();
    data
}

/// Moving only some objects takes the cheap path end to end, and what it
/// leaves behind is exactly what rebuilding everything from the same rows
/// would: the join's object rows byte for byte, the object batch's sorted
/// instances slot for slot (matched by where the object is, since two full
/// runs may order equal sort keys differently).
#[test]
fn the_moved_rows_path_matches_a_full_rebuild() {
    let Some(mut bench) = Bench::new(3_000) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let mut runs = Vec::new();
    for frame in 1..=5 {
        bench.frame(40, frame as f32 * 0.37);
        runs.push(bench.last_run);
    }
    assert_eq!(
        runs.last(),
        Some(&BatchRun::Moved),
        "steady-state movement must gather only the moved rows ({runs:?})"
    );

    // Join: the same inputs through a join that has never run.
    let mut reference: Vec<Box<dyn SceneDerivation>> =
        vec![engine_backend::scene::scene_join(&bench.device, false)];
    let mut reference_inputs = bench.last_raw.clone();
    if let Some(commands) = helio_core::run_scene_derivations(
        &mut reference,
        &bench.device,
        &bench.queue,
        &mut reference_inputs,
        None,
    ) {
        bench.queue.submit([commands]);
    }
    let rows = |inputs: &SceneBufferProjection| {
        read_buffer(
            &bench.device,
            &bench.queue,
            &inputs.get(OBJECTS_KEY).expect("objects").buffer,
        )
    };
    assert!(
        rows(&bench.last_inputs) == rows(&reference_inputs),
        "the join's object rows differ from a full rebuild's"
    );

    // Object batch: a fresh pass doing its full run over the same rows.
    let live = 3_000usize;
    let mut fresh = ObjectBatchPass::new(&bench.device);
    let objects = reference_inputs.get(OBJECTS_KEY).unwrap().buffer.clone();
    let materials = reference_inputs.get(MATERIALS_KEY).unwrap().buffer.clone();
    fresh.run_once_for_testing(
        &bench.device,
        &bench.queue,
        &objects,
        &materials,
        (objects.size() / 252) as u32,
    );
    const STRIDE: usize = 224;
    let by_position = |instances: &[u8]| {
        (0..live)
            .map(|i| {
                let slot = &instances[i * STRIDE..(i + 1) * STRIDE];
                // Bounds centre, then the whole slot but the previous
                // transform (a fresh pass has no history).
                (slot[112..124].to_vec(), (slot[..64].to_vec(), slot[192..].to_vec()))
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let got = by_position(&read_buffer(&bench.device, &bench.queue, bench.batch.instances_buffer()));
    let want = by_position(&read_buffer(&bench.device, &bench.queue, fresh.instances_buffer()));
    assert_eq!(got.len(), live, "every object has its own position");
    assert_eq!(want.len(), live);
    for (position, expected) in &want {
        assert!(got.get(position) == Some(expected), "instance at {position:?} differs");
    }
}

/// The CPU cost of moving one object must not grow with how many others the
/// scene holds: mutation, mirror flush and join encoding are all about the
/// moved row.
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
