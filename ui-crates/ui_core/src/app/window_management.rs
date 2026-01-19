//! Window management and creation logic

use std::sync::Arc;
use gpui::{px, size, Bounds, Context, Point, Window, WindowBounds, WindowKind, WindowOptions};
use gpui::AppContext;
use ui::Root;
use ui_problems::ProblemsWindow;
use ui_flamegraph::{FlamegraphWindow, TraceData};
use ui_type_debugger::TypeDebuggerWindow;
use ui_multiplayer::MultiplayerWindow;

use super::PulsarApp;

impl PulsarApp {
    /// Create a detached window with a panel, sharing the rust analyzer
    pub(super) fn create_detached_window(
        &self,
        panel: Arc<dyn ui::dock::PanelView>,
        position: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let window_size = size(px(800.), px(600.));
        let window_bounds = Bounds::new(
            Point {
                x: position.x - px(100.0),
                y: position.y - px(30.0),
            },
            window_size,
        );

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: None,
            }),
            window_min_size: Some(gpui::Size {
                width: px(400.),
                height: px(300.),
            }),
            kind: WindowKind::Normal,
            is_resizable: true,
            window_decorations: Some(gpui::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        let project_path = self.state.project_path.clone();
        let rust_analyzer = self.state.rust_analyzer.clone();

        let _ = cx.open_window(window_options, move |window, cx| {
            let app = cx.new(|cx| {
                let app = Self::new_with_shared_analyzer(
                    project_path.clone(),
                    rust_analyzer.clone(),
                    window,
                    cx,
                );

                app.state.center_tabs.update(cx, |tabs, cx| {
                    tabs.add_panel(panel.clone(), window, cx);
                });

                app
            });

            cx.new(|cx| Root::new(app.into(), window, cx))
        });
    }

    pub(super) fn toggle_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.drawer_open = !self.state.drawer_open;
        cx.notify();
    }

    pub(super) fn toggle_problems(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let problems_drawer = self.state.problems_drawer.clone();

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(100.0),
                        y: px(100.0),
                    },
                    size: size(px(900.0), px(600.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(500.),
                    height: px(300.),
                }),
                ..Default::default()
            },
            |window, cx| {
                let problems_window = cx.new(|cx| ProblemsWindow::new(problems_drawer, window, cx));
                cx.new(|cx| Root::new(problems_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_type_debugger(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let type_debugger_drawer = self.state.type_debugger_drawer.clone();

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(120.0),
                        y: px(120.0),
                    },
                    size: size(px(1000.0), px(700.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(600.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            |window, cx| {
                let type_debugger_window = cx.new(|cx| TypeDebuggerWindow::new(type_debugger_drawer, window, cx));
                cx.new(|cx| Root::new(type_debugger_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_multiplayer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let project_path = self.state.project_path.clone();

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(200.0),
                        y: px(200.0),
                    },
                    size: size(px(500.0), px(600.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(400.),
                    height: px(500.),
                }),
                ..Default::default()
            },
            move |window, cx| {
                let multiplayer_window = cx.new(|cx| MultiplayerWindow::new(project_path, window, cx));
                cx.new(|cx| Root::new(multiplayer_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_flamegraph(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Generate realistic large-scale multi-threaded trace data
        let trace_data = TraceData::new();
        
        use ui_flamegraph::TraceSpan;
        use rand::Rng;
        
        let mut rng = rand::thread_rng();
        let mut current_time = 0u64;
        
        // Simulate 200 frames of execution
        for frame_idx in 0..200 {
            let frame_start = current_time;
            let frame_duration: u64 = rng.gen_range(12_000_000..20_000_000); // 12-20ms
            
            // Calculate actual frame time for graph
            let frame_time_ms = frame_duration as f32 / 1_000_000.0;
            trace_data.add_frame_time(frame_time_ms);
            
            // === GPU Thread (thread_id = 0) ===
            let gpu_start = frame_start + rng.gen_range(0..2_000_000);
            let gpu_duration = rng.gen_range(8_000_000..15_000_000);
            
            trace_data.add_span(TraceSpan {
                name: format!("GPU Frame {}", frame_idx),
                start_ns: gpu_start,
                duration_ns: gpu_duration,
                depth: 0,
                thread_id: 0, // GPU
                color_index: 14,
            });
            
            // GPU rendering stages
            let gpu_stages = [
                ("Shadow Pass", 0.15),
                ("G-Buffer Pass", 0.25),
                ("Lighting Pass", 0.30),
                ("Post-Processing", 0.20),
                ("Present", 0.10),
            ];
            
            let mut gpu_time = gpu_start;
            for (idx, (stage, ratio)) in gpu_stages.iter().enumerate() {
                let stage_duration = (gpu_duration as f32 * ratio) as u64;
                trace_data.add_span(TraceSpan {
                    name: stage.to_string(),
                    start_ns: gpu_time,
                    duration_ns: stage_duration,
                    depth: 1,
                    thread_id: 0,
                    color_index: (14 + idx) as u8 % 16,
                });
                gpu_time += stage_duration;
            }
            
            // === Main Thread (thread_id = 1) ===
            trace_data.add_span(TraceSpan {
                name: format!("Frame {}", frame_idx),
                start_ns: frame_start,
                duration_ns: frame_duration,
                depth: 0,
                thread_id: 1,
                color_index: 0,
            });
            
            // Main thread systems
            let systems = [
                ("Input", 200_000, 800_000, 5),
                ("Update", 2_000_000, 6_000_000, 15),
                ("Render Submit", 1_000_000, 3_000_000, 12),
                ("Audio", 100_000, 500_000, 6),
            ];
            
            let mut system_time = frame_start;
            
            for (sys_idx, (system_name, min_dur, max_dur, max_subtasks)) in systems.iter().enumerate() {
                let sys_duration = rng.gen_range(*min_dur..*max_dur);
                
                trace_data.add_span(TraceSpan {
                    name: system_name.to_string(),
                    start_ns: system_time,
                    duration_ns: sys_duration,
                    depth: 1,
                    thread_id: 1,
                    color_index: (sys_idx + 1) as u8,
                });
                
                // Add subsystems
                let num_subsystems = rng.gen_range(2..=*max_subtasks);
                let mut subsystem_time = system_time;
                let avg_subsystem_duration = sys_duration / num_subsystems as u64;
                
                for sub_idx in 0..num_subsystems {
                    let sub_duration = rng.gen_range(
                        avg_subsystem_duration / 3..avg_subsystem_duration * 2
                    ).min(sys_duration - (subsystem_time - system_time));
                    
                    if sub_duration == 0 {
                        break;
                    }
                    
                    let subsystem_name = match system_name {
                        &"Update" => format!("Entity {}", sub_idx),
                        &"Render Submit" => format!("DrawCall {}", sub_idx),
                        &"Audio" => format!("Channel {}", sub_idx),
                        &"Input" => format!("Device {}", sub_idx),
                        _ => format!("Task {}", sub_idx),
                    };
                    
                    trace_data.add_span(TraceSpan {
                        name: subsystem_name,
                        start_ns: subsystem_time,
                        duration_ns: sub_duration,
                        depth: 2,
                        thread_id: 1,
                        color_index: ((sys_idx + sub_idx) % 16) as u8,
                    });
                    
                    // Deep nesting occasionally
                    if rng.gen_bool(0.3) && sub_duration > 100_000 {
                        let num_deep = rng.gen_range(2..4);
                        let mut deep_time = subsystem_time;
                        let avg_deep = sub_duration / num_deep as u64;
                        
                        for deep_idx in 0..num_deep {
                            let deep_duration = rng.gen_range(avg_deep / 3..avg_deep)
                                .min(sub_duration - (deep_time - subsystem_time));
                            if deep_duration == 0 { break; }
                            
                            trace_data.add_span(TraceSpan {
                                name: format!("Op {}", deep_idx),
                                start_ns: deep_time,
                                duration_ns: deep_duration,
                                depth: 3,
                                thread_id: 1,
                                color_index: ((sys_idx * 3 + deep_idx) % 16) as u8,
                            });
                            deep_time += deep_duration;
                        }
                    }
                    
                    subsystem_time += sub_duration;
                }
                
                system_time += sys_duration;
            }
            
            // === Worker Threads (thread_id = 2-9) ===
            let num_workers = 8;
            for worker_id in 0..num_workers {
                let thread_id = worker_id + 2;
                
                // Physics on workers 2-5
                if worker_id < 4 {
                    let phys_start = frame_start + rng.gen_range(0..3_000_000);
                    let phys_duration = rng.gen_range(2_000_000..6_000_000);
                    
                    trace_data.add_span(TraceSpan {
                        name: "Physics Job".to_string(),
                        start_ns: phys_start,
                        duration_ns: phys_duration,
                        depth: 0,
                        thread_id,
                        color_index: 5,
                    });
                    
                    // Collision checks
                    let num_collisions = rng.gen_range(5..15);
                    let mut coll_time = phys_start;
                    let avg_coll = phys_duration / num_collisions as u64;
                    
                    for coll_idx in 0..num_collisions {
                        let coll_duration = rng.gen_range(avg_coll / 2..avg_coll * 2)
                            .min(phys_duration - (coll_time - phys_start));
                        if coll_duration == 0 { break; }
                        
                        trace_data.add_span(TraceSpan {
                            name: format!("Collision {}", coll_idx),
                            start_ns: coll_time,
                            duration_ns: coll_duration,
                            depth: 1,
                            thread_id,
                            color_index: ((5 + coll_idx) % 16) as u8,
                        });
                        coll_time += coll_duration;
                    }
                }
                
                // AI/Pathfinding on workers 6-9
                if worker_id >= 4 {
                    let ai_start = frame_start + rng.gen_range(0..5_000_000);
                    let ai_duration = rng.gen_range(1_000_000..4_000_000);
                    
                    trace_data.add_span(TraceSpan {
                        name: "AI Job".to_string(),
                        start_ns: ai_start,
                        duration_ns: ai_duration,
                        depth: 0,
                        thread_id,
                        color_index: 10,
                    });
                    
                    // Pathfinding operations
                    let num_paths = rng.gen_range(3..8);
                    let mut path_time = ai_start;
                    let avg_path = ai_duration / num_paths as u64;
                    
                    for path_idx in 0..num_paths {
                        let path_duration = rng.gen_range(avg_path / 2..avg_path * 2)
                            .min(ai_duration - (path_time - ai_start));
                        if path_duration == 0 { break; }
                        
                        trace_data.add_span(TraceSpan {
                            name: format!("Pathfind {}", path_idx),
                            start_ns: path_time,
                            duration_ns: path_duration,
                            depth: 1,
                            thread_id,
                            color_index: ((10 + path_idx) % 16) as u8,
                        });
                        path_time += path_duration;
                    }
                }
            }
            
            current_time += frame_duration;
        }

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(140.0),
                        y: px(140.0),
                    },
                    size: size(px(1400.0), px(900.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(800.),
                    height: px(600.),
                }),
                ..Default::default()
            },
            move |window, cx| {
                let flamegraph_window = FlamegraphWindow::new(trace_data, window, cx);
                cx.new(|cx| Root::new(flamegraph_window.into(), window, cx))
            },
        );
    }
}