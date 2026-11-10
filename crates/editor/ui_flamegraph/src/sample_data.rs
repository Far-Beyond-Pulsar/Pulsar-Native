//! Demo trace generation for the flamegraph UI.
//!
//! Owns the synthetic multi-threaded engine workload behind
//! `TraceData::with_sample_data`. Each engine thread has its own `emit_*`
//! function, the operation names live in `const` tables, and
//! [`ThreadEmitter`] provides the two shapes of nesting used throughout (an even
//! split over named operations, and an even split over N numbered items).
//!
//! Entry point: [`populate_sample_trace`].

use rand::rngs::ThreadRng;
use rand::RngExt;

use crate::trace_data::{TraceData, TraceSpan};

/// Number of frames in the default sample trace.
pub(crate) const SAMPLE_FRAME_COUNT: usize = 2000;

const TARGET_FRAME_NS: u64 = 16_600_000; // 16.6ms target
const MIN_FRAME_NS: u64 = 8_000_000;

const THREAD_GPU: u64 = 0;
const THREAD_MAIN: u64 = 1;
const THREAD_RENDER: u64 = 2;
const THREAD_PHYSICS: u64 = 3;
const THREAD_AUDIO: u64 = 4;
const THREAD_NETWORK: u64 = 5;
const THREAD_IO: u64 = 6;
const FIRST_WORKER_THREAD: u64 = 7;
const WORKER_COUNT: u64 = 12;

/// GPU passes as `(name, min_duration_ns, max_duration_ns)`.
const GPU_PASSES: [(&str, u64, u64); 5] = [
    ("ShadowPass", 1_500_000, 3_000_000),
    ("GeometryPass", 2_000_000, 4_000_000),
    ("LightingPass", 2_500_000, 4_500_000),
    ("PostProcess", 1_000_000, 2_500_000),
    ("Present", 500_000, 1_000_000),
];
const SHADER_STAGES: [&str; 3] = ["VertexShader", "GeometryShader", "FragmentShader"];
const TEXTURE_FETCHES: [&str; 3] = ["SampleAlbedo", "SampleNormal", "SampleMetallic"];

const ENTITY_SYSTEMS: [&str; 4] = ["Transform", "Physics", "Animation", "AI"];
const AI_OPS: [&str; 4] = ["Perception", "DecisionTree", "Pathfinding", "Behavior"];
const PATHFINDING_OPS: [&str; 3] = ["BuildGraph", "AStar", "SmoothPath"];
const ANIMATION_OPS: [&str; 4] = ["Sample", "Blend", "IK", "ApplyPose"];

const RENDER_TASKS: [&str; 4] = ["Cull", "Sort", "BuildCmdBuffer", "Submit"];
const CULL_OPS: [&str; 3] = ["FrustumCull", "OcclusionQuery", "LODSelect"];
const COMMAND_OPS: [&str; 4] = [
    "SetPipeline",
    "BindDescriptors",
    "SetConstants",
    "DrawIndexed",
];

const PHYSICS_PHASES: [&str; 4] = ["BroadPhase", "NarrowPhase", "SolveConstraints", "Integrate"];
const BROAD_PHASE_OPS: [&str; 3] = ["UpdateAABBs", "SortAndSweep", "BuildPairs"];
const COLLISION_OPS: [&str; 4] = ["GJK", "EPA", "ContactGen", "Manifold"];
const GJK_STEPS: [&str; 3] = ["Support", "Simplex", "ClosestPoint"];

const AUDIO_TASKS: [&str; 4] = ["MixChannels", "ApplyEffects", "StreamDecode", "Output"];
const NETWORK_OPS: [&str; 4] = ["RecvPackets", "ProcessEvents", "SendUpdates", "Serialize"];

const IO_OPS: [&str; 3] = ["LoadAsset", "StreamTexture", "WriteCache"];
const LOAD_OPS: [&str; 5] = ["OpenFile", "ReadData", "Decompress", "Parse", "Validate"];
const DECOMPRESS_OPS: [&str; 3] = ["ReadHeader", "InflateBlocks", "CRC32"];
const PARSE_OPS: [&str; 3] = ["ReadChunks", "BuildScene", "LinkRefs"];
const STREAM_OPS: [&str; 3] = ["FetchMip", "Transcode", "Upload"];
const TRANSCODE_OPS: [&str; 3] = ["BCDecode", "ConvertFormat", "GenMipmaps"];

