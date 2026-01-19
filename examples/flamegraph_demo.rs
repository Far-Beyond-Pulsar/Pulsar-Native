//! Flamegraph Demo
//! 
//! Demonstrates the high-performance flamegraph viewer with thousands of spans

use gpui::*;
use ui_flamegraph::{FlamegraphWindow, TraceData, TraceSpan};
use rand::Rng;

fn generate_demo_trace(trace: &TraceData) {
    let mut rng = rand::thread_rng();
    let mut current_time = 0u64;

    // Realistic function names for various systems
    let rendering_fns = ["draw_meshes", "update_transforms", "cull_objects", "render_shadows",
                         "post_process", "composite", "update_uniforms", "bind_textures"];
    let physics_fns = ["broad_phase", "narrow_phase", "solve_constraints", "integrate_velocities",
                       "update_aabb", "detect_collisions", "resolve_contacts"];
    let audio_fns = ["mix_channels", "apply_effects", "stream_decode", "spatial_audio",
                     "update_listener", "process_reverb", "compress_audio"];
    let network_fns = ["recv_packets", "send_updates", "serialize_state", "deserialize_events",
                       "compress_data", "encrypt_packet", "handle_ack"];
    let ai_fns = ["pathfind", "update_behavior_tree", "evaluate_goals", "sense_environment",
                  "make_decision", "execute_action", "update_navmesh"];
    let ui_fns = ["layout_widgets", "handle_events", "render_text", "update_animations",
                  "process_input", "draw_primitives", "batch_draw_calls"];

    // Generate 2000 frames with varying complexity
    for frame_idx in 0..2000 {
        let frame_start = current_time;

        // Vary frame time to simulate performance variation
        let base_frame_time = 16_600_000u64; // 16.6ms
        let frame_variance = rng.gen_range(-3_000_000i64..5_000_000i64);
        let frame_duration = (base_frame_time as i64 + frame_variance).max(8_000_000) as u64;

        // Record frame time for graph
        trace.add_frame_time(frame_duration as f32 / 1_000_000.0);

        // Main thread frame
        trace.add_span(TraceSpan {
            name: format!("Frame {}", frame_idx),
            start_ns: frame_start,
            duration_ns: frame_duration,
            depth: 0,
            thread_id: 1,
            color_index: (frame_idx % 16) as u8,
        });

        let mut main_time = frame_start;

        // === MAIN THREAD WORK ===

        // Input Processing
        let input_duration = rng.gen_range(100_000..500_000);
        trace.add_span(TraceSpan {
            name: "process_input".to_string(),
            start_ns: main_time,
            duration_ns: input_duration,
            depth: 1,
            thread_id: 1,
            color_index: 0,
        });
        main_time += input_duration;

        // UI System with detail
        let ui_duration = rng.gen_range(1_000_000..3_000_000);
        trace.add_span(TraceSpan {
            name: "UI::update".to_string(),
            start_ns: main_time,
            duration_ns: ui_duration,
            depth: 1,
            thread_id: 1,
            color_index: 1,
        });

        let mut ui_time = main_time;
        for (i, ui_fn) in ui_fns.iter().enumerate().take(rng.gen_range(3..6)) {
            let dur = rng.gen_range(50_000..400_000);
            trace.add_span(TraceSpan {
                name: format!("UI::{}", ui_fn),
                start_ns: ui_time,
                duration_ns: dur,
                depth: 2,
                thread_id: 1,
                color_index: 1,
            });

            // Add nested calls
            if rng.gen_bool(0.4) {
                let nested_dur = dur / 3;
                trace.add_span(TraceSpan {
                    name: format!("{}::inner", ui_fn),
                    start_ns: ui_time + dur / 3,
                    duration_ns: nested_dur,
                    depth: 3,
                    thread_id: 1,
                    color_index: 2,
                });
            }
            ui_time += dur;
        }
        main_time += ui_duration;

        // Physics System with deep nesting
        let physics_duration = rng.gen_range(2_000_000..6_000_000);
        trace.add_span(TraceSpan {
            name: "Physics::simulate".to_string(),
            start_ns: main_time,
            duration_ns: physics_duration,
            depth: 1,
            thread_id: 1,
            color_index: 3,
        });

        let mut phys_time = main_time;
        for (i, phys_fn) in physics_fns.iter().enumerate() {
            let dur = rng.gen_range(200_000..900_000);
            trace.add_span(TraceSpan {
                name: format!("Physics::{}", phys_fn),
                start_ns: phys_time,
                duration_ns: dur,
                depth: 2,
                thread_id: 1,
                color_index: 3,
            });

            // Deep nesting for complex physics
            if i < 4 {
                let num_objects = rng.gen_range(5..15);
                let obj_dur = dur / num_objects as u64;
                for obj in 0..num_objects {
                    trace.add_span(TraceSpan {
                        name: format!("{}::obj_{}", phys_fn, obj),
                        start_ns: phys_time + obj as u64 * obj_dur,
                        duration_ns: obj_dur,
                        depth: 3,
                        thread_id: 1,
                        color_index: 4,
                    });

                    // Even deeper
                    if rng.gen_bool(0.3) {
                        trace.add_span(TraceSpan {
                            name: format!("calculate"),
                            start_ns: phys_time + obj as u64 * obj_dur + obj_dur / 4,
                            duration_ns: obj_dur / 2,
                            depth: 4,
                            thread_id: 1,
                            color_index: 5,
                        });
                    }
                }
            }
            phys_time += dur;
        }
        main_time += physics_duration;

        // AI System
        let ai_duration = rng.gen_range(1_000_000..4_000_000);
        trace.add_span(TraceSpan {
            name: "AI::update".to_string(),
            start_ns: main_time,
            duration_ns: ai_duration,
            depth: 1,
            thread_id: 1,
            color_index: 6,
        });

        let mut ai_time = main_time;
        for ai_fn in ai_fns.iter().take(rng.gen_range(3..7)) {
            let dur = rng.gen_range(100_000..700_000);
            trace.add_span(TraceSpan {
                name: format!("AI::{}", ai_fn),
                start_ns: ai_time,
                duration_ns: dur,
                depth: 2,
                thread_id: 1,
                color_index: 6,
            });
            ai_time += dur;
        }
        main_time += ai_duration;

        // Audio System
        let audio_duration = rng.gen_range(500_000..2_000_000);
        trace.add_span(TraceSpan {
            name: "Audio::process".to_string(),
            start_ns: main_time,
            duration_ns: audio_duration,
            depth: 1,
            thread_id: 1,
            color_index: 7,
        });

        let mut audio_time = main_time;
        for audio_fn in audio_fns.iter().take(rng.gen_range(2..5)) {
            let dur = rng.gen_range(80_000..500_000);
            trace.add_span(TraceSpan {
                name: format!("Audio::{}", audio_fn),
                start_ns: audio_time,
                duration_ns: dur,
                depth: 2,
                thread_id: 1,
                color_index: 7,
            });
            audio_time += dur;
        }
        main_time += audio_duration;

        // Rendering System
        let render_duration = rng.gen_range(3_000_000..8_000_000);
        trace.add_span(TraceSpan {
            name: "Renderer::render".to_string(),
            start_ns: main_time,
            duration_ns: render_duration,
            depth: 1,
            thread_id: 1,
            color_index: 8,
        });

        let mut render_time = main_time;
        for (i, render_fn) in rendering_fns.iter().enumerate() {
            let dur = rng.gen_range(300_000..1_200_000);
            trace.add_span(TraceSpan {
                name: format!("Renderer::{}", render_fn),
                start_ns: render_time,
                duration_ns: dur,
                depth: 2,
                thread_id: 1,
                color_index: 8,
            });

            // Rendering often has batches
            if i % 2 == 0 {
                let batch_count = rng.gen_range(3..12);
                let batch_dur = dur / batch_count as u64;
                for batch in 0..batch_count {
                    trace.add_span(TraceSpan {
                        name: format!("batch_{}", batch),
                        start_ns: render_time + batch as u64 * batch_dur,
                        duration_ns: batch_dur,
                        depth: 3,
                        thread_id: 1,
                        color_index: 9,
                    });
                }
            }
            render_time += dur;
        }

        // === GPU THREAD ===
        let gpu_start = frame_start + rng.gen_range(2_000_000..4_000_000);
        let gpu_duration = rng.gen_range(8_000_000..14_000_000);

        trace.add_span(TraceSpan {
            name: "GPU Frame".to_string(),
            start_ns: gpu_start,
            duration_ns: gpu_duration,
            depth: 0,
            thread_id: 0,
            color_index: 10,
        });

        let mut gpu_time = gpu_start;
        let gpu_passes = ["ShadowPass", "GeometryPass", "LightingPass", "PostProcess", "Present"];
        for (i, pass) in gpu_passes.iter().enumerate() {
            let dur = rng.gen_range(1_000_000..3_000_000);
            trace.add_span(TraceSpan {
                name: format!("GPU::{}", pass),
                start_ns: gpu_time,
                duration_ns: dur,
                depth: 1,
                thread_id: 0,
                color_index: 10 + i as u8,
            });

            // Draw calls
            let draw_count = rng.gen_range(5..20);
            let draw_dur = dur / draw_count as u64;
            for draw in 0..draw_count {
                trace.add_span(TraceSpan {
                    name: format!("DrawCall_{}", draw),
                    start_ns: gpu_time + draw as u64 * draw_dur,
                    duration_ns: draw_dur,
                    depth: 2,
                    thread_id: 0,
                    color_index: 11,
                });
            }
            gpu_time += dur;
        }

        // === WORKER THREADS ===
        for worker_id in 2..6 {
            let worker_start = frame_start + rng.gen_range(0..2_000_000);
            let worker_duration = rng.gen_range(5_000_000..12_000_000);

            trace.add_span(TraceSpan {
                name: format!("Worker Job {}", worker_id - 2),
                start_ns: worker_start,
                duration_ns: worker_duration,
                depth: 0,
                thread_id: worker_id,
                color_index: (worker_id % 16) as u8,
            });

            // Parallel tasks
            let task_count = rng.gen_range(4..10);
            let mut task_time = worker_start;
            for task in 0..task_count {
                let task_dur = rng.gen_range(500_000..2_000_000);
                trace.add_span(TraceSpan {
                    name: format!("parallel_task_{}", task),
                    start_ns: task_time,
                    duration_ns: task_dur,
                    depth: 1,
                    thread_id: worker_id,
                    color_index: ((worker_id + task) % 16) as u8,
                });
                task_time += task_dur;
            }
        }

        // Network updates (occasional)
        if frame_idx % 3 == 0 {
            let net_start = main_time;
            let net_duration = rng.gen_range(200_000..1_500_000);
            trace.add_span(TraceSpan {
                name: "Network::update".to_string(),
                start_ns: net_start,
                duration_ns: net_duration,
                depth: 1,
                thread_id: 1,
                color_index: 15,
            });

            let mut net_time = net_start;
            for net_fn in network_fns.iter().take(rng.gen_range(2..5)) {
                let dur = rng.gen_range(50_000..300_000);
                trace.add_span(TraceSpan {
                    name: format!("Net::{}", net_fn),
                    start_ns: net_time,
                    duration_ns: dur,
                    depth: 2,
                    thread_id: 1,
                    color_index: 15,
                });
                net_time += dur;
            }
        }

        current_time += frame_duration;
    }

    let total_spans = trace.get_frame().spans.len();
    println!("Generated trace with {} spans across 500 frames", total_spans);
    println!("Average spans per frame: {}", total_spans / 500);
}

fn main() {
    App::new().run(|cx| {
        cx.activate(true);
        
        // Generate demo trace data
        let trace_data = TraceData::new();
        generate_demo_trace(&trace_data);
        
        // Open flamegraph window
        FlamegraphWindow::open(trace_data, cx);
    });
}
