use gpui::*;
use ui::{TitleBar, v_flex, h_flex, ActiveTheme, StyledExt, button::Button, IconName};
use crate::{FlamegraphView, TraceData, InstrumentationCollector};
use std::sync::Arc;
use gpui::prelude::FluentBuilder;

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
            titlebar: Some(TitleBar::title_bar_options()),
            window_background: WindowBackgroundAppearance::Opaque,
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_movable: true,
            is_minimizable: true,
            is_resizable: true,
            window_decorations: Some(WindowDecorations::Client),
            display_id: None,
            window_min_size: Some(size(px(800.0), px(600.0))),
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
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("Flamegraph Profiler")
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme.muted_foreground)
                                            .child("• Instrumentation-Based")
                                    )
                            )
                            .when(self.current_db_path.is_some(), |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.0))
                                        .bg(theme.accent.opacity(0.1))
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(theme.accent)
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .child(format!("Session: {}", 
                                                    self.current_db_path.as_ref()
                                                        .and_then(|p| p.file_name())
                                                        .and_then(|n| n.to_str())
                                                        .unwrap_or("Unknown")
                                                ))
                                        )
                                )
                            })
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                if !is_profiling {
                                    Button::new("start-profiling")
                                        .icon(IconName::Play)
                                        .label("Record")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.start_profiling(cx);
                                        }))
                                        .into_any_element()
                                } else {
                                    h_flex()
                                        .gap_3()
                                        .items_center()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .px_3()
                                                .py_1()
                                                .rounded(px(6.0))
                                                .bg(gpui::red().opacity(0.15))
                                                .border_1()
                                                .border_color(gpui::red().opacity(0.3))
                                                .child(
                                                    div()
                                                        .size(px(8.0))
                                                        .rounded(px(4.0))
                                                        .bg(gpui::red())
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(gpui::red())
                                                        .font_weight(gpui::FontWeight::BOLD)
                                                        .child("RECORDING")
                                                )
                                        )
                                        .child(
                                            Button::new("stop-profiling")
                                                .icon(IconName::Square)
                                                .label("Stop")
                                                .on_click(cx.listener(|this, _event, _window, cx| {
                                                    this.stop_profiling(cx);
                                                }))
                                        )
                                        .into_any_element()
                                }
                            )
                            .child(
                                Button::new("open-session")
                                    .icon(IconName::FolderOpen)
                                    .label("Open Session")
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.open_database_picker(cx);
                                    }))
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
