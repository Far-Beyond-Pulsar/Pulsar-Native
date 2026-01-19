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
                for draw in 0..num_draws {
                    trace.add_span(TraceSpan {
                        name: format!("Draw_{}", draw),
                        start_ns: gpu_time + draw as u64 * draw_dur,
                        duration_ns: draw_dur,
                        depth: 2,
                        thread_id: 0,
                        color_index: 12,
                    });
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

            // Game logic update
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
            for entity in 0..num_entities {
                trace.add_span(TraceSpan {
                    name: format!("Entity_{}", entity),
                    start_ns: main_time + entity as u64 * entity_dur,
                    duration_ns: entity_dur,
                    depth: 2,
                    thread_id: 1,
                    color_index: 3,
                });
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

                // Objects per phase
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

                // Parallel tasks
                let num_tasks = rng.gen_range(5..12);
                let task_dur = job_dur / num_tasks as u64;
                for task in 0..num_tasks {
                    trace.add_span(TraceSpan {
                        name: format!("Task_{}", task),
                        start_ns: job_start + task as u64 * task_dur,
                        duration_ns: task_dur,
                        depth: 1,
                        thread_id,
                        color_index: ((worker_id + task) % 16) as u8,
                    });
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

    pub fn clear(&self) {
        *self.inner.write() = Arc::new(TraceFrame::new());
    }
}

impl Default for TraceData {
    fn default() -> Self {
        Self::new()
    }
}