const WORKER_TASK_TYPES: [&str; 4] = ["MeshProcess", "ParticleSim", "ComputeJob", "DataSort"];
const MESH_OPS: [&str; 3] = ["GenNormals", "ComputeTangents", "Optimize"];
const PARTICLE_OPS: [&str; 4] = ["UpdateForces", "Integrate", "Collide", "Sort"];

/// Fills `trace` with `frame_count` frames of a realistic multi-threaded
/// workload: GPU, main, render, physics, audio, network and I/O threads plus a
/// pool of job-system workers.
pub(crate) fn populate_sample_trace(trace: &TraceData, frame_count: usize) {
    let mut rng = rand::rng();
    let mut current_time = 0u64;

    for frame_idx in 0..frame_count {
        let frame_start = current_time;
        let frame_duration = random_frame_duration(&mut rng);

        trace.add_frame_time(frame_duration as f32 / 1_000_000.0);
        trace.add_frame_boundary(frame_start);

        emit_gpu_thread(trace, &mut rng, frame_start);
        emit_main_thread(trace, &mut rng, frame_idx, frame_start, frame_duration);
        emit_render_thread(trace, &mut rng, frame_start);
        emit_physics_thread(trace, &mut rng, frame_start);
        emit_audio_thread(trace, &mut rng, frame_start);
        if frame_idx % 3 == 0 {
            // Network updates every 3rd frame
            emit_network_thread(trace, &mut rng, frame_start);
        }
        if frame_idx % 5 == 0 {
            // I/O operations every 5th frame
            emit_io_thread(trace, &mut rng, frame_start);
        }
        emit_worker_threads(trace, &mut rng, frame_start);

        current_time += frame_duration;
    }
}

/// Target frame time with random variance, never below `MIN_FRAME_NS`.
fn random_frame_duration(rng: &mut ThreadRng) -> u64 {
    let frame_variance = rng.random_range(-3_000_000i64..5_000_000i64);
    (TARGET_FRAME_NS as i64 + frame_variance).max(MIN_FRAME_NS as i64) as u64
}

/// Returns a naming function that prepends `prefix` to an operation name.
fn prefixed(prefix: &'static str) -> impl Fn(&str) -> String {
    move |name| format!("{}{}", prefix, name)
}

/// Nested-span callback for operations that have no children.
fn no_children(_name: &str, _start_ns: u64, _duration_ns: u64) {}

/// Emits spans onto one trace thread.
struct ThreadEmitter<'a> {
    trace: &'a TraceData,
    thread_id: u64,
}

impl<'a> ThreadEmitter<'a> {
    fn new(trace: &'a TraceData, thread_id: u64) -> Self {
        Self { trace, thread_id }
    }

    /// Adds one span on this thread.
    fn span(
        &self,
        name: impl Into<String>,
        start_ns: u64,
        duration_ns: u64,
        depth: u32,
        color_index: u8,
    ) {
        self.trace.add_span(TraceSpan {
            name: name.into(),
            start_ns,
            duration_ns,
            depth,
            thread_id: self.thread_id,
            color_index,
        });
    }

    /// Splits `parent_duration_ns` evenly across `names`, emitting one span per
    /// name (labelled by `label`) laid out back to back from `start_ns`. After
    /// each span, `nested(name, span_start, span_duration)` can emit its children.
    #[allow(clippy::too_many_arguments)]
    fn split_evenly_with(
        &self,
        names: &[&str],
        label: impl Fn(&str) -> String,
        start_ns: u64,
        parent_duration_ns: u64,
        depth: u32,
        color_index: u8,
        mut nested: impl FnMut(&str, u64, u64),
    ) {
        let duration_ns = parent_duration_ns / names.len() as u64;
        let mut time = start_ns;
        for name in names {
            self.span(label(name), time, duration_ns, depth, color_index);
            nested(name, time, duration_ns);
            time += duration_ns;
        }
    }

    /// Like [`Self::split_evenly_with`] for operations without children.
    fn split_evenly(
        &self,
        names: &[&str],
        label: impl Fn(&str) -> String,
        start_ns: u64,
        parent_duration_ns: u64,
        depth: u32,
        color_index: u8,
    ) {
        self.split_evenly_with(
            names,
            label,
            start_ns,
            parent_duration_ns,
            depth,
            color_index,
            no_children,
        );
    }

