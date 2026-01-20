use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::HashMap;
use parking_lot::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSpan {
    pub name: String,
    pub start_ns: u64,
    pub duration_ns: u64,
    pub depth: u32,
    pub thread_id: u64,
    pub color_index: u8,
}

impl TraceSpan {
    pub fn end_ns(&self) -> u64 {
        self.start_ns + self.duration_ns
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadInfo {
    pub id: u64,
    pub name: String,
}

impl ThreadInfo {
    /// Check if this thread has a human-friendly name (not just "Thread N")
    pub fn has_custom_name(&self) -> bool {
        !self.name.starts_with("Thread ")
    }
    
    /// Get a sort priority (lower = earlier)
    /// Named threads come first, then unnamed threads sorted by ID
    pub fn sort_priority(&self) -> (bool, u64) {
        (!self.has_custom_name(), self.id)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TraceFrame {
    pub spans: Vec<TraceSpan>,
    pub min_time_ns: u64,
    pub max_time_ns: u64,
    pub max_depth: u32,
    pub threads: HashMap<u64, ThreadInfo>,
    pub frame_times_ms: Vec<f32>, // History of frame times
}

impl TraceFrame {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_data(spans: Vec<TraceSpan>, threads: HashMap<u64, String>) -> Self {
        let mut frame = Self::default();
        
        // Convert thread names HashMap to ThreadInfo HashMap
        for (id, name) in threads {
            frame.threads.insert(id, ThreadInfo { id, name });
        }
        
        // Add all spans
        for span in spans {
            frame.add_span(span);
        }
        
        frame
    }

    pub fn add_span(&mut self, span: TraceSpan) {
        if self.spans.is_empty() {
            self.min_time_ns = span.start_ns;
            self.max_time_ns = span.end_ns();
        } else {
            self.min_time_ns = self.min_time_ns.min(span.start_ns);
            self.max_time_ns = self.max_time_ns.max(span.end_ns());
        }
        self.max_depth = self.max_depth.max(span.depth);
        
        // Ensure thread exists
        if !self.threads.contains_key(&span.thread_id) {
            self.threads.insert(span.thread_id, ThreadInfo {
                id: span.thread_id,
                name: match span.thread_id {
                    0 => "GPU".to_string(),
                    1 => "Main Thread".to_string(),
                    2 => "Render Thread".to_string(),
                    3 => "Physics Thread".to_string(),
                    4 => "Audio Thread".to_string(),
                    5 => "Network Thread".to_string(),
                    6 => "I/O Thread".to_string(),
                    7..=10 => format!("Worker {}", span.thread_id - 7),
                    id => format!("Thread {}", id),
                },
            });
        }
        
        self.spans.push(span);
    }

    pub fn duration_ns(&self) -> u64 {
        if self.spans.is_empty() {
            0
        } else {
            self.max_time_ns - self.min_time_ns
        }
    }
    
    /// Get threads sorted with named threads first, then by ID
    pub fn get_sorted_threads(&self) -> Vec<ThreadInfo> {
        let mut threads: Vec<ThreadInfo> = self.threads.values().cloned().collect();
        threads.sort_by_key(|t| t.sort_priority());
        threads
    }
    
    pub fn add_frame_time(&mut self, ms: f32) {
        self.frame_times_ms.push(ms);
        // Keep last 200 frames
        if self.frame_times_ms.len() > 200 {
            self.frame_times_ms.remove(0);
        }
    }
}

#[derive(Clone)]
pub struct TraceData {
    inner: Arc<RwLock<Arc<TraceFrame>>>,
}

impl TraceData {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Arc::new(TraceFrame::new()))),
        }
    }

