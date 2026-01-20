use gpui::*;
use ui::{TitleBar, v_flex, h_flex, ActiveTheme, StyledExt};
use crate::{FlamegraphView, TraceData, InstrumentationCollector};
use std::sync::Arc;

pub struct FlamegraphWindow {
    view: Entity<FlamegraphView>,
    collector: Option<Arc<InstrumentationCollector>>,
    trace_data: Arc<TraceData>,
    is_profiling: bool,
    current_db_path: Option<std::path::PathBuf>,
    db_connection: Option<Arc<parking_lot::Mutex<rusqlite::Connection>>>,
}

impl FlamegraphWindow {
    pub fn new(trace_data: Arc<TraceData>, _window: &mut Window, cx: &mut App) -> Entity<Self> {
        // Clone the Arc so window and view share the same TraceData
        let view_trace_data = Arc::clone(&trace_data);
        let view = cx.new(move |_cx| FlamegraphView::new((*view_trace_data).clone()));
        
        cx.new(|_cx| Self { 
            view,
            collector: None,
            trace_data,
            is_profiling: false,
            current_db_path: None,
            db_connection: None,
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
                title: Some("Flamegraph Profiler (Instrumentation)".into()),
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

    fn start_profiling(&mut self, _cx: &mut Context<Self>) {
        if self.is_profiling {
            return;
        }

        println!("[PROFILER] Starting instrumentation collector");

        // Create database file in project directory
        if let Some(project_path) = engine_state::get_project_path() {
            match profiling::database::ensure_profiling_dir(project_path) {
                Ok(profiling_dir) => {
                    let db_filename = profiling::database::generate_db_filename();
                    let db_path = profiling_dir.join(&db_filename);
                    
                    match profiling::database::create_database(&db_path) {
                        Ok(conn) => {
                            println!("[PROFILER] Created database: {}", db_path.display());
                            self.current_db_path = Some(db_path);
                            self.db_connection = Some(Arc::new(parking_lot::Mutex::new(conn)));
                        }
                        Err(e) => {
                            eprintln!("[PROFILER] Failed to create database: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[PROFILER] Failed to create profiling directory: {}", e);
                }
            }
        }

        // Create instrumentation collector
        let collector = Arc::new(InstrumentationCollector::new(
            Arc::clone(&self.trace_data),
            100, // Update UI every 100ms
        ));
        
        collector.start();
        self.collector = Some(collector);
        self.is_profiling = true;

        println!("[PROFILER] Instrumentation profiling started");
        _cx.notify();
    }

    fn stop_profiling(&mut self, _cx: &mut Context<Self>) {
        if !self.is_profiling {
            return;
        }

        if let Some(collector) = &self.collector {
            collector.stop();
        }

        // Save all events to database before stopping
        if let Some(db_conn) = &self.db_connection {
            let events = profiling::get_all_events();
            if let Err(e) = profiling::database::save_events(&db_conn.lock(), &events) {
                eprintln!("[PROFILER] Failed to save events to database: {}", e);
            } else {
                println!("[PROFILER] Saved {} events to database", events.len());
                if let Some(path) = &self.current_db_path {
                    println!("[PROFILER] Database saved to: {}", path.display());
                }
            }
        }

        self.collector = None;
        self.is_profiling = false;
        
        println!("[PROFILER] Instrumentation profiling stopped");
        _cx.notify();
    }

    fn open_database_picker(&mut self, cx: &mut Context<Self>) {
        // Stop current profiling if active
        if self.is_profiling {
            self.stop_profiling(cx);
        }

        // Open file picker for .db files using rfd
        let file_dialog = rfd::AsyncFileDialog::new()
            .set_title("Select Profiling Database")
            .add_filter("Database", &["db"])
            .set_directory(
                engine_state::get_project_path()
                    .and_then(|p| std::path::PathBuf::from(p).join(".pulsar/profiling/flamegraph").canonicalize().ok())
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            );

        cx.spawn(async move |this, cx| {
            if let Some(file) = file_dialog.pick_file().await {
                let db_path = file.path().to_path_buf();
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.load_from_database(db_path, cx);
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    fn load_from_database(&mut self, db_path: std::path::PathBuf, _cx: &mut Context<Self>) {
        match rusqlite::Connection::open(&db_path) {
            Ok(conn) => {
                match profiling::database::load_events(&conn) {
                    Ok(events) => {
                        println!("[PROFILER] Loaded {} events from {}", events.len(), db_path.display());
                        
                        // Convert to TraceData format
                        if let Err(e) = crate::profiler::convert_profile_events_to_trace(&events, &self.trace_data) {
                            eprintln!("[PROFILER] Failed to convert events: {}", e);
                        }
                        
                        self.current_db_path = Some(db_path);
                        _cx.notify();
                    }
                    Err(e) => {
                        eprintln!("[PROFILER] Failed to load events from database: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("[PROFILER] Failed to open database: {}", e);
            }
        }
    }

    fn open_file_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(project_path) = engine_state::get_project_path() {
            // List available sessions
            match profiling::database::list_profiling_sessions(project_path) {
                Ok(sessions) => {
                    println!("[PROFILER] Found {} profiling sessions", sessions.len());
                    if let Some(latest) = sessions.first() {
                        // For now, just load the latest
                        // TODO: Show a UI list to pick from
                        self.load_from_database(latest.clone(), cx);
                    } else {
                        println!("[PROFILER] No profiling sessions found");
                    }
                }
                Err(e) => {
                    eprintln!("[PROFILER] Failed to list sessions: {}", e);
                }
            }
        }
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
                    .child("Flamegraph Profiler - Instrumentation (Unreal Insights Style)")
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
                            .child(
                                // Open button - load past recordings via file picker
                                div()
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                        this.open_database_picker(cx);
                                    }))
                                    .child("📂 Open")
                                    .px_3()
                                    .py_1()
                                    .bg(theme.accent)
                                    .rounded(px(4.0))
                                    .cursor(CursorStyle::PointingHand)
                                    .text_size(px(12.0))
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