    /// Splits `parent_duration_ns` evenly into `count` spans named `{prefix}{index}`,
    /// back to back from `start_ns`. After each span,
    /// `nested(index, span_start, span_duration)` can emit its children.
    #[allow(clippy::too_many_arguments)]
    fn split_n_with(
        &self,
        prefix: &str,
        count: u64,
        start_ns: u64,
        parent_duration_ns: u64,
        depth: u32,
        color_index: u8,
        mut nested: impl FnMut(u64, u64, u64),
    ) {
        let duration_ns = parent_duration_ns / count;
        let mut time = start_ns;
        for index in 0..count {
            self.span(
                format!("{}{}", prefix, index),
                time,
                duration_ns,
                depth,
                color_index,
            );
            nested(index, time, duration_ns);
            time += duration_ns;
        }
    }

    /// Like [`Self::split_n_with`] for items without children.
    fn split_n(
        &self,
        prefix: &str,
        count: u64,
        start_ns: u64,
        parent_duration_ns: u64,
        depth: u32,
        color_index: u8,
    ) {
        self.split_n_with(
            prefix,
            count,
            start_ns,
            parent_duration_ns,
            depth,
            color_index,
            |_, _, _| {},
        );
    }
}

/// Thread 0: GPU frame with render passes, draw calls, shader stages and texture fetches.
fn emit_gpu_thread(trace: &TraceData, rng: &mut ThreadRng, frame_start: u64) {
    let gpu = ThreadEmitter::new(trace, THREAD_GPU);
    let gpu_start = frame_start + rng.random_range(1_000_000..3_000_000);
    let gpu_duration = rng.random_range(8_000_000..14_000_000);

    gpu.span("GPU Frame", gpu_start, gpu_duration, 0, 10);

    let mut gpu_time = gpu_start;
    for (pass_name, min_dur, max_dur) in GPU_PASSES {
        let pass_dur = rng.random_range(min_dur..max_dur);
        gpu.span(pass_name, gpu_time, pass_dur, 1, 11);

        // Draw calls per pass
        let num_draws = rng.random_range(5..20);
        gpu.split_n_with(
            "Draw_",
            num_draws,
            gpu_time,
            pass_dur,
            2,
            12,
            |_, draw_time, draw_dur| {
                // Shader stages within draw calls
                if pass_name == "GeometryPass" || pass_name == "LightingPass" {
                    gpu.split_evenly_with(
                        &SHADER_STAGES,
                        prefixed(""),
                        draw_time,
                        draw_dur,
                        3,
                        13,
                        |stage, shader_time, stage_dur| {
                            // Texture fetches in fragment shader
                            if stage == "FragmentShader" && pass_name == "LightingPass" {
                                gpu.split_evenly(
                                    &TEXTURE_FETCHES,
                                    prefixed("Tex::"),
                                    shader_time,
                                    stage_dur,
                                    4,
                                    14,
                                );
                            }
                        },
                    );
                }
            },
        );
        gpu_time += pass_dur;
    }
}

/// Thread 1: main thread with input, then game logic over entities and their component systems.
fn emit_main_thread(
    trace: &TraceData,
    rng: &mut ThreadRng,
    frame_idx: usize,
    frame_start: u64,
    frame_duration: u64,
) {
    let main = ThreadEmitter::new(trace, THREAD_MAIN);
    main.span(
        format!("Frame {}", frame_idx),
        frame_start,
        frame_duration,
        0,
        0,
    );

    let mut main_time = frame_start;

    // Input processing
    let input_dur = rng.random_range(100_000..500_000);
    main.span("Input::Process", main_time, input_dur, 1, 1);
    main_time += input_dur;

    // Game logic update with deeper callstacks
    let update_dur = rng.random_range(2_000_000..6_000_000);
    main.span("GameLogic::Update", main_time, update_dur, 1, 2);

    let num_entities = rng.random_range(10..30);
    main.split_n_with(
        "Entity_",
        num_entities,
        main_time,
        update_dur,
        2,
        3,
        |_, entity_time, entity_dur| {
            emit_entity_systems(&main, entity_time, entity_dur);
        },
    );
}

