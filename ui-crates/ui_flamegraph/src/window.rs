use gpui::*;
use rust_i18n::t;
use ui::{
    TitleBar, v_flex, h_flex, ActiveTheme, IconName, Icon,
    button::Button,
    resizable::{h_resizable, resizable_panel, ResizableState},
};
use crate::{FlamegraphView, TraceData, InstrumentationCollector, FlamegraphPanel, StatisticsPanel};
use std::sync::Arc;
use gpui::prelude::FluentBuilder;

pub struct FlamegraphWindow {
    view: Entity<FlamegraphView>,
    collector: Option<Arc<InstrumentationCollector>>,
    trace_data: Arc<TraceData>,
    is_profiling: bool,
    current_db_path: Option<std::path::PathBuf>,
    db_connection: Option<Arc<parking_lot::Mutex<rusqlite::Connection>>>,
    flamegraph_panel: Option<Entity<FlamegraphPanel>>,
    statistics_panel: Option<Entity<StatisticsPanel>>,
    resizable_state: Entity<ResizableState>,
}

impl FlamegraphWindow {
    pub fn new(trace_data: Arc<TraceData>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        // Clone the Arc so window and view share the same TraceData
        let view_trace_data = Arc::clone(&trace_data);
        let view = cx.new(move |_cx| FlamegraphView::new((*view_trace_data).clone()));
        
        cx.new(|cx| {
            let resizable_state = ResizableState::new(cx);
            
            Self { 
                view,
                collector: None,
                trace_data,
                is_profiling: false,
                current_db_path: None,
                db_connection: None,
                flamegraph_panel: None,
                statistics_panel: None,
                resizable_state,
            }
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

    fn render_empty_state(&mut self, is_profiling: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let accent_color = theme.accent;
        
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_12()
            .child(
                v_flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(96.0))
                            .rounded(px(16.0))
                            .bg(theme.muted.opacity(0.1))
                            .child(
                                Icon::new(IconName::Activity)
                                    .size(px(48.0))
                                    .text_color(theme.muted_foreground.opacity(0.4))
                            )
                    )
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child(if is_profiling {
                                        t!("Flamegraph.RecordingInProgress").to_string()
                                    } else {
                                        t!("Flamegraph.NoDataLoaded").to_string()
                                    })
                            )
                            .child(
                                div()
                                    .text_base()
                                    .text_color(theme.muted_foreground)
                                    .child(if is_profiling {
                                        t!("Flamegraph.WaitingForData").to_string()
                                    } else {
                                        t!("Flamegraph.GetStarted").to_string()
                                    })
                            )
                    )
            )
            .when(!is_profiling, |this| {
                this.child(
                    v_flex()
                        .gap_3()
                        .w(px(480.0))
                        .child(
                            h_flex()
                                .w_full()
                                .p_5()
                                .gap_4()
                                .rounded(px(12.0))
                                .bg(theme.popover)
                                .border_1()
                                .border_color(theme.border)
                                .cursor_pointer()
                                .hover(|style| {
                                    style
                                        .bg(theme.accent.opacity(0.08))
                                        .border_color(theme.accent.opacity(0.3))
                                })
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                    this.start_profiling(cx);
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(56.0))
                                        .flex_shrink_0()
                                        .rounded(px(10.0))
                                        .bg(gpui::red().opacity(0.15))
                                        .border_1()
                                        .border_color(gpui::red().opacity(0.2))
                                        .child(
                                            Icon::new(IconName::Circle)
                                                .size(px(28.0))
                                                .text_color(gpui::red())
                                        )
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(theme.foreground)
                                                .child(t!("Flamegraph.StartRecording").to_string())
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .line_height(relative(1.4))
                                                .child(t!("Flamegraph.StartRecordingDesc").to_string())
                                        )
                                )
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .p_5()
                                .gap_4()
                                .rounded(px(12.0))
                                .bg(theme.popover)
                                .border_1()
                                .border_color(theme.border)
                                .cursor_pointer()
                                .hover(|style| {
                                    style
                                        .bg(theme.accent.opacity(0.08))
                                        .border_color(theme.accent.opacity(0.3))
                                })
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                    this.open_database_picker(cx);
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(56.0))
                                        .flex_shrink_0()
                                        .rounded(px(10.0))
                                        .bg(accent_color.opacity(0.15))
                                        .border_1()
                                        .border_color(accent_color.opacity(0.2))
                                        .child(
                                            Icon::new(IconName::FolderOpen)
                                                .size(px(28.0))
                                                .text_color(accent_color)
                                        )
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(theme.foreground)
                                                .child(t!("Flamegraph.OpenPreviousSession").to_string())
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .line_height(relative(1.4))
                                                .child(t!("Flamegraph.OpenSessionDesc").to_string())
                                        )
                                )
                        )
                        .child(
                            div()
                                .mt_6()
                                .px_5()
                                .py_4()
                                .rounded(px(10.0))
                                .bg(theme.muted.opacity(0.08))
                                .border_1()
                                .border_color(theme.border.opacity(0.5))
                                .child(
                                    v_flex()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(theme.muted_foreground)
                                                .child(t!("Flamegraph.ProTips").to_string().to_uppercase())
                                        )
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_start()
                                                        .child(
                                                            div()
                                                                .mt_0p5()
                                                                .text_sm()
                                                                .text_color(theme.accent)
                                                                .child("•")
                                                        )
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .text_color(theme.muted_foreground)
                                                                .line_height(relative(1.5))
                                                                .child(t!("Flamegraph.Tip1").to_string())
                                                        )
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_start()
                                                        .child(
                                                            div()
                                                                .mt_0p5()
                                                                .text_sm()
                                                                .text_color(theme.accent)
                                                                .child("•")
                                                        )
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .text_color(theme.muted_foreground)
                                                                .line_height(relative(1.5))
                                                                .child(t!("Flamegraph.Tip2").to_string())
                                                        )
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_start()
                                                        .child(
                                                            div()
                                                                .mt_0p5()
                                                                .text_sm()
                                                                .text_color(theme.accent)
                                                                .child("•")
                                                        )
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .text_color(theme.muted_foreground)
                                                                .line_height(relative(1.5))
                                                                .child(t!("Flamegraph.Tip3").to_string())
                                                        )
                                                )
                                        )
                                )
                        )
                )
            })
            .when(is_profiling, |this| {
                this.child(
                    v_flex()
                        .gap_4()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_6()
                                .py_3()
                                .rounded(px(10.0))
                                .bg(gpui::red().opacity(0.1))
                                .border_1()
                                .border_color(gpui::red().opacity(0.25))
                                .child(
                                    div()
                                        .size(px(10.0))
                                        .rounded(px(5.0))
                                        .bg(gpui::red())
                                        .child(
                                            div()
                                                .size(px(10.0))
                                                .rounded(px(5.0))
                                                .bg(gpui::red())
                                                // Simple pulse animation via opacity
                                        )
                                )
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme.foreground)
                                        .child(t!("Flamegraph.RecordingActive").to_string())
                                )
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(t!("Flamegraph.DataWillAppear").to_string())
                        )
                )
            })
    }

    fn render_profiling_overlay(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let frame = self.trace_data.get_frame();
        let span_count = frame.spans.len();
        let thread_count = frame.threads.len();
        
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background.opacity(0.85))
            .child(
                v_flex()
                    .gap_6()
                    .items_center()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_4()
                            .px_8()
                            .py_6()
                            .rounded(px(16.0))
                            .bg(theme.popover)
                            .border_1()
                            .border_color(theme.border)
                            .shadow_xl()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .px_4()
                                    .py_2()
                                    .rounded(px(8.0))
                                    .bg(gpui::red().opacity(0.1))
                                    .border_1()
                                    .border_color(gpui::red().opacity(0.25))
                                    .child(
                                        div()
                                            .size(px(10.0))
                                            .rounded(px(5.0))
                                            .bg(gpui::red())
                                    )
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(gpui::red())
                                            .child(t!("Flamegraph.CollectingData").to_string())
                                    )
                            )
                            .child(
                                v_flex()
                                    .gap_3()
                                    .w(px(300.0))
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_base()
                                                    .text_color(theme.muted_foreground)
                                                    .child(t!("Flamegraph.SpansCollected").to_string())
                                            )
                                            .child(
                                                div()
                                                    .text_base()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child(format!("{}", span_count))
                                            )
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_base()
                                                    .text_color(theme.muted_foreground)
                                                    .child(t!("Flamegraph.Threads").to_string())
                                            )
                                            .child(
                                                div()
                                                    .text_base()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child(format!("{}", thread_count))
                                            )
                                    )
                            )
                            .child(
                                Button::new("stop-recording-btn")
                                    .w_full()
                                    .label(t!("Flamegraph.StopRecording").to_string())
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.stop_profiling(cx);
                                    }))
                            )
                    )
            )
    }
}

