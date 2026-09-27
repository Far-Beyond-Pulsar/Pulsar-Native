//! The profiler's start screen: choose a process to record (this editor, a
//! running game, another editor), set up the recording, or reopen a saved
//! session.
//!
//! Layout, top to bottom: a toolbar (search, kind filter, refresh, open
//! session), the body (a scrollable target list, and a sidebar with the
//! selected target's details, the recording options and recent sessions)
//! and a status line. Every part scrolls or truncates on its own, so the
//! screen never grows past the window.

use super::*;
use std::path::PathBuf;
use std::time::SystemTime;
use ui::{
    button::ButtonVariants, input::TextInput, scroll::ScrollbarAxis, Disableable, Selectable,
    Sizable, StyledExt,
};

/// Recent sessions listed in the sidebar.
const MAX_RECENT: usize = 8;
/// Auto-stop choices, in seconds (`None`: record until stopped).
const AUTO_STOP_CHOICES: [Option<u64>; 5] = [None, Some(5), Some(10), Some(30), Some(60)];
const SIDEBAR_WIDTH: f32 = 340.0;

/// Which kinds of target the list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KindFilter {
    All,
    Games,
    Editors,
}

impl KindFilter {
    fn label(self) -> String {
        match self {
            KindFilter::All => t!("Flamegraph.FilterAll").to_string(),
            KindFilter::Games => t!("Flamegraph.FilterGames").to_string(),
            KindFilter::Editors => t!("Flamegraph.FilterEditors").to_string(),
        }
    }

    fn admits(self, kind: &str) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Games => kind == "game",
            KindFilter::Editors => kind == "editor",
        }
    }
}

/// A saved session of the current project.
#[derive(Clone, Debug)]
pub(super) struct SessionFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}

/// Other profilable processes and the project's saved sessions. Does file
/// IO; the window runs it on the background executor.
pub(super) fn scan() -> (Vec<TargetInfo>, Vec<SessionFile>) {
    let targets = profiling::remote::list_targets().into_iter().filter(|t| !t.is_current_process()).collect();
    let sessions = engine_state::get_project_path()
        .and_then(|project| profiling::database::list_profiling_sessions(&project).ok())
        .unwrap_or_default()
        .into_iter()
        .take(MAX_RECENT)
        .map(|path| {
            let meta = std::fs::metadata(&path).ok();
            SessionFile {
                bytes: meta.as_ref().map_or(0, |m| m.len()),
                modified: meta.and_then(|m| m.modified().ok()),
                path,
            }
        })
        .collect();
    (targets, sessions)
}

/// Whether a target can be recorded, and how to say so.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Availability {
    Available,
    /// This window is recording it.
    RecordingHere,
    /// Another profiler controls it.
    Busy,
    NotResponding,
}

impl Availability {
    fn of(target: &TargetInfo) -> Self {
        if !target.responsive() {
            Availability::NotResponding
        } else if target.viewer_pid == Some(std::process::id()) {
            Availability::RecordingHere
        } else if target.viewer_pid.is_some() {
            Availability::Busy
        } else {
            Availability::Available
        }
    }

    fn label(self) -> String {
        match self {
            Availability::Available => t!("Flamegraph.TargetAvailable").to_string(),
            Availability::RecordingHere => t!("Flamegraph.TargetRecordingHere").to_string(),
            Availability::Busy => t!("Flamegraph.TargetBusy").to_string(),
            Availability::NotResponding => t!("Flamegraph.TargetNotResponding").to_string(),
        }
    }

    fn color(self, cx: &App) -> Hsla {
        match self {
            Availability::Available => gpui::green(),
            Availability::RecordingHere => gpui::red(),
            Availability::Busy => gpui::yellow(),
            Availability::NotResponding => cx.theme().muted_foreground,
        }
    }