/// Component systems within one entity, with AI and animation sub-operations.
fn emit_entity_systems(main: &ThreadEmitter, entity_time: u64, entity_dur: u64) {
    main.split_evenly_with(
        &ENTITY_SYSTEMS,
        |system| format!("{}::Update", system),
        entity_time,
        entity_dur,
        3,
        4,
        |system, comp_time, system_dur| {
            // Deeper operations within systems
            if system == "AI" {
                main.split_evenly_with(
                    &AI_OPS,
                    prefixed("AI::"),
                    comp_time,
                    system_dur,
                    4,
                    5,
                    |op, ai_time, op_dur| {
                        // Even deeper for pathfinding
                        if op == "Pathfinding" {
                            main.split_evenly(
                                &PATHFINDING_OPS,
                                prefixed("Path::"),
                                ai_time,
                                op_dur,
                                5,
                                6,
                            );
                        }
                    },
                );
            } else if system == "Animation" {
                main.split_evenly(
                    &ANIMATION_OPS,
                    prefixed("Anim::"),
                    comp_time,
                    system_dur,
                    4,
                    5,
                );
            }
        },
    );
}

/// Thread 2: render thread with cull, sort, command-buffer build and submit.
fn emit_render_thread(trace: &TraceData, rng: &mut ThreadRng, frame_start: u64) {
    let render = ThreadEmitter::new(trace, THREAD_RENDER);
    let render_start = frame_start + rng.random_range(500_000..2_000_000);
    let render_dur = rng.random_range(4_000_000..10_000_000);

    render.span("RenderThread", render_start, render_dur, 0, 4);

    let mut render_time = render_start;
    for task in RENDER_TASKS {
        let task_dur = rng.random_range(500_000..2_500_000);
        render.span(format!("Render::{}", task), render_time, task_dur, 1, 5);

        // Deeper render operations
        if task == "Cull" {
            render.split_evenly_with(
                &CULL_OPS,
                prefixed("Cull::"),
                render_time,
                task_dur,
                2,
                6,
                |op, cull_time, cull_dur| {
                    // Octree traversal for frustum culling
                    if op == "FrustumCull" {
                        let num_nodes = rng.random_range(3..8);
                        render.split_n("Octree::Node", num_nodes, cull_time, cull_dur, 3, 7);
                    }
                },
            );
        } else if task == "BuildCmdBuffer" {
            let num_batches = rng.random_range(5..15);
            render.split_n_with(
                "Batch_",
                num_batches,
                render_time,
                task_dur,
                2,
                6,
                |_, cmd_time, batch_dur| {
                    // Individual draw commands
                    render.split_evenly(&COMMAND_OPS, prefixed("Cmd::"), cmd_time, batch_dur, 3, 7);
                },
            );
        }

        render_time += task_dur;
    }
}

/// Thread 3: physics thread with broad phase, narrow phase (GJK), constraint solve and integrate.
fn emit_physics_thread(trace: &TraceData, rng: &mut ThreadRng, frame_start: u64) {
    let physics = ThreadEmitter::new(trace, THREAD_PHYSICS);
    let physics_start = frame_start + rng.random_range(0..2_000_000);
    let physics_dur = rng.random_range(3_000_000..8_000_000);

    physics.span("PhysicsThread", physics_start, physics_dur, 0, 6);

    let mut phys_time = physics_start;
    for phase in PHYSICS_PHASES {
        let phase_dur = rng.random_range(500_000..2_000_000);
        physics.span(format!("Physics::{}", phase), phys_time, phase_dur, 1, 7);

        // Deeper physics operations
        match phase {
            "BroadPhase" => physics.split_evenly(
                &BROAD_PHASE_OPS,
                prefixed("Broad::"),
                phys_time,
                phase_dur,
                2,
                8,
            ),
            "NarrowPhase" => {
                let num_pairs = rng.random_range(8..20);
                physics.split_n_with(
                    "CollisionPair_",
                    num_pairs,
                    phys_time,
                    phase_dur,
                    2,
                    8,
                    |_, pair_time, pair_dur| {
                        // Collision detection pipeline
                        physics.split_evenly_with(
                            &COLLISION_OPS,
                            prefixed("Collision::"),
                            pair_time,
                            pair_dur,
                            3,
                            9,
                            |op, coll_time, coll_dur| {
                                // GJK algorithm steps
                                if op == "GJK" {
                                    physics.split_evenly(
                                        &GJK_STEPS,
                                        prefixed("GJK::"),
                                        coll_time,
                                        coll_dur,
                                        4,
                                        10,
                                    );
                                }
                            },
                        );
                    },
                );
            }
            "SolveConstraints" => {
                let num_islands = rng.random_range(3..8);
                physics.split_n_with(
                    "Island_",
                    num_islands,
                    phys_time,
                    phase_dur,
                    2,
                    8,
                    |_, island_time, island_dur| {
                        // Constraint solver iterations
                        let num_iterations = rng.random_range(4..10);
                        physics.split_n(
                            "SolverIter_",
                            num_iterations,
                            island_time,
                            island_dur,
                            3,
                            9,
                        );
                    },
                );
            }
            _ => {
                // Objects per phase for other phases
                let num_objs = rng.random_range(8..20);
                physics.split_n("Obj_", num_objs, phys_time, phase_dur, 2, 8);
            }
        }
        phys_time += phase_dur;
    }
}

