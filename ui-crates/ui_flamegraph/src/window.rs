use gpui::*;
use ui::{TitleBar, v_flex, h_flex, ActiveTheme, StyledExt};
use crate::{FlamegraphView, TraceData};
use std::sync::Arc;

pub struct FlamegraphWindow {
    view: Entity<FlamegraphView>,
    profiler: Option<dtrace_profiler::DTraceProfiler>,
    trace_data: Arc<TraceData>,
    is_profiling: bool,
}

impl FlamegraphWindow {
    pub fn new(trace_data: Arc<TraceData>, _window: &mut Window, cx: &mut App) -> Entity<Self> {
        // Clone the Arc so window and view share the same TraceData
        let view_trace_data = Arc::clone(&trace_data);
        let view = cx.new(move |_cx| FlamegraphView::new((*view_trace_data).clone()));
        
        cx.new(|_cx| Self { 
            view,
            profiler: None,
            trace_data,
            is_profiling: false,
        })
    }

    pub fn open(cx: &mut App) -> WindowHandle<Self> {
        let trace_data = Arc::new(TraceData::new());
        
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(100.0), px(100.0)),
                size: size(px(1200.0), px(800.0)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("Flamegraph Trace Viewer".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            window_background: WindowBackgroundAppearance::Opaque,
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_movable: true,
            is_minimizable: true,
            is_resizable: true,
            window_decorations: None,
            display_id: None,
            window_min_size: Some(size(px(600.0), px(400.0))),
            tabbing_identifier: None,
            app_id: None,
        };

        cx.open_window(window_options, |_window, cx| {
            Self::new(trace_data, _window, cx)
        }).unwrap()
    }

    fn start_profiling(&mut self, cx: &mut Context<Self>) {
        if self.is_profiling {
            return;
        }

        // Create database path in temp directory with timestamp
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let db_path = std::env::temp_dir().join(format!("dtrace_profile_{}.db", timestamp));
        
        println!("[PROFILER] Starting trace with database: {:?}", db_path);

        // Create profiler with database
        let mut profiler = match dtrace_profiler::DTraceProfiler::with_database(&db_path) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[PROFILER] Failed to create profiler: {}", e);
                return;
            }
        };

        // Start profiling at 99 Hz
        if let Err(e) = profiler.start(99) {
            eprintln!("[PROFILER] Failed to start profiling: {}", e);
            eprintln!("[PROFILER] Troubleshooting:");
            eprintln!("[PROFILER]   1. Make sure you rebooted after installing DTrace");
            eprintln!("[PROFILER]   2. Run this application as Administrator (right-click → Run as administrator)");
            eprintln!("[PROFILER]   3. Check if dtrace.exe exists: where dtrace.exe");
            eprintln!("[PROFILER]   4. Try running manually: dtrace.exe -V");
            return;
        }

        self.profiler = Some(profiler);
        self.is_profiling = true;

        // Start update timer to poll for new samples
        let trace_data = Arc::clone(&self.trace_data);
        let view = self.view.clone();
        cx.spawn(async move |this, mut cx| {
            let mut last_timestamp = 0u64;
            
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(1000)).await;
                
                // Check if still profiling
                let should_continue = cx.update(|cx| {
                    this.update(cx, |window, _cx| window.is_profiling).ok()
                }).ok().flatten().unwrap_or(false);

                if !should_continue {
                    break;
                }

                // Get profiler and fetch new samples
                let _ = cx.update(|cx| {
                    this.update(cx, |window, cx| {
                        if let Some(ref profiler) = window.profiler {
                            // Get samples from database since last timestamp
                            match profiler.get_samples_from_db(last_timestamp) {
                                Ok(samples) if !samples.is_empty() => {
                                    println!("[PROFILER] Got {} new samples from DB, converting to spans", samples.len());
                                    
                                    let current_frame = trace_data.get_frame();
                                    println!("[PROFILER] Before: TraceFrame has {} spans, time range: {} - {}", 
                                        current_frame.spans.len(), current_frame.min_time_ns, current_frame.max_time_ns);
                                    
                                    // Convert samples to TraceSpans and add to trace_data
                                    for sample in &samples {
                                        println!("[PROFILER] Sample: timestamp={}, thread={}, {} frames", 
                                            sample.timestamp_ns, sample.thread_id, sample.stack_frames.len());
                                        
                                        // Each stack frame becomes a span with increasing depth
                                        for (depth, frame) in sample.stack_frames.iter().enumerate() {
                                            let span = crate::TraceSpan {
                                                name: frame.function_name.clone(),
                                                start_ns: sample.timestamp_ns,
                                                duration_ns: 1_000_000, // 1ms default duration for samples
                                                depth: depth as u32,
                                                thread_id: sample.thread_id,
                                                color_index: (depth % 10) as u8,
                                            };
                                            
                                            println!("[PROFILER] Adding span: '{}' at depth {} on thread {}, time {} -> {}", 
                                                span.name, span.depth, span.thread_id, span.start_ns, span.end_ns());
                                            
                                            trace_data.add_span(span);
                                        }
                                    }
                                    
                                    let updated_frame = trace_data.get_frame();
                                    println!("[PROFILER] After: TraceFrame has {} spans, time range: {} - {}", 
                                        updated_frame.spans.len(), updated_frame.min_time_ns, updated_frame.max_time_ns);
                                    
                                    // Update last timestamp
                                    if let Some(last_sample) = samples.last() {
                                        last_timestamp = last_sample.timestamp_ns;
                                    }
                                    
                                    // Notify view to refresh
                                    window.view.update(cx, |_, vcx| vcx.notify());
                                    cx.notify();
                                }
                                Err(e) => {
                                    eprintln!("[PROFILER] Failed to read samples: {}", e);
                                }
                                _ => {}
                            }
                        }
                    })
                });
            }
        }).detach();

        cx.notify();
    }

    fn stop_profiling(&mut self, cx: &mut Context<Self>) {
        if let Some(mut profiler) = self.profiler.take() {
            profiler.stop();
            println!("[PROFILER] Profiling stopped");
        }
        self.is_profiling = false;
        cx.notify();
    }
}

impl Render for FlamegraphWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_profiling = self.is_profiling;
        
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                TitleBar::new()
                    .child("Flamegraph Profiler - Live CPU Sampling")
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                if !is_profiling {
                                    div()
                                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                            this.start_profiling(cx);
                                        }))
                                        .child("▶ Start")
                                        .px_3()
                                        .py_1()
                                        .bg(gpui::green())
                                        .rounded(px(4.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .text_size(px(12.0))
                                } else {
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                                    this.stop_profiling(cx);
                                                }))
                                                .child("⏹ Stop")
                                                .px_3()
                                                .py_1()
                                                .bg(gpui::red())
                                                .rounded(px(4.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .text_size(px(12.0))
                                        )
                                        .child(
                                            div()
                                                .child("● Recording")
                                                .text_color(gpui::red())
                                                .text_size(px(12.0))
                                        )
                                }
                            )
                    )
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.view.clone())
            )
    }
}
