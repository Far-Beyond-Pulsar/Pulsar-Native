use crate::{
    FlamegraphPanel, FlamegraphView, InstrumentationCollector, StatisticsPanel, TraceData,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use rust_i18n::t;
use profiling::remote::{TargetConnection, TargetInfo};
use std::sync::Arc;
use ui::{
    button::Button,
    checkbox::Checkbox,
    h_flex,
    resizable::{h_resizable, resizable_panel, ResizableState},
    v_flex, ActiveTheme, Icon, IconName, TitleBar,
};

pub struct FlamegraphWindow {
    view: Entity<FlamegraphView>,
    collector: Option<Arc<InstrumentationCollector>>,
    trace_data: Arc<TraceData>,
    is_profiling: bool,
    current_db_path: Option<std::path::PathBuf>,
    db_connection: Option<rusqlite::Connection>,
    flamegraph_panel: Option<Entity<FlamegraphPanel>>,
    statistics_panel: Option<Entity<StatisticsPanel>>,
    resizable_state: Entity<ResizableState>,
    /// "Uncap frame rate while recording": lifts the engine's frame-rate target and
    /// vsync for the duration of the next recording.
    uncap_frame_rate: bool,
    /// Other profilable processes on this machine (games, other editors),
    /// refreshed every second (`profiling::remote`).
    targets: Vec<TargetInfo>,
    /// The process to record: `None` is this editor, in-process.
    selected_target: Option<u32>,
    /// What the current (or last) recording records.
    recording_label: String,
    /// Why the last start failed.
    start_error: Option<String>,
    _refresh_targets: Task<()>,
}

/// How often the start screen re-lists profilable processes.
const TARGET_REFRESH: std::time::Duration = std::time::Duration::from_secs(1);

/// Profilable processes other than this one.
fn other_targets() -> Vec<TargetInfo> {
    profiling::remote::list_targets().into_iter().filter(|t| !t.is_current_process()).collect()
}

fn uptime(started_unix_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let secs = now.saturating_sub(started_unix_ms) / 1000;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

impl Drop for FlamegraphWindow {
    fn drop(&mut self) {
        // Closing the window mid-recording must not leave the engine uncapped.
        profiling::set_uncap_frame_rate(false);
        gpui::render_stats::set_uncapped_presentation(false);
    }
}

impl FlamegraphWindow {
    pub fn new(trace_data: Arc<TraceData>, _window: &mut Window, cx: &mut App) -> Entity<Self> {
        // Clone the Arc so window and view share the same TraceData
        let view_trace_data = Arc::clone(&trace_data);
        let view = cx.new(move |_cx| FlamegraphView::new((*view_trace_data).clone()));

        cx.new(|cx| {
            let resizable_state = ResizableState::new(cx);
            let refresh = cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(TARGET_REFRESH).await;
                let targets = cx.background_executor().spawn(async { other_targets() }).await;
                if this.update(cx, |window: &mut FlamegraphWindow, cx| window.set_targets(targets, cx)).is_err() {
                    break;
                }
            });

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
                uncap_frame_rate: false,
                targets: other_targets(),
                selected_target: None,
                recording_label: String::new(),
                start_error: None,
                _refresh_targets: refresh,
            }
        })
    }

    /// Push the "uncap frame rate" choice to the engine: on only while a recording
    /// is running with the box ticked. Sets both halves of the cap: the Helio
    /// render thread's frame pacer, and vsync on the window swapchain.
    fn apply_frame_rate_cap(&self) {
        // A remote target lifts its own cap (the option travels with the
        // recording request); this editor stays as it is.
        let local = self.collector.as_ref().is_none_or(|c| !c.is_remote());
        let uncapped = self.is_profiling && self.uncap_frame_rate && local;
        profiling::set_uncap_frame_rate(uncapped);
        gpui::render_stats::set_uncapped_presentation(uncapped);
    }

    fn set_targets(&mut self, targets: Vec<TargetInfo>, cx: &mut Context<Self>) {
        self.targets = targets;
        // A selected process that exited falls back to this editor.
        if let Some(pid) = self.selected_target {
            if !self.targets.iter().any(|t| t.pid == pid) && !self.is_profiling {
                self.selected_target = None;
            }
        }
        if !self.is_profiling {
            cx.notify();
        }
    }

    fn selected_target(&self) -> Option<&TargetInfo> {
        let pid = self.selected_target?;
        self.targets.iter().find(|t| t.pid == pid)
    }

    fn selected_label(&self) -> String {
        match self.selected_target() {
            Some(target) => format!("{} (pid {})", target.name, target.pid),
            None => t!("Flamegraph.ThisEditor").to_string(),
        }
    }

    fn start_profiling(&mut self, _cx: &mut Context<Self>) {
        if self.is_profiling {
            return;
        }

        tracing::trace!("[PROFILER] Starting instrumentation collector");
        self.start_error = None;

        // This editor records in-process; any other target through its
        // shared-memory ring.
        let collector = match self.selected_target().cloned() {
            None => InstrumentationCollector::new(Arc::clone(&self.trace_data), 100),
            Some(target) => match TargetConnection::open(&target.path) {
                Ok(connection) => InstrumentationCollector::remote(
                    Arc::clone(&self.trace_data),
                    100,
                    connection,
                    self.uncap_frame_rate,
                ),
                Err(error) => {
                    self.start_error = Some(t!("Flamegraph.StartFailed", error => error.to_string()).to_string());
                    _cx.notify();
                    return;
                }
            },
        };
        if let Err(error) = collector.start() {
            self.start_error = Some(t!("Flamegraph.StartFailed", error => error).to_string());
            _cx.notify();
            return;
        }
        self.recording_label = self.selected_label();

        // Create database file in project directory
        if let Some(project_path) = engine_state::get_project_path() {
            match profiling::database::ensure_profiling_dir(&project_path) {
                Ok(profiling_dir) => {
                    let db_filename = profiling::database::generate_db_filename();
                    let db_path = profiling_dir.join(&db_filename);

                    match profiling::database::create_database(&db_path) {
                        Ok(conn) => {
                            tracing::trace!("[PROFILER] Created database: {}", db_path.display());
                            self.current_db_path = Some(db_path);
                            self.db_connection = Some(conn);
                        }
                        Err(e) => {
                            tracing::error!("[PROFILER] Failed to create database: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("[PROFILER] Failed to create profiling directory: {}", e);
                }
            }
        }

        self.collector = Some(Arc::new(collector));
        self.is_profiling = true;
        self.apply_frame_rate_cap();

        tracing::trace!("[PROFILER] Instrumentation profiling started");
        _cx.notify();
    }

    fn stop_profiling(&mut self, _cx: &mut Context<Self>) {
        if !self.is_profiling {
            return;
        }

        if let Some(collector) = &self.collector {
            collector.stop();
        }

        // Restore the normal frame-rate cap before anything slow (the save below)
        // so the editor is not left running uncapped.
        profiling::set_uncap_frame_rate(false);
        gpui::render_stats::set_uncapped_presentation(false);

        // Save all events to database before stopping
        if let Some(db_conn) = &self.db_connection {
            let events = match &self.collector {
                Some(collector) if collector.is_remote() => collector.session_events(),
                _ => profiling::get_all_events(),
            };
            if let Err(e) = profiling::database::save_events(db_conn, &events) {
                tracing::error!("[PROFILER] Failed to save events to database: {}", e);
            } else {
                tracing::trace!("[PROFILER] Saved {} events to database", events.len());
                if let Some(path) = &self.current_db_path {
                    tracing::trace!("[PROFILER] Database saved to: {}", path.display());
                }
            }
        }

        self.collector = None;
        self.is_profiling = false;

        tracing::trace!("[PROFILER] Instrumentation profiling stopped");
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
                    .and_then(|p| {
                        std::path::PathBuf::from(p)
                            .join(".pulsar/profiling/flamegraph")
                            .canonicalize()
                            .ok()
                    })
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
            );

        cx.spawn(async move |this, cx| {
            if let Some(file) = file_dialog.pick_file().await {
                let db_path = file.path().to_path_buf();
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.load_from_database(db_path, cx);
                    });
                });
            }
        })
        .detach();
    }

    fn load_from_database(&mut self, db_path: std::path::PathBuf, _cx: &mut Context<Self>) {
        let load_started = std::time::Instant::now();
        tracing::warn!(
            target: "flamegraph.workload",
            path = %db_path.display(),
            "beginning flamegraph database load"
        );
        match rusqlite::Connection::open(&db_path) {
            Ok(conn) => {
                match profiling::database::load_events(&conn) {
                    Ok(events) => {
                        let event_count = events.len();
                        tracing::trace!(
                            "[PROFILER] Loaded {} events from {}",
                            event_count,
                            db_path.display()
                        );

                        // Convert to TraceData format
                        if let Err(e) = crate::profiler::convert_profile_events_to_trace(
                            &events,
                            &self.trace_data,
                        ) {
                            tracing::error!("[PROFILER] Failed to convert events: {}", e);
                        }

                        self.current_db_path = Some(db_path);
                        tracing::warn!(
                            target: "flamegraph.workload",
                            events = event_count,
                            total_ms = load_started.elapsed().as_secs_f64() * 1000.0,
                            trace_stats = ?self.trace_data.debug_stats(),
                            "completed flamegraph database load"
                        );
                        _cx.notify();
                    }
                    Err(e) => {
                        tracing::error!("[PROFILER] Failed to load events from database: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("[PROFILER] Failed to open database: {}", e);
            }
        }
    }

    fn open_file_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(project_path) = engine_state::get_project_path() {
            // List available sessions
            match profiling::database::list_profiling_sessions(&project_path) {
                Ok(sessions) => {
                    tracing::trace!("[PROFILER] Found {} profiling sessions", sessions.len());
                    if let Some(latest) = sessions.first() {
                        // For now, just load the latest
                        // TODO: Show a UI list to pick from
                        self.load_from_database(latest.clone(), cx);
                    } else {
                        tracing::trace!("[PROFILER] No profiling sessions found");
                    }
                }
                Err(e) => {
                    tracing::error!("[PROFILER] Failed to list sessions: {}", e);
                }
            }
        }
    }

    /// One row of the target list.
    fn render_target_row(
        &self,
        key: Option<u32>,
        icon: IconName,
        title: String,
        detail: String,
        status: Option<(String, bool)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selectable = status.as_ref().is_none_or(|(_, ok)| *ok);
        let selected = self.selected_target == key;
        let listener = cx.listener(move |this, _event, _window, cx| {
            if selectable {
                this.selected_target = key;
                this.start_error = None;
                cx.notify();
            }
        });
        let theme = cx.theme();
        let accent = theme.accent;
        h_flex()
            .id(SharedString::from(format!("profiling-target-{}", key.unwrap_or(0))))
            .w_full()
            .px_4()
            .py_3()
            .gap_3()
            .items_center()
            .rounded(px(10.0))
            .border_1()
            .when(selected, |el| el.bg(accent.opacity(0.12)).border_color(accent.opacity(0.6)))
            .when(!selected, |el| el.bg(theme.popover).border_color(theme.border))
            .when(selectable && !selected, |el| {
                el.cursor_pointer().hover(|style| style.border_color(accent.opacity(0.3)))
            })
            .when(!selectable, |el| el.opacity(0.55))
            .on_mouse_down(MouseButton::Left, listener)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(36.0))
                    .flex_shrink_0()
                    .rounded(px(8.0))
                    .bg(accent.opacity(0.12))
                    .child(Icon::new(icon).size(px(20.0)).text_color(accent)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(title),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(detail)),
            )
            .when_some(status, |el, (label, ok)| {
                el.child(
                    div()
                        .flex_shrink_0()
                        .px_2()
                        .py_0p5()
                        .rounded(px(6.0))
                        .text_xs()
                        .when(ok, |el| el.bg(gpui::green().opacity(0.12)).text_color(gpui::green()))
                        .when(!ok, |el| el.bg(theme.muted.opacity(0.2)).text_color(theme.muted_foreground))
                        .child(label),
                )
            })
            .into_any_element()
    }

    /// The start screen's list of profilable processes: this editor first,
    /// then every game and editor found on this machine.
    fn render_targets(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = engine_state::get_project_path().unwrap_or_default();
        let mut rows = vec![self.render_target_row(
            None,
            IconName::LayoutDashboard,
            t!("Flamegraph.ThisEditor").to_string(),
            format!("editor · pid {} · {project}", std::process::id()),
            None,
            cx,
        )];
        for target in self.targets.clone() {
            let (icon, kind) = match target.kind.as_str() {
                "game" => (IconName::Gamepad, t!("Flamegraph.KindGame").to_string()),
                "editor" => (IconName::LayoutDashboard, t!("Flamegraph.KindEditor").to_string()),
                other => (IconName::Cpu, other.to_owned()),
            };
            let status = if !target.responsive() {
                (t!("Flamegraph.TargetNotResponding").to_string(), false)
            } else if target.viewer_pid.is_some() {
                (t!("Flamegraph.TargetBusy").to_string(), false)
            } else {
                (t!("Flamegraph.TargetAvailable").to_string(), true)
            };
            let detail = format!(
                "{kind} · pid {} · {} · {}",
                target.pid,
                t!("Flamegraph.TargetUptime", time => uptime(target.started_unix_ms)),
                target.project
            );
            rows.push(self.render_target_row(Some(target.pid), icon, target.name.clone(), detail, Some(status), cx));
        }
        let no_others = self.targets.is_empty();
        let error = self.start_error.clone();
        let theme = cx.theme();
        v_flex()
            .w_full()
            .gap_2()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(t!("Flamegraph.ChooseTarget").to_string().to_uppercase()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground.opacity(0.8))
                            .child(t!("Flamegraph.ChooseTargetDesc").to_string()),
                    ),
            )
            .children(rows)
            .when(no_others, |el| {
                el.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("Flamegraph.NoOtherTargets").to_string()),
                )
            })
            .when_some(error, |el, error| {
                el.child(div().px_4().py_2().text_sm().text_color(gpui::red()).child(error))
            })
            .into_any_element()
    }

    fn render_empty_state(
        &mut self,
        is_profiling: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let targets = (!is_profiling).then(|| self.render_targets(cx));
        let record_title = t!("Flamegraph.RecordTarget", target => self.selected_label()).to_string();
        let recording_title = t!("Flamegraph.RecordingTarget", target => self.recording_label.clone()).to_string();
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
                                    .text_color(theme.muted_foreground.opacity(0.4)),
                            ),
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
                                    }),
                            )
                            .child(div().text_base().text_color(theme.muted_foreground).child(
                                if is_profiling {
                                    t!("Flamegraph.WaitingForData").to_string()
                                } else {
                                    t!("Flamegraph.GetStarted").to_string()
                                },
                            )),
                    ),
            )
            .when(!is_profiling, |this| {
                this.child(
                    v_flex()
                        .gap_3()
                        .w(px(560.0))
                        .children(targets)
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
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.start_profiling(cx);
                                    }),
                                )
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
                                                .text_color(gpui::red()),
                                        ),
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
                                                .child(record_title),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .line_height(relative(1.4))
                                                .child(
                                                    t!("Flamegraph.StartRecordingDesc").to_string(),
                                                ),
                                        ),
                                ),
                        )
                        .child(
                            // Recording option, shown with the start card it applies to.
                            // Not inside the clickable start card: clicking the checkbox
                            // must toggle the option, not start a recording.
                            h_flex()
                                .w_full()
                                .px_5()
                                .py_3()
                                .gap_3()
                                .items_start()
                                .rounded(px(10.0))
                                .bg(theme.muted.opacity(0.08))
                                .border_1()
                                .border_color(theme.border.opacity(0.5))
                                .child(
                                    Checkbox::new("uncap-frame-rate")
                                        .checked(self.uncap_frame_rate)
                                        .on_click(cx.listener(|this, checked: &bool, _window, cx| {
                                            this.uncap_frame_rate = *checked;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(theme.foreground)
                                                .child(t!("Flamegraph.UncapFrameRate").to_string()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .line_height(relative(1.4))
                                                .child(t!("Flamegraph.UncapFrameRateDesc").to_string()),
                                        ),
                                ),
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
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.open_database_picker(cx);
                                    }),
                                )
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
                                                .text_color(accent_color),
                                        ),
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
                                                .child(
                                                    t!("Flamegraph.OpenPreviousSession")
                                                        .to_string(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .line_height(relative(1.4))
                                                .child(
                                                    t!("Flamegraph.OpenSessionDesc").to_string(),
                                                ),
                                        ),
                                ),
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
                                                .child(
                                                    t!("Flamegraph.ProTips")
                                                        .to_string()
                                                        .to_uppercase(),
                                                ),
                                        )
                                        .child(
                                            v_flex()
                                                .gap_2()
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_start()
                                                        .child(div().mt_0p5().text_sm().child("•"))
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .text_color(theme.muted_foreground)
                                                                .line_height(relative(1.5))
                                                                .child(
                                                                    t!("Flamegraph.Tip1")
                                                                        .to_string(),
                                                                ),
                                                        ),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_start()
                                                        .child(div().mt_0p5().text_sm().child("•"))
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .text_color(theme.muted_foreground)
                                                                .line_height(relative(1.5))
                                                                .child(
                                                                    t!("Flamegraph.Tip2")
                                                                        .to_string(),
                                                                ),
                                                        ),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_start()
                                                        .child(div().mt_0p5().text_sm().child("•"))
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .text_color(theme.muted_foreground)
                                                                .line_height(relative(1.5))
                                                                .child(
                                                                    t!("Flamegraph.Tip3")
                                                                        .to_string(),
                                                                ),
                                                        ),
                                                ),
                                        ),
                                ),
                        ),
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
                                .child(div().size(px(10.0)).rounded(px(5.0)).bg(gpui::red()).child(
                                    div().size(px(10.0)).rounded(px(5.0)).bg(gpui::red()), // Simple pulse animation via opacity
                                ))
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme.foreground)
                                        .child(recording_title),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(t!("Flamegraph.DataWillAppear").to_string()),
                        ),
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
                v_flex().gap_6().items_center().child(
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
                                .child(div().size(px(10.0)).rounded(px(5.0)).bg(gpui::red()))
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(gpui::red())
                                        .child(t!("Flamegraph.CollectingData").to_string()),
                                ),
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
                                                .child(t!("Flamegraph.SpansCollected").to_string()),
                                        )
                                        .child(
                                            div()
                                                .text_base()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(theme.foreground)
                                                .child(format!("{}", span_count)),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_base()
                                                .text_color(theme.muted_foreground)
                                                .child(t!("Flamegraph.Threads").to_string()),
                                        )
                                        .child(
                                            div()
                                                .text_base()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(theme.foreground)
                                                .child(format!("{}", thread_count)),
                                        ),
                                ),
                        )
                        .child(
                            Button::new("stop-recording-btn")
                                .w_full()
                                .label(t!("Flamegraph.StopRecording").to_string())
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.stop_profiling(cx);
                                })),
                        ),
                ),
            )
    }
}