/// Thread 4: audio thread with sequential mixing tasks of random length.
fn emit_audio_thread(trace: &TraceData, rng: &mut ThreadRng, frame_start: u64) {
    let audio = ThreadEmitter::new(trace, THREAD_AUDIO);
    let audio_start = frame_start + rng.random_range(0..1_000_000);
    let audio_dur = rng.random_range(1_000_000..3_000_000);

    audio.span("AudioThread", audio_start, audio_dur, 0, 9);

    let mut audio_time = audio_start;
    for task in AUDIO_TASKS {
        let task_dur = rng.random_range(200_000..800_000);
        audio.span(format!("Audio::{}", task), audio_time, task_dur, 1, 10);
        audio_time += task_dur;
    }
}

/// Thread 5: network thread (packets, events, updates).
fn emit_network_thread(trace: &TraceData, rng: &mut ThreadRng, frame_start: u64) {
    let net = ThreadEmitter::new(trace, THREAD_NETWORK);
    let net_start = frame_start + rng.random_range(0..5_000_000);
    let net_dur = rng.random_range(500_000..2_000_000);

    net.span("NetworkThread", net_start, net_dur, 0, 11);

    let mut net_time = net_start;
    for op in NETWORK_OPS {
        let op_dur = rng.random_range(100_000..500_000);
        net.span(format!("Net::{}", op), net_time, op_dur, 1, 12);
        net_time += op_dur;
    }
}

/// Thread 6: I/O thread with asset loading (decompress/parse), texture streaming and cache writes.
fn emit_io_thread(trace: &TraceData, rng: &mut ThreadRng, frame_start: u64) {
    let io = ThreadEmitter::new(trace, THREAD_IO);
    let io_start = frame_start + rng.random_range(0..8_000_000);
    let io_dur = rng.random_range(1_000_000..4_000_000);

    io.span("IOThread", io_start, io_dur, 0, 13);

    let mut io_time = io_start;
    for op in IO_OPS {
        let op_dur = rng.random_range(300_000..1_500_000);
        io.span(format!("IO::{}", op), io_time, op_dur, 1, 14);

        // Deeper I/O operations
        if op == "LoadAsset" {
            io.split_evenly_with(
                &LOAD_OPS,
                prefixed("Load::"),
                io_time,
                op_dur,
                2,
                15,
                |load_op, load_time, load_dur| {
                    if load_op == "Decompress" {
                        // Decompression pipeline
                        io.split_evenly(
                            &DECOMPRESS_OPS,
                            prefixed("Decomp::"),
                            load_time,
                            load_dur,
                            3,
                            0,
                        );
                    } else if load_op == "Parse" {
                        io.split_evenly(&PARSE_OPS, prefixed("Parse::"), load_time, load_dur, 3, 1);
                    }
                },
            );
        } else if op == "StreamTexture" {
            io.split_evenly_with(
                &STREAM_OPS,
                prefixed("Stream::"),
                io_time,
                op_dur,
                2,
                15,
                |stream_op, stream_time, stream_dur| {
                    // Transcoding steps
                    if stream_op == "Transcode" {
                        io.split_evenly(
                            &TRANSCODE_OPS,
                            prefixed("Transcode::"),
                            stream_time,
                            stream_dur,
                            3,
                            2,
                        );
                    }
                },
            );
        }

        io_time += op_dur;
    }
}

