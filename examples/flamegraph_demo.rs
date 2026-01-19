//! Flamegraph Demo
//! 
//! Demonstrates the high-performance flamegraph viewer with thousands of spans

use gpui::*;
use ui_flamegraph::{FlamegraphWindow, TraceData, TraceSpan};
use rand::Rng;

fn generate_demo_trace(trace: &TraceData) {
    let mut rng = rand::thread_rng();
    let mut current_time = 0u64;
    
    // Simulate nested function calls with varying depths
    for frame_idx in 0..100 {
        let frame_start = current_time;
        
        // Top-level function
        trace.add_span(TraceSpan {
            name: format!("Frame {}", frame_idx),
            start_ns: frame_start,
            duration_ns: 16_600_000, // ~16.6ms per frame
            depth: 0,
            thread_id: 1,
            color_index: (frame_idx % 16) as u8,
        });
        
        // Simulate multiple systems
        let systems = ["Physics", "Rendering", "Audio", "Network", "AI"];
        let mut system_time = frame_start;
        
        for (sys_idx, system) in systems.iter().enumerate() {
            let sys_duration = rng.gen_range(1_000_000..5_000_000);
            
            trace.add_span(TraceSpan {
                name: system.to_string(),
                start_ns: system_time,
                duration_ns: sys_duration,
                depth: 1,
                thread_id: 1,
                color_index: (sys_idx % 16) as u8,
            });
            
            // Add sub-tasks for each system
            let num_subtasks = rng.gen_range(3..10);
            let mut subtask_time = system_time;
            
            for subtask_idx in 0..num_subtasks {
                let subtask_duration = sys_duration / num_subtasks as u64;
                let actual_duration = rng.gen_range(subtask_duration / 2..subtask_duration);
                
                trace.add_span(TraceSpan {
                    name: format!("{} Task {}", system, subtask_idx),
                    start_ns: subtask_time,
                    duration_ns: actual_duration,
                    depth: 2,
                    thread_id: 1,
                    color_index: ((sys_idx + subtask_idx) % 16) as u8,
                });
                
                // Add even deeper nesting for some tasks
                if rng.gen_bool(0.3) {
                    let deep_duration = actual_duration / 3;
                    trace.add_span(TraceSpan {
                        name: format!("{} Deep {}", system, subtask_idx),
                        start_ns: subtask_time + deep_duration,
                        duration_ns: deep_duration,
                        depth: 3,
                        thread_id: 1,
                        color_index: ((sys_idx * 3 + subtask_idx) % 16) as u8,
                    });
                    
                    // Ultra-deep nesting
                    if rng.gen_bool(0.5) {
                        let ultra_duration = deep_duration / 2;
                        trace.add_span(TraceSpan {
                            name: format!("Inner {}", subtask_idx),
                            start_ns: subtask_time + deep_duration + ultra_duration / 2,
                            duration_ns: ultra_duration,
                            depth: 4,
                            thread_id: 1,
                            color_index: ((sys_idx * 5 + subtask_idx * 2) % 16) as u8,
                        });
                    }
                }
                
                subtask_time += actual_duration;
            }
            
            system_time += sys_duration;
        }
        
        current_time += 16_600_000;
    }
    
    println!("Generated trace with {} spans", trace.get_frame().spans.len());
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