impl Render for FlamegraphWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_profiling = self.is_profiling;
        // Only show data when NOT profiling - during profiling, data is being collected but not displayed
        let has_data = !is_profiling && self.trace_data.get_frame().spans.len() > 0;
        
        // Initialize panels on first render with data (when profiling stops)
        if has_data && self.flamegraph_panel.is_none() {
            self.flamegraph_panel = Some(cx.new(|cx| FlamegraphPanel::new(self.view.clone(), cx)));
            self.statistics_panel = Some(cx.new(|cx| StatisticsPanel::new(self.trace_data.clone(), cx)));
        }
        
        let theme = cx.theme();
        
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                TitleBar::new()
                    .child(
                        h_flex()
                            .gap_4()
                            .items_center()
                            .flex_1()
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
                                    .when(has_data && !is_profiling && self.current_db_path.is_some(), |this| {
                                        this.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1p5()
                                                .px_2p5()
                                                .py_1()
                                                .rounded(px(6.0))
                                                .bg(theme.accent.opacity(0.1))
                                                .border_1()
                                                .border_color(theme.accent.opacity(0.2))
                                                .child(
                                                    Icon::new(IconName::Database)
                                                        .size(px(12.0))
                                                        .text_color(theme.accent)
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.0))
                                                        .text_color(theme.accent)
                                                        .font_weight(gpui::FontWeight::MEDIUM)
                                                        .child(
                                                            self.current_db_path.as_ref()
                                                                .and_then(|p| p.file_name())
                                                                .and_then(|n| n.to_str())
                                                                .unwrap_or("Unknown")
                                                                .to_string()
                                                        )
                                                )
                                        )
                                    })
                                    .when(has_data && is_profiling, |this| {
                                        this.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .px_3()
                                                .py_1p5()
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
                                                        .child(t!("Flamegraph.Recording").to_string())
                                                )
                                        )
                                    })
                            )
                    )
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    .when(!has_data && !is_profiling, |this| {
                        // Empty state - no data and not profiling
                        this.child(self.render_empty_state(false, cx))
                    })
                    .when(is_profiling, |this| {
                        // Show overlay when profiling (whether or not there's data from previous sessions)
                        this.child(self.render_profiling_overlay(cx))
                    })
                    .when(has_data && !is_profiling, |this| {
                        // Show flamegraph viewer only when not profiling and data exists
                        this.child(
                            h_resizable("flamegraph-resizable", self.resizable_state.clone())
                                .child(
                                    resizable_panel()
                                        .child(self.view.clone())
                                        .size(px(800.0))
                                )
                                .child(
                                    resizable_panel()
                                        .when_some(self.statistics_panel.clone(), |panel, stats| {
                                            panel.child(stats)
                                        })
                                        .size(px(400.0))
                                )
                        )
                    })
            )
    }
}