impl Render for FlamegraphWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_profiling = self.is_profiling;
        let frame = self.trace_data.get_frame();
        // Only show data when NOT profiling - during profiling, data is being collected but not displayed
        let has_data = !is_profiling && !frame.spans.is_empty();

        // Initialize panels on first render with data (when profiling stops)
        if has_data && self.flamegraph_panel.is_none() {
            self.flamegraph_panel = Some(cx.new(|cx| FlamegraphPanel::new(self.view.clone(), cx)));
            self.statistics_panel =
                Some(cx.new(|cx| StatisticsPanel::new(self.trace_data.clone(), cx)));
        }

        let theme = cx.theme();
        let summary_bar = if has_data || is_profiling {
            Some(
                div()
                    .w_full()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .flex()
                    .items_center()
                    .bg(theme.sidebar.opacity(0.9))
                    .border_b_1()
                    .border_color(theme.border.opacity(0.8))
                    .shadow_sm()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2p5()
                            .py_1()
                            .rounded(px(6.0))
                            .bg(if is_profiling {
                                gpui::red().opacity(0.15)
                            } else {
                                theme.accent.opacity(0.12)
                            })
                            .border_1()
                            .border_color(if is_profiling {
                                gpui::red().opacity(0.25)
                            } else {
                                theme.accent.opacity(0.2)
                            })
                            .child(div().size(px(8.0)).rounded_full().bg(if is_profiling {
                                gpui::red()
                            } else {
                                theme.accent
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(if is_profiling {
                                        gpui::red()
                                    } else {
                                        theme.accent
                                    })
                                    .child(if is_profiling {
                                        t!("Flamegraph.Recording").to_string()
                                    } else {
                                        "Session Ready".to_string()
                                    }),
                            ),
                    )
                    .child(self.summary_chip(
                        "Spans",
                        format!("{}", frame.spans.len()),
                        theme.foreground,
                        &theme,
                    ))
                    .child(self.summary_chip(
                        "Threads",
                        format!("{}", frame.threads.len()),
                        theme.foreground,
                        &theme,
                    ))
                    .child(self.summary_chip(
                        "Frames",
                        format!("{}", frame.frame_times_ms.len()),
                        theme.foreground,
                        &theme,
                    ))
                    .child(self.summary_chip(
                        "Duration",
                        format!("{:.2}ms", frame.duration_ns() as f64 / 1_000_000.0),
                        theme.foreground,
                        &theme,
                    ))
                    .when(self.current_db_path.is_some(), |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .px_2p5()
                                .py_1()
                                .rounded(px(6.0))
                                .bg(theme.accent.opacity(0.08))
                                .border_1()
                                .border_color(theme.accent.opacity(0.16))
                                .child(Icon::new(IconName::Database).size(px(12.0)))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.foreground)
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(
                                            self.current_db_path
                                                .as_ref()
                                                .and_then(|p| p.file_name())
                                                .and_then(|n| n.to_str())
                                                .unwrap_or("Unknown")
                                                .to_string(),
                                        ),
                                ),
                        )
                    }),
            )
        } else {
            None
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                TitleBar::new().child(
                    h_flex().gap_4().items_center().flex_1().child(
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
                                            .child("Flamegraph Profiler"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme.muted_foreground)
                                            .child("• Instrumentation-Based"),
                                    ),
                            )
                            .child(div().flex_1()),
                    ),
                ),
            )
            .when_some(summary_bar, |this, bar| this.child(bar))
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
                            h_resizable("flamegraph-resizable")
                                .state(self.resizable_state.clone())
                                .child(resizable_panel().child(self.view.clone()).size(px(800.0)))
                                .child(
                                    resizable_panel()
                                        .when_some(self.statistics_panel.clone(), |panel, stats| {
                                            panel.child(stats)
                                        })
                                        .size(px(400.0)),
                                ),
                        )
                    }),
            )
    }
}

impl FlamegraphWindow {
    fn summary_chip(
        &self,
        label: &str,
        value: String,
        value_color: Hsla,
        theme: &ui::theme::Theme,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .rounded(px(6.0))
            .bg(theme.background.opacity(0.45))
            .border_1()
            .border_color(theme.border.opacity(0.5))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(value_color)
                    .child(value),
            )
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for FlamegraphWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "FlamegraphWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1200.0, 800.0)
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        FlamegraphWindow::new(std::sync::Arc::new(crate::TraceData::new()), window, cx)
    }
}