/// Threads 7-18: job-system workers running mesh, particle, compute and sort tasks.
fn emit_worker_threads(trace: &TraceData, rng: &mut ThreadRng, frame_start: u64) {
    for worker_id in 0..WORKER_COUNT {
        let worker = ThreadEmitter::new(trace, FIRST_WORKER_THREAD + worker_id);
        let job_start = frame_start + rng.random_range(0..3_000_000);
        let job_dur = rng.random_range(2_000_000..7_000_000);

        worker.span(format!("Worker_{}", worker_id), job_start, job_dur, 0, 15);

        // Parallel tasks with varying complexity
        let num_tasks = rng.random_range(5..12);
        let task_dur = job_dur / num_tasks as u64;
        let mut task_time = job_start;
        for task in 0..num_tasks {
            let task_name = WORKER_TASK_TYPES[task % WORKER_TASK_TYPES.len()];

            worker.span(
                format!("{}_{}", task_name, task),
                task_time,
                task_dur,
                1,
                ((worker_id + task as u64) % 16) as u8,
            );

            // Some tasks have deeper work
            if task_name == "MeshProcess" {
                worker.split_evenly(
                    &MESH_OPS,
                    prefixed("Mesh::"),
                    task_time,
                    task_dur,
                    2,
                    ((worker_id + 1) % 16) as u8,
                );
            } else if task_name == "ParticleSim" {
                worker.split_evenly(
                    &PARTICLE_OPS,
                    prefixed("Particle::"),
                    task_time,
                    task_dur,
                    2,
                    ((worker_id + 2) % 16) as u8,
                );
            }

            task_time += task_dur;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_of(trace: &TraceData) -> Vec<TraceSpan> {
        trace.publish_pending();
        trace.get_frame().spans.clone()
    }

    #[test]
    fn split_evenly_lays_children_back_to_back() {
        let trace = TraceData::new();
        let emitter = ThreadEmitter::new(&trace, 9);
        emitter.split_evenly(&["A", "B", "C"], prefixed("Op::"), 100, 30, 2, 5);

        let spans = spans_of(&trace);
        let summary: Vec<_> = spans
            .iter()
            .map(|s| {
                (
                    s.name.as_str(),
                    s.start_ns,
                    s.duration_ns,
                    s.depth,
                    s.thread_id,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("Op::A", 100, 10, 2, 9),
                ("Op::B", 110, 10, 2, 9),
                ("Op::C", 120, 10, 2, 9),
            ]
        );
    }

    #[test]
    fn split_evenly_with_reports_each_child_span() {
        let trace = TraceData::new();
        let emitter = ThreadEmitter::new(&trace, 0);
        let mut seen = Vec::new();
        emitter.split_evenly_with(
            &["X", "Y"],
            prefixed(""),
            0,
            10,
            1,
            0,
            |name, start, dur| seen.push((name.to_string(), start, dur)),
        );
        assert_eq!(seen, vec![("X".to_string(), 0, 5), ("Y".to_string(), 5, 5)]);
    }

    #[test]
    fn split_n_names_items_by_index() {
        let trace = TraceData::new();
        let emitter = ThreadEmitter::new(&trace, 0);
        emitter.split_n("Item_", 4, 1_000, 400, 3, 1);

        let spans = spans_of(&trace);
        let names: Vec<_> = spans.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Item_0", "Item_1", "Item_2", "Item_3"]);
        let starts: Vec<_> = spans.iter().map(|s| s.start_ns).collect();
        assert_eq!(starts, vec![1_000, 1_100, 1_200, 1_300]);
        assert!(spans.iter().all(|s| s.duration_ns == 100 && s.depth == 3));
    }

    #[test]
    fn sample_trace_covers_every_thread_and_frame() {
        let trace = TraceData::new();
        populate_sample_trace(&trace, 3);

        let frame = trace.get_frame();
        assert_eq!(frame.frame_times_ms.len(), 3);
        assert_eq!(frame.frame_boundaries_ns.len(), 3);
        // Frame 0 emits network and I/O work, so every thread 0..=18 has spans.
        for thread_id in 0..(FIRST_WORKER_THREAD + WORKER_COUNT) {
            assert!(
                frame.spans.iter().any(|s| s.thread_id == thread_id),
                "no spans for thread {}",
                thread_id
            );
        }
        assert!(frame.spans.iter().any(|s| s.name == "Frame 2"));
    }
}