    fn selectable(self) -> bool {
        self == Availability::Available
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn duration_text(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

fn uptime(started_unix_ms: u64) -> String {
    duration_text(now_unix_ms().saturating_sub(started_unix_ms) / 1000)
}

fn ago(time: Option<SystemTime>) -> String {
    let Some(secs) = time.and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()) else {
        return "—".into();
    };
    match secs {
        0..=59 => t!("Flamegraph.JustNow").to_string(),
        60..=3599 => t!("Flamegraph.MinutesAgo", n => secs / 60).to_string(),
        3600..=86_399 => t!("Flamegraph.HoursAgo", n => secs / 3600).to_string(),
        _ => t!("Flamegraph.DaysAgo", n => secs / 86_400).to_string(),
    }
}

fn file_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

fn kind_icon(kind: &str) -> IconName {
    match kind {
        "game" => IconName::Gamepad,
        "editor" => IconName::LayoutDashboard,
        _ => IconName::Cpu,
    }
}

fn kind_label(kind: &str) -> String {
    match kind {
        "game" => t!("Flamegraph.KindGame").to_string(),
        "editor" => t!("Flamegraph.KindEditor").to_string(),
        other => other.to_owned(),
    }
}

impl FlamegraphWindow {
    pub(super) fn render_start_screen(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(self.render_toolbar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.render_target_list(cx))
                    .child(self.render_sidebar(cx)),
            )
            .child(self.render_status_line(cx))
            .into_any_element()
    }

    // ---- toolbar ----------------------------------------------------------------

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let filters = [KindFilter::All, KindFilter::Games, KindFilter::Editors].map(|filter| {
            Button::new(SharedString::from(format!("kind-filter-{filter:?}")))
                .label(filter.label())
                .small()
                .ghost()
                .selected(self.kind_filter == filter)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind_filter = filter;
                    cx.notify();
                }))
        });
        h_flex()
            .w_full()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                div().w(px(300.0)).child(
                    TextInput::new(&self.search)
                        .small()
                        .cleanable()
                        .prefix(Icon::new(IconName::Search).size(px(14.0)).text_color(theme.muted_foreground)),
                ),
            )
            .child(h_flex().gap_1().children(filters))
            .child(div().flex_1())
            .child(
                Button::new("targets-refresh")
                    .icon(IconName::Refresh)
                    .small()
                    .ghost()
                    .tooltip(t!("Flamegraph.RefreshTargets").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_now(cx))),
            )
            .child(
                Button::new("open-session")
                    .icon(IconName::FolderOpen)
                    .label(t!("Flamegraph.OpenSession").to_string())
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, _, cx| this.open_database_picker(cx))),
            )
            .into_any_element()
    }

    // ---- target list -----------------------------------------------------------

    /// The search box's text, lowercased.
    fn search_query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn matches(query: &str, fields: &[&str]) -> bool {
        query.is_empty() || fields.iter().any(|field| field.to_lowercase().contains(query))
    }

    fn render_target_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.search_query(cx);
        let project = engine_state::get_project_path().unwrap_or_default();
        let editor_label = t!("Flamegraph.ThisEditor").to_string();
        let pid = std::process::id().to_string();
        let show_editor = self.kind_filter != KindFilter::Games
            && Self::matches(&query, &[&editor_label, &project, &pid, "editor"]);

        let mut others: Vec<TargetInfo> = self
            .targets
            .iter()
            .filter(|t| self.kind_filter.admits(&t.kind))
            .filter(|t| Self::matches(&query, &[&t.name, &t.project, &t.kind, &t.pid.to_string()]))
            .cloned()
            .collect();
        // Recordable first, then newest.
        others.sort_by_key(|t| (!Availability::of(t).selectable(), std::cmp::Reverse(t.started_unix_ms)));
        let games = others.iter().filter(|t| t.kind == "game").count();

        let mut list = v_flex().w_full().gap_1p5();
        if show_editor {
            list = list.child(self.render_section_header(t!("Flamegraph.SectionThisProcess").to_string(), None, cx));
            let row = self.render_editor_row(&project, cx);
            list = list.child(row);
        }
        list = list.child(self.render_section_header(
            t!("Flamegraph.SectionRunning").to_string(),
            Some(format!("{} · {}", others.len(), t!("Flamegraph.GameCount", n => games))),
            cx,
        ));
        if others.is_empty() {
            list = list.child(self.render_no_targets(!query.is_empty() || self.kind_filter != KindFilter::All, cx));
        }
        for target in others {
            let row = self.render_target_row(target, cx);
            list = list.child(row);
        }

        div()
            .id("profiling-target-list")
            .flex_1()
            .min_w_0()
            .h_full()
            .child(v_flex().size_full().p_3().child(list).scrollable(ScrollbarAxis::Vertical))
            .into_any_element()
    }

    fn render_section_header(&self, title: String, detail: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .w_full()
            .pt_2()
            .pb_0p5()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(title.to_uppercase()),
            )
            .when_some(detail, |el, detail| {
                el.child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(detail))
            })
            .into_any_element()
    }

    /// Shared row chrome: selection, hover, click to select, double-click
    /// to record.
    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        key: Option<u32>,
        icon: IconName,
        title: String,
        details: Vec<String>,
        subtitle: String,
        status: Option<Availability>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selectable = status.is_none_or(Availability::selectable);
        let selected = self.selected_target == key;
        let on_down = cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
            if !selectable {
                return;
            }
            this.selected_target = key;
            this.start_error = None;
            if event.click_count >= 2 {
                this.start_profiling(cx);
            }
            cx.notify();
        });
        let theme = cx.theme();
        let accent = theme.accent;
        let status_color = status.map(|s| s.color(cx));
        let theme = cx.theme();
        h_flex()
            .id(SharedString::from(format!("profiling-target-{}", key.unwrap_or(0))))
            .w_full()
            .px_3()
            .py_2p5()
            .gap_3()
            .items_center()
            .rounded(px(8.0))
            .border_1()
            .when(selected, |el| el.bg(accent.opacity(0.12)).border_color(accent.opacity(0.7)))
            .when(!selected, |el| el.bg(theme.popover).border_color(theme.border.opacity(0.7)))
            .when(selectable && !selected, |el| el.cursor_pointer().hover(|s| s.border_color(accent.opacity(0.35))))
            .when(!selectable, |el| el.opacity(0.6))
            .on_mouse_down(MouseButton::Left, on_down)
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .size(px(34.0))
                    .rounded(px(8.0))
                    .bg(accent.opacity(0.12))
                    .child(Icon::new(icon).size(px(18.0)).text_color(accent)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .truncate()
                                    .child(title),
                            )
                            .children(details.into_iter().map(|detail| {
                                div()
                                    .flex_shrink_0()
                                    .px_1p5()
                                    .rounded(px(4.0))
                                    .bg(theme.muted.opacity(0.18))
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(detail)
                            })),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(subtitle)),
            )
            .when_some(status.zip(status_color), |el, (status, color)| {
                el.child(
                    h_flex()
                        .flex_shrink_0()
                        .gap_1p5()
                        .items_center()
                        .px_2()
                        .py_0p5()
                        .rounded(px(10.0))
                        .bg(color.opacity(0.12))
                        .child(div().size(px(7.0)).rounded_full().bg(color))
                        .child(div().text_xs().text_color(color).child(status.label())),
                )
            })
            .into_any_element()
    }

    fn render_editor_row(&self, project: &str, cx: &mut Context<Self>) -> AnyElement {
        self.render_row(
            None,
            IconName::LayoutDashboard,
            t!("Flamegraph.ThisEditor").to_string(),
            vec![kind_label("editor"), format!("pid {}", std::process::id())],
            if project.is_empty() { t!("Flamegraph.InProcess").to_string() } else { project.to_owned() },
            None,
            cx,
        )
    }

    fn render_target_row(&self, target: TargetInfo, cx: &mut Context<Self>) -> AnyElement {
        let status = Availability::of(&target);
        self.render_row(
            Some(target.pid),
            kind_icon(&target.kind),
            target.name.clone(),
            vec![
                kind_label(&target.kind),
                format!("pid {}", target.pid),
                t!("Flamegraph.TargetUptime", time => uptime(target.started_unix_ms)).to_string(),
            ],
            if target.project.is_empty() { "—".into() } else { target.project.clone() },
            Some(status),
            cx,
        )
    }

    fn render_no_targets(&self, filtered: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .w_full()
            .p_4()
            .gap_2()
            .rounded(px(8.0))
            .border_1()
            .border_dashed()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Info).size(px(14.0)).text_color(theme.muted_foreground))
                    .child(div().text_sm().text_color(theme.foreground).child(if filtered {
                        t!("Flamegraph.NoMatchingTargets").to_string()
                    } else {
                        t!("Flamegraph.NoOtherTargets").to_string()
                    })),
            )
            .when(!filtered, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("Flamegraph.HowToPublish", flag => profiling::remote::ARG_FLAG, env => profiling::remote::ENV_FLAG).to_string()),
                )
            })
            .into_any_element()
    }

    // ---- sidebar -----------------------------------------------------------------

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let details = self.render_details(cx);
        let options = self.render_recording_options(cx);
        let sessions = self.render_recent_sessions(cx);
        div()
            .id("profiling-sidebar")
            .w(px(SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                v_flex()
                    .size_full()
                    .p_3()
                    .gap_4()
                    .child(details)
                    .child(options)
                    .child(sessions)
                    .scrollable(ScrollbarAxis::Vertical),
            )
            .into_any_element()
    }

    fn sidebar_section(title: String, body: impl IntoElement, cx: &App) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(title.to_uppercase()),
            )
            .child(body)
            .into_any_element()
    }

    fn detail_row(label: String, value: String, cx: &App) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .w_full()
            .gap_3()
            .justify_between()
            .child(div().flex_shrink_0().text_xs().text_color(theme.muted_foreground).child(label))
            .child(div().min_w_0().text_xs().text_color(theme.foreground).truncate().child(value))
            .into_any_element()
    }

    fn render_details(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<(String, String)> = match self.selected_target() {
            None => vec![
                (t!("Flamegraph.DetailName").to_string(), t!("Flamegraph.ThisEditor").to_string()),
                (t!("Flamegraph.DetailKind").to_string(), kind_label("editor")),
                (t!("Flamegraph.DetailPid").to_string(), std::process::id().to_string()),
                (
                    t!("Flamegraph.DetailProject").to_string(),
                    engine_state::get_project_path().unwrap_or_else(|| "—".into()),
                ),
                (t!("Flamegraph.DetailTransport").to_string(), t!("Flamegraph.InProcess").to_string()),
            ],
            Some(target) => {
                let heartbeat = if target.responsive() {
                    format!("{} ms", target.heartbeat_age_ms)
                } else {
                    t!("Flamegraph.TargetNotResponding").to_string()
                };
                vec![
                    (t!("Flamegraph.DetailName").to_string(), target.name.clone()),
                    (t!("Flamegraph.DetailKind").to_string(), kind_label(&target.kind)),
                    (t!("Flamegraph.DetailPid").to_string(), target.pid.to_string()),
                    (
                        t!("Flamegraph.DetailProject").to_string(),
                        if target.project.is_empty() { "—".into() } else { target.project.clone() },
                    ),
                    (t!("Flamegraph.DetailUptime").to_string(), uptime(target.started_unix_ms)),
                    (t!("Flamegraph.DetailHeartbeat").to_string(), heartbeat),
                    (t!("Flamegraph.DetailStatus").to_string(), Availability::of(target).label()),
                    (
                        t!("Flamegraph.DetailViewer").to_string(),
                        target.viewer_pid.map_or_else(|| "—".into(), |pid| format!("pid {pid}")),
                    ),
                    (t!("Flamegraph.DetailTransport").to_string(), t!("Flamegraph.SharedMemory").to_string()),
                ]
            }
        };
        let body = v_flex()
            .gap_1p5()
            .p_3()
            .rounded(px(8.0))
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border.opacity(0.7))
            .children(rows.into_iter().map(|(label, value)| Self::detail_row(label, value, cx)));
        Self::sidebar_section(t!("Flamegraph.SectionTarget").to_string(), body, cx)
    }

    fn render_recording_options(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let can_record = self.selected_target().is_none_or(|t| Availability::of(t).selectable());
        let record_title = t!("Flamegraph.RecordTarget", target => self.selected_label()).to_string();
        let auto_stop = AUTO_STOP_CHOICES.map(|choice| {
            let label = match choice {
                None => t!("Flamegraph.AutoStopOff").to_string(),
                Some(secs) => format!("{secs}s"),
            };
            Button::new(SharedString::from(format!("auto-stop-{}", choice.unwrap_or(0))))
                .label(label)
                .xsmall()
                .ghost()
                .selected(self.auto_stop_secs == choice)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.auto_stop_secs = choice;
                    cx.notify();
                }))
        });
        let error = self.start_error.clone();
        let theme = cx.theme().clone();
        let body = v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
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
                            .gap_0p5()
                            .child(div().text_sm().text_color(theme.foreground).child(t!("Flamegraph.UncapFrameRate").to_string()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("Flamegraph.UncapFrameRateShort").to_string()),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().text_color(theme.foreground).child(t!("Flamegraph.AutoStop").to_string()))
                    .child(h_flex().gap_1().children(auto_stop)),
            )
            .child(
                Button::new("start-recording")
                    .icon(IconName::Circle)
                    .label(record_title)
                    .primary()
                    .w_full()
                    .disabled(!can_record)
                    .on_click(cx.listener(|this, _, _, cx| this.start_profiling(cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("Flamegraph.DoubleClickHint").to_string()),
            )
            .when_some(error, |el, error| el.child(div().text_xs().text_color(gpui::red()).child(error)));
        Self::sidebar_section(t!("Flamegraph.SectionRecording").to_string(), body, cx)
    }

    fn render_recent_sessions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let sessions = self.recent_sessions.clone();
        let theme = cx.theme().clone();
        let mut body = v_flex().gap_1();
        if sessions.is_empty() {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("Flamegraph.NoRecentSessions").to_string()),
            );
        }
        for (index, session) in sessions.into_iter().enumerate() {
            let path = session.path.clone();
            let name = session
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            body = body.child(
                h_flex()
                    .id(SharedString::from(format!("recent-session-{index}")))
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.accent.opacity(0.08)))
                    .on_click(cx.listener(move |this, _, _, cx| this.load_from_database(path.clone(), cx)))
                    .child(Icon::new(IconName::Database).size(px(14.0)).text_color(theme.muted_foreground))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_xs().text_color(theme.foreground).truncate().child(name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} · {}", ago(session.modified), file_size(session.bytes))),
                            ),
                    ),
            );
        }
        Self::sidebar_section(t!("Flamegraph.SectionRecent").to_string(), body, cx)
    }

    // ---- status line --------------------------------------------------------------

    fn render_status_line(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let available = self.targets.iter().filter(|t| Availability::of(t).selectable()).count();
        h_flex()
            .w_full()
            .flex_shrink_0()
            .px_3()
            .py_1()
            .gap_3()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(t!(
                "Flamegraph.StatusTargets",
                total => self.targets.len() + 1,
                available => available + 1
            ).to_string())
            .child(t!(
                "Flamegraph.StatusRefreshed",
                ago => duration_text(self.last_refresh.elapsed().as_secs())
            ).to_string())
            .child(div().flex_1())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(t!("Flamegraph.StatusDir", dir => profiling::remote::default_dir().display().to_string()).to_string()),
            )
            .into_any_element()
    }
}