    /// Create TraceData with comprehensive sample data
    /// Generates 2000+ frames with dedicated threads for engine subsystems
    pub fn with_sample_data() -> Self {
        use rand::Rng;
        let trace = Self::new();
        let mut rng = rand::thread_rng();
        let mut current_time = 0u64;

        // Generate 2000 frames with realistic multi-threaded workload
        for frame_idx in 0..2000 {
            let frame_start = current_time;
            let base_frame_time = 16_600_000u64; // 16.6ms target
            let frame_variance = rng.gen_range(-3_000_000i64..5_000_000i64);
            let frame_duration = (base_frame_time as i64 + frame_variance).max(8_000_000) as u64;

            trace.add_frame_time(frame_duration as f32 / 1_000_000.0);

            // === THREAD 0: GPU ===
            let gpu_start = frame_start + rng.gen_range(1_000_000..3_000_000);
            let gpu_duration = rng.gen_range(8_000_000..14_000_000);

            trace.add_span(TraceSpan {
                name: "GPU Frame".to_string(),
                start_ns: gpu_start,
                duration_ns: gpu_duration,
                depth: 0,
                thread_id: 0,
                color_index: 10,
            });

            let gpu_passes = [
                ("ShadowPass", 1_500_000, 3_000_000),
                ("GeometryPass", 2_000_000, 4_000_000),
                ("LightingPass", 2_500_000, 4_500_000),
                ("PostProcess", 1_000_000, 2_500_000),
                ("Present", 500_000, 1_000_000),
            ];

            let mut gpu_time = gpu_start;
            for (pass_name, min_dur, max_dur) in gpu_passes {
                let pass_dur = rng.gen_range(min_dur..max_dur);
                trace.add_span(TraceSpan {
                    name: pass_name.to_string(),
                    start_ns: gpu_time,
                    duration_ns: pass_dur,
                    depth: 1,
                    thread_id: 0,
                    color_index: 11,
                });

                // Draw calls per pass
                let num_draws = rng.gen_range(5..20);
                let draw_dur = pass_dur / num_draws as u64;
                let mut draw_time = gpu_time;
                for draw in 0..num_draws {
                    trace.add_span(TraceSpan {
                        name: format!("Draw_{}", draw),
                        start_ns: draw_time,
                        duration_ns: draw_dur,
                        depth: 2,
                        thread_id: 0,
                        color_index: 12,
                    });

                    // Shader stages within draw calls
                    if pass_name == "GeometryPass" || pass_name == "LightingPass" {
                        let shader_stages = ["VertexShader", "GeometryShader", "FragmentShader"];
                        let mut shader_time = draw_time;
                        let stage_dur = draw_dur / shader_stages.len() as u64;
                        for stage in &shader_stages {
                            trace.add_span(TraceSpan {
                                name: format!("{}", stage),
                                start_ns: shader_time,
                                duration_ns: stage_dur,
                                depth: 3,
                                thread_id: 0,
                                color_index: 13,
                            });

                            // Texture fetches in fragment shader
                            if *stage == "FragmentShader" && pass_name == "LightingPass" {
                                let fetch_ops = ["SampleAlbedo", "SampleNormal", "SampleMetallic"];
                                let mut fetch_time = shader_time;
                                let fetch_dur = stage_dur / fetch_ops.len() as u64;
                                for fetch_op in &fetch_ops {
                                    trace.add_span(TraceSpan {
                                        name: format!("Tex::{}", fetch_op),
                                        start_ns: fetch_time,
                                        duration_ns: fetch_dur,
                                        depth: 4,
                                        thread_id: 0,
                                        color_index: 14,
                                    });
                                    fetch_time += fetch_dur;
                                }
                            }
                            shader_time += stage_dur;
                        }
                    }

                    draw_time += draw_dur;
                }
                gpu_time += pass_dur;
            }

            // === THREAD 1: MAIN THREAD ===
            trace.add_span(TraceSpan {
                name: format!("Frame {}", frame_idx),
                start_ns: frame_start,
                duration_ns: frame_duration,
                depth: 0,
                thread_id: 1,
                color_index: 0,
            });

            let mut main_time = frame_start;

            // Input processing
            let input_dur = rng.gen_range(100_000..500_000);
            trace.add_span(TraceSpan {
                name: "Input::Process".to_string(),
                start_ns: main_time,
                duration_ns: input_dur,
                depth: 1,
                thread_id: 1,
                color_index: 1,
            });
            main_time += input_dur;

            // Game logic update with deeper callstacks
            let update_dur = rng.gen_range(2_000_000..6_000_000);
            trace.add_span(TraceSpan {
                name: "GameLogic::Update".to_string(),
                start_ns: main_time,
                duration_ns: update_dur,
                depth: 1,
                thread_id: 1,
                color_index: 2,
            });

            let num_entities = rng.gen_range(10..30);
            let entity_dur = update_dur / num_entities as u64;
            let mut entity_time = main_time;
            for entity in 0..num_entities {
                // Entity update
                trace.add_span(TraceSpan {
                    name: format!("Entity_{}", entity),
                    start_ns: entity_time,
                    duration_ns: entity_dur,
                    depth: 2,
                    thread_id: 1,
                    color_index: 3,
                });

                // Component systems within each entity
                let systems = ["Transform", "Physics", "Animation", "AI"];
                let mut comp_time = entity_time;
                let system_dur = entity_dur / systems.len() as u64;
                for system in &systems {
                    trace.add_span(TraceSpan {
                        name: format!("{}::Update", system),
                        start_ns: comp_time,
                        duration_ns: system_dur,
                        depth: 3,
                        thread_id: 1,
                        color_index: 4,
                    });

                    // Deeper operations within systems
                    if *system == "AI" {
                        let mut ai_time = comp_time;
                        let ai_ops = ["Perception", "DecisionTree", "Pathfinding", "Behavior"];
                        let op_dur = system_dur / ai_ops.len() as u64;
                        for op in &ai_ops {
                            trace.add_span(TraceSpan {
                                name: format!("AI::{}", op),
                                start_ns: ai_time,
                                duration_ns: op_dur,
                                depth: 4,
                                thread_id: 1,
                                color_index: 5,
                            });

                            // Even deeper for pathfinding
                            if *op == "Pathfinding" {
                                let path_ops = ["BuildGraph", "AStar", "SmoothPath"];
                                let mut path_time = ai_time;
                                let path_dur = op_dur / path_ops.len() as u64;
                                for path_op in &path_ops {
                                    trace.add_span(TraceSpan {
                                        name: format!("Path::{}", path_op),
                                        start_ns: path_time,
                                        duration_ns: path_dur,
                                        depth: 5,
                                        thread_id: 1,
                                        color_index: 6,
                                    });
                                    path_time += path_dur;
                                }
                            }
                            ai_time += op_dur;
                        }
                    } else if *system == "Animation" {
                        let mut anim_time = comp_time;
                        let anim_ops = ["Sample", "Blend", "IK", "ApplyPose"];
                        let op_dur = system_dur / anim_ops.len() as u64;
                        for op in &anim_ops {
                            trace.add_span(TraceSpan {
                                name: format!("Anim::{}", op),
                                start_ns: anim_time,
                                duration_ns: op_dur,
                                depth: 4,
                                thread_id: 1,
                                color_index: 5,
                            });
                            anim_time += op_dur;
                        }
                    }

                    comp_time += system_dur;
                }
                entity_time += entity_dur;
            }
            main_time += update_dur;

            // === THREAD 2: RENDER THREAD ===
            let render_start = frame_start + rng.gen_range(500_000..2_000_000);
            let render_dur = rng.gen_range(4_000_000..10_000_000);

            trace.add_span(TraceSpan {
                name: "RenderThread".to_string(),
                start_ns: render_start,
                duration_ns: render_dur,
                depth: 0,
                thread_id: 2,
                color_index: 4,
            });

            let render_tasks = ["Cull", "Sort", "BuildCmdBuffer", "Submit"];
            let mut render_time = render_start;
            for task in render_tasks {
                let task_dur = rng.gen_range(500_000..2_500_000);
                trace.add_span(TraceSpan {
                    name: format!("Render::{}", task),
                    start_ns: render_time,
                    duration_ns: task_dur,
                    depth: 1,
                    thread_id: 2,
                    color_index: 5,
                });

                // Deeper render operations
                if task == "Cull" {
                    let mut cull_time = render_time;
                    let cull_ops = ["FrustumCull", "OcclusionQuery", "LODSelect"];
                    let cull_dur = task_dur / cull_ops.len() as u64;
                    for op in &cull_ops {
                        trace.add_span(TraceSpan {
                            name: format!("Cull::{}", op),
                            start_ns: cull_time,
                            duration_ns: cull_dur,
                            depth: 2,
                            thread_id: 2,
                            color_index: 6,
                        });

                        // Octree traversal for frustum culling
                        if *op == "FrustumCull" {
                            let num_nodes = rng.gen_range(3..8);
                            let node_dur = cull_dur / num_nodes as u64;
                            for node in 0..num_nodes {
                                trace.add_span(TraceSpan {
                                    name: format!("Octree::Node{}", node),
                                    start_ns: cull_time + node as u64 * node_dur,
                                    duration_ns: node_dur,
                                    depth: 3,
                                    thread_id: 2,
                                    color_index: 7,
                                });
                            }
                        }
                        cull_time += cull_dur;
                    }
                } else if task == "BuildCmdBuffer" {
                    let mut cmd_time = render_time;
                    let num_batches = rng.gen_range(5..15);
                    let batch_dur = task_dur / num_batches as u64;
                    for batch in 0..num_batches {
                        trace.add_span(TraceSpan {
                            name: format!("Batch_{}", batch),
                            start_ns: cmd_time,
                            duration_ns: batch_dur,
                            depth: 2,
                            thread_id: 2,
                            color_index: 6,
                        });

                        // Individual draw commands
                        let cmd_ops = ["SetPipeline", "BindDescriptors", "SetConstants", "DrawIndexed"];
                        let mut draw_time = cmd_time;
                        let draw_dur = batch_dur / cmd_ops.len() as u64;
                        for cmd_op in &cmd_ops {
                            trace.add_span(TraceSpan {
                                name: format!("Cmd::{}", cmd_op),
                                start_ns: draw_time,
                                duration_ns: draw_dur,
                                depth: 3,
                                thread_id: 2,
                                color_index: 7,
                            });
                            draw_time += draw_dur;
                        }
                        cmd_time += batch_dur;
                    }
                }

                render_time += task_dur;
            }

            // === THREAD 3: PHYSICS THREAD ===
            let physics_start = frame_start + rng.gen_range(0..2_000_000);
            let physics_dur = rng.gen_range(3_000_000..8_000_000);

            trace.add_span(TraceSpan {
                name: "PhysicsThread".to_string(),
                start_ns: physics_start,
                duration_ns: physics_dur,
                depth: 0,
                thread_id: 3,
                color_index: 6,
            });

            let physics_phases = ["BroadPhase", "NarrowPhase", "SolveConstraints", "Integrate"];
            let mut phys_time = physics_start;
            for phase in physics_phases {
                let phase_dur = rng.gen_range(500_000..2_000_000);
                trace.add_span(TraceSpan {
                    name: format!("Physics::{}", phase),
                    start_ns: phys_time,
                    duration_ns: phase_dur,
                    depth: 1,
                    thread_id: 3,
                    color_index: 7,
                });

                // Deeper physics operations
                if phase == "BroadPhase" {
                    let mut broad_time = phys_time;
                    let broad_ops = ["UpdateAABBs", "SortAndSweep", "BuildPairs"];
                    let broad_dur = phase_dur / broad_ops.len() as u64;
                    for op in &broad_ops {
                        trace.add_span(TraceSpan {
                            name: format!("Broad::{}", op),
                            start_ns: broad_time,
                            duration_ns: broad_dur,
                            depth: 2,
                            thread_id: 3,
                            color_index: 8,
                        });
                        broad_time += broad_dur;
                    }
                } else if phase == "NarrowPhase" {
                    let num_pairs = rng.gen_range(8..20);
                    let pair_dur = phase_dur / num_pairs as u64;
                    let mut pair_time = phys_time;
                    for pair in 0..num_pairs {
                        trace.add_span(TraceSpan {
                            name: format!("CollisionPair_{}", pair),
                            start_ns: pair_time,
                            duration_ns: pair_dur,
                            depth: 2,
                            thread_id: 3,
                            color_index: 8,
                        });

                        // Collision detection pipeline
                        let coll_ops = ["GJK", "EPA", "ContactGen", "Manifold"];
                        let mut coll_time = pair_time;
                        let coll_dur = pair_dur / coll_ops.len() as u64;
                        for op in &coll_ops {
                            trace.add_span(TraceSpan {
                                name: format!("Collision::{}", op),
                                start_ns: coll_time,
                                duration_ns: coll_dur,
                                depth: 3,
                                thread_id: 3,
                                color_index: 9,
                            });

                            // GJK algorithm steps
                            if *op == "GJK" {
                                let gjk_steps = ["Support", "Simplex", "ClosestPoint"];
                                let mut gjk_time = coll_time;
                                let gjk_dur = coll_dur / gjk_steps.len() as u64;
                                for step in &gjk_steps {
                                    trace.add_span(TraceSpan {
                                        name: format!("GJK::{}", step),
                                        start_ns: gjk_time,
                                        duration_ns: gjk_dur,
                                        depth: 4,
                                        thread_id: 3,
                                        color_index: 10,
                                    });
                                    gjk_time += gjk_dur;
                                }
                            }
                            coll_time += coll_dur;
                        }
                        pair_time += pair_dur;
                    }
                } else if phase == "SolveConstraints" {
                    let num_islands = rng.gen_range(3..8);
                    let island_dur = phase_dur / num_islands as u64;
                    let mut island_time = phys_time;
                    for island in 0..num_islands {
                        trace.add_span(TraceSpan {
                            name: format!("Island_{}", island),
                            start_ns: island_time,
                            duration_ns: island_dur,
                            depth: 2,
                            thread_id: 3,
                            color_index: 8,
                        });

                        // Constraint solver iterations
                        let num_iterations = rng.gen_range(4..10);
                        let iter_dur = island_dur / num_iterations as u64;
                        let mut iter_time = island_time;
                        for iter in 0..num_iterations {
                            trace.add_span(TraceSpan {
                                name: format!("SolverIter_{}", iter),
                                start_ns: iter_time,
                                duration_ns: iter_dur,
                                depth: 3,
                                thread_id: 3,
                                color_index: 9,
                            });
                            iter_time += iter_dur;
                        }
                        island_time += island_dur;
                    }
                } else {
                    // Objects per phase for other phases
                    let num_objs = rng.gen_range(8..20);
                    let obj_dur = phase_dur / num_objs as u64;
                    for obj in 0..num_objs {
                        trace.add_span(TraceSpan {
                            name: format!("Obj_{}", obj),
                            start_ns: phys_time + obj as u64 * obj_dur,
                            duration_ns: obj_dur,
                            depth: 2,
                            thread_id: 3,
                            color_index: 8,
                        });
                    }
                }
                phys_time += phase_dur;
            }

            // === THREAD 4: AUDIO THREAD ===
            let audio_start = frame_start + rng.gen_range(0..1_000_000);
            let audio_dur = rng.gen_range(1_000_000..3_000_000);

            trace.add_span(TraceSpan {
                name: "AudioThread".to_string(),
                start_ns: audio_start,
                duration_ns: audio_dur,
                depth: 0,
                thread_id: 4,
                color_index: 9,
            });

            let audio_tasks = ["MixChannels", "ApplyEffects", "StreamDecode", "Output"];
            let mut audio_time = audio_start;
            for task in audio_tasks {
                let task_dur = rng.gen_range(200_000..800_000);
                trace.add_span(TraceSpan {
                    name: format!("Audio::{}", task),
                    start_ns: audio_time,
                    duration_ns: task_dur,
                    depth: 1,
                    thread_id: 4,
                    color_index: 10,
                });
                audio_time += task_dur;
            }

            // === THREAD 5: NETWORK THREAD ===
            if frame_idx % 3 == 0 { // Network updates every 3rd frame
                let net_start = frame_start + rng.gen_range(0..5_000_000);
                let net_dur = rng.gen_range(500_000..2_000_000);

                trace.add_span(TraceSpan {
                    name: "NetworkThread".to_string(),
                    start_ns: net_start,
                    duration_ns: net_dur,
                    depth: 0,
                    thread_id: 5,
                    color_index: 11,
                });

                let net_ops = ["RecvPackets", "ProcessEvents", "SendUpdates", "Serialize"];
                let mut net_time = net_start;
                for op in net_ops {
                    let op_dur = rng.gen_range(100_000..500_000);
                    trace.add_span(TraceSpan {
                        name: format!("Net::{}", op),
                        start_ns: net_time,
                        duration_ns: op_dur,
                        depth: 1,
                        thread_id: 5,
                        color_index: 12,
                    });
                    net_time += op_dur;
                }
            }

            // === THREAD 6: I/O THREAD ===
            if frame_idx % 5 == 0 { // I/O operations every 5th frame
                let io_start = frame_start + rng.gen_range(0..8_000_000);
                let io_dur = rng.gen_range(1_000_000..4_000_000);

                trace.add_span(TraceSpan {
                    name: "IOThread".to_string(),
                    start_ns: io_start,
                    duration_ns: io_dur,
                    depth: 0,
                    thread_id: 6,
                    color_index: 13,
                });

                let io_ops = ["LoadAsset", "StreamTexture", "WriteCache"];
                let mut io_time = io_start;
                for op in io_ops {
                    let op_dur = rng.gen_range(300_000..1_500_000);
                    trace.add_span(TraceSpan {
                        name: format!("IO::{}", op),
                        start_ns: io_time,
                        duration_ns: op_dur,
                        depth: 1,
                        thread_id: 6,
                        color_index: 14,
                    });

                    // Deeper I/O operations
                    if op == "LoadAsset" {
                        let load_ops = ["OpenFile", "ReadData", "Decompress", "Parse", "Validate"];
                        let mut load_time = io_time;
                        let load_dur = op_dur / load_ops.len() as u64;
                        for load_op in &load_ops {
                            trace.add_span(TraceSpan {
                                name: format!("Load::{}", load_op),
                                start_ns: load_time,
                                duration_ns: load_dur,
                                depth: 2,
                                thread_id: 6,
                                color_index: 15,
                            });

                            // Decompression pipeline
                            if *load_op == "Decompress" {
                                let decomp_ops = ["ReadHeader", "InflateBlocks", "CRC32"];
                                let mut decomp_time = load_time;
                                let decomp_dur = load_dur / decomp_ops.len() as u64;
                                for decomp_op in &decomp_ops {
                                    trace.add_span(TraceSpan {
                                        name: format!("Decomp::{}", decomp_op),
                                        start_ns: decomp_time,
                                        duration_ns: decomp_dur,
                                        depth: 3,
                                        thread_id: 6,
                                        color_index: 0,
                                    });
                                    decomp_time += decomp_dur;
                                }
                            } else if *load_op == "Parse" {
                                let parse_ops = ["ReadChunks", "BuildScene", "LinkRefs"];
                                let mut parse_time = load_time;
                                let parse_dur = load_dur / parse_ops.len() as u64;
                                for parse_op in &parse_ops {
                                    trace.add_span(TraceSpan {
                                        name: format!("Parse::{}", parse_op),
                                        start_ns: parse_time,
                                        duration_ns: parse_dur,
                                        depth: 3,
                                        thread_id: 6,
                                        color_index: 1,
                                    });
                                    parse_time += parse_dur;
                                }
                            }
                            load_time += load_dur;
                        }
                    } else if op == "StreamTexture" {
                        let stream_ops = ["FetchMip", "Transcode", "Upload"];
                        let mut stream_time = io_time;
                        let stream_dur = op_dur / stream_ops.len() as u64;
                        for stream_op in &stream_ops {
                            trace.add_span(TraceSpan {
                                name: format!("Stream::{}", stream_op),
                                start_ns: stream_time,
                                duration_ns: stream_dur,
                                depth: 2,
                                thread_id: 6,
                                color_index: 15,
                            });

                            // Transcoding steps
                            if *stream_op == "Transcode" {
                                let transcode_ops = ["BCDecode", "ConvertFormat", "GenMipmaps"];
                                let mut transcode_time = stream_time;
                                let transcode_dur = stream_dur / transcode_ops.len() as u64;
                                for transcode_op in &transcode_ops {
                                    trace.add_span(TraceSpan {
                                        name: format!("Transcode::{}", transcode_op),
                                        start_ns: transcode_time,
                                        duration_ns: transcode_dur,
                                        depth: 3,
                                        thread_id: 6,
                                        color_index: 2,
                                    });
                                    transcode_time += transcode_dur;
                                }
                            }
                            stream_time += stream_dur;
                        }
                    }

                    io_time += op_dur;
                }
            }

            // === THREADS 7-18: JOB SYSTEM WORKERS ===
            for worker_id in 0..12 {
                let thread_id = 7 + worker_id;
                let job_start = frame_start + rng.gen_range(0..3_000_000);
                let job_dur = rng.gen_range(2_000_000..7_000_000);

                trace.add_span(TraceSpan {
                    name: format!("Worker_{}", worker_id),
                    start_ns: job_start,
                    duration_ns: job_dur,
                    depth: 0,
                    thread_id,
                    color_index: 15,
                });

                // Parallel tasks with varying complexity
                let num_tasks = rng.gen_range(5..12);
                let task_dur = job_dur / num_tasks as u64;
                let mut task_time = job_start;
                for task in 0..num_tasks {
                    let task_type = ["MeshProcess", "ParticleSim", "ComputeJob", "DataSort"];
                    let task_name = task_type[task % task_type.len()];

                    trace.add_span(TraceSpan {
                        name: format!("{}_{}", task_name, task),
                        start_ns: task_time,
                        duration_ns: task_dur,
                        depth: 1,
                        thread_id,
                        color_index: ((worker_id + task as u64) % 16) as u8,
                    });

                    // Some tasks have deeper work
                    if task_name == "MeshProcess" {
                        let mesh_ops = ["GenNormals", "ComputeTangents", "Optimize"];
                        let mut mesh_time = task_time;
                        let mesh_dur = task_dur / mesh_ops.len() as u64;
                        for mesh_op in &mesh_ops {
                            trace.add_span(TraceSpan {
                                name: format!("Mesh::{}", mesh_op),
                                start_ns: mesh_time,
                                duration_ns: mesh_dur,
                                depth: 2,
                                thread_id,
                                color_index: ((worker_id + 1) % 16) as u8,
                            });
                            mesh_time += mesh_dur;
                        }
                    } else if task_name == "ParticleSim" {
                        let particle_ops = ["UpdateForces", "Integrate", "Collide", "Sort"];
                        let mut particle_time = task_time;
                        let particle_dur = task_dur / particle_ops.len() as u64;
                        for particle_op in &particle_ops {
                            trace.add_span(TraceSpan {
                                name: format!("Particle::{}", particle_op),
                                start_ns: particle_time,
                                duration_ns: particle_dur,
                                depth: 2,
                                thread_id,
                                color_index: ((worker_id + 2) % 16) as u8,
                            });
                            particle_time += particle_dur;
                        }
                    }

                    task_time += task_dur;
                }
            }

            current_time += frame_duration;
        }

        let total_spans = trace.get_frame().spans.len();
        println!("Generated sample trace: {} spans across 2000 frames", total_spans);

        trace
    }

    pub fn add_span(&self, span: TraceSpan) {
        let mut guard = self.inner.write();
        Arc::make_mut(&mut guard).add_span(span);
    }

    pub fn add_frame_time(&self, ms: f32) {
        let mut guard = self.inner.write();
        Arc::make_mut(&mut guard).add_frame_time(ms);
    }

    pub fn get_frame(&self) -> Arc<TraceFrame> {
        Arc::clone(&self.inner.read())
    }

    pub fn set_frame(&self, frame: TraceFrame) {
        *self.inner.write() = Arc::new(frame);
    }

    pub fn clear(&self) {
        *self.inner.write() = Arc::new(TraceFrame::new());
    }
}

impl Default for TraceData {
    fn default() -> Self {
        Self::new()
    }
}