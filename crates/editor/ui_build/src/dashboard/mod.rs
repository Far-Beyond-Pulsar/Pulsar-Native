//! The build dashboard: a window that opens when a build starts and shows
//! it while it runs and after it ends.
//!
//! - **Output**: cargo's output, the build task's own log, in Mission
//!   Control's log view (`ui_log_viewer::LogDrawer::for_task`: the same
//!   table, level filters and search as the engine log and the Tasks
//!   window).
//! - **Processes**: the running cargo command and everything it started
//!   (rustc per crate, build scripts, linkers), busiest first.
//! - **Resources**: the build's CPU and memory, and the machine's, over time.
//!
//! It follows the latest build ([`crate::runner::session::current`]): a new
//! build in an open dashboard replaces the old one in place. Sampling runs
//! off the UI thread, once a second, and the process tree is only walked
//! while a cargo command runs.

mod sampler;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use parking_lot::Mutex;
use rust_i18n::t;
use ui::button::{Button, ButtonVariants as _};
use ui::chart::AreaChart;
use ui::progress::Progress;
use ui::resizable::{ResizableState, h_resizable, resizable_panel};
use ui::scroll::ScrollbarAxis;
use ui::{ActiveTheme as _, IconName, StyledExt as _, TitleBar, h_flex, v_flex};
use ui_log_viewer::LogDrawer;

use crate::runner::session::{self, BuildSession, Outcome};
use sampler::{ProcessKind, ProcessRow, Sample, Sampler};

/// Window name in the window registry.
pub const WINDOW_NAME: &str = "BuildDashboard";

/// Samples kept for the charts: three minutes at one a second.
const HISTORY: usize = 180;
const SAMPLE_EVERY: Duration = Duration::from_secs(1);
const SIDEBAR_WIDTH: f32 = 400.0;

/// Open the dashboard, or bring the open one to the front.
pub fn open(cx: &mut App) {
    use ui_common::PulsarWindowExt as _;
    BuildDashboard::open((), cx);
}

/// One chart's samples.
#[derive(Clone, Copy)]
struct Point(f64);

#[derive(Default)]
struct History {
    build_cpu: VecDeque<Point>,
    build_memory: VecDeque<Point>,
    system_cpu: VecDeque<Point>,
    system_memory: VecDeque<Point>,
    peak_build_memory: u64,
    peak_processes: usize,
}

impl History {
    fn push(&mut self, sample: &Sample, running: bool) {
        let push = |series: &mut VecDeque<Point>, value: f64| {
            if series.len() == HISTORY {
                series.pop_front();
            }
            series.push_back(Point(value));
        };
        if running {
            push(&mut self.build_cpu, f64::from(sample.build_cpu));
            push(&mut self.build_memory, mib(sample.build_memory));
            self.peak_build_memory = self.peak_build_memory.max(sample.build_memory);
            self.peak_processes = self.peak_processes.max(sample.processes.len());
        }
        push(&mut self.system_cpu, f64::from(sample.system_cpu));
        push(&mut self.system_memory, mib(sample.system_memory_used));
    }
}

pub struct BuildDashboard {
    focus_handle: FocusHandle,
    /// The build shown, and its output.
    shown: Option<(Arc<BuildSession>, Entity<LogDrawer>)>,
    sample: Sample,
    history: History,
    split: Entity<ResizableState>,
    _poll: Task<()>,
}

impl BuildDashboard {
    fn new(cx: &mut Context<Self>) -> Self {
        let sampler = Arc::new(Mutex::new(Sampler::new()));
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let root = session::current().and_then(|s| s.cargo_pid());
                let sampler = Arc::clone(&sampler);
                let sample = cx
                    .background_executor()
                    .spawn(async move { sampler.lock().sample(root) })
                    .await;
                let alive = cx.update(|cx| {
                    let Some(this) = this.upgrade() else {
                        return false;
                    };
                    this.update(cx, |dashboard, cx| dashboard.apply(sample, root.is_some(), cx));
                    true
                });
                if !alive {
                    break;
                }
                cx.background_executor().timer(SAMPLE_EVERY).await;
            }
        });
        let mut dashboard = Self {
            focus_handle: cx.focus_handle(),
            shown: None,
            sample: Sample::default(),
            history: History::default(),
            split: ResizableState::new(cx),
            _poll: poll,
        };
        dashboard.follow_latest(cx);
        dashboard
    }

    /// Show the latest build, if it is not the one shown.
    fn follow_latest(&mut self, cx: &mut Context<Self>) {
        let Some(latest) = session::current() else {
            return;
        };
        if self
            .shown
            .as_ref()
            .is_some_and(|(shown, _)| Arc::ptr_eq(shown, &latest))
        {
            return;
        }
        let Some(log) = latest.task().and_then(|id| editor_task_queue::global().log(id)) else {
            return;
        };
        let drawer = cx.new(|cx| LogDrawer::for_task(log, cx));
        drawer.update(cx, |drawer, cx| drawer.start_monitoring(cx));
        self.shown = Some((latest, drawer));
        self.history = History::default();
        self.sample = Sample::default();
    }

    fn apply(&mut self, sample: Sample, running: bool, cx: &mut Context<Self>) {
        self.follow_latest(cx);
        self.history.push(&sample, running);
        self.sample = sample;
        cx.notify();
    }
}

impl Focusable for BuildDashboard {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BuildDashboard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let title = match &self.shown {
            Some((build, _)) => t!("Build.Dashboard.TitleFor", name = build.name).to_string(),
            None => t!("Build.Dashboard.Title").to_string(),
        };
        let body = match &self.shown {
            None => empty_state(cx).into_any_element(),
            Some((build, drawer)) => v_flex()
                .size_full()
                .child(header(build, cx))
                .child(
                    div().flex_1().min_h_0().w_full().child(
                        h_resizable("build-dashboard-split")
                            .state(self.split.clone())
                            .child(resizable_panel().child(drawer.clone()))
                            .child(
                                resizable_panel()
                                    .size(px(SIDEBAR_WIDTH))
                                    .size_range(px(300.0)..px(900.0))
                                    .child(self.sidebar(build.outcome(), cx)),
                            ),
                    ),
                )
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .flex_1()
                        .items_center()
                        .px_4()
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(title),
                ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl BuildDashboard {
    fn sidebar(&self, outcome: Outcome, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let sample = &self.sample;
        let history = &self.history;
        let running = outcome == Outcome::Running;
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_3()
            .gap_3()
            .scrollable(ScrollbarAxis::Vertical)
            .child(chart_card(
                t!("Build.Dashboard.BuildCpu").to_string(),
                if running {
                    format!("{:.0}%", sample.build_cpu)
                } else {
                    "—".to_owned()
                },
                &history.build_cpu,
                theme.info,
                cx,
            ))
            .child(chart_card(
                t!("Build.Dashboard.BuildMemory").to_string(),
                if running {
                    format_bytes(sample.build_memory)
                } else {
                    t!(
                        "Build.Dashboard.Peak",
                        value = format_bytes(history.peak_build_memory)
                    )
                    .to_string()
                },
                &history.build_memory,
                theme.success,
                cx,
            ))
            .child(chart_card(
                t!("Build.Dashboard.SystemCpu").to_string(),
                format!("{:.0}%", sample.system_cpu),
                &history.system_cpu,
                theme.warning,
                cx,
            ))
            .child(chart_card(
                t!("Build.Dashboard.SystemMemory").to_string(),
                format!(
                    "{} / {}",
                    format_bytes(sample.system_memory_used),
                    format_bytes(sample.system_memory_total)
                ),
                &history.system_memory,
                theme.danger,
                cx,
            ))
            .child(processes(&sample.processes, running, history.peak_processes, cx))
    }
}

fn header(build: &BuildSession, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let outcome = build.outcome();
    let (label, color) = match outcome {
        Outcome::Running => (t!("Build.Dashboard.Running"), theme.primary),
        Outcome::Succeeded => (t!("Build.Dashboard.Succeeded"), theme.success),
        Outcome::Failed => (t!("Build.Dashboard.Failed"), theme.danger),
        Outcome::Cancelled => (t!("Build.Dashboard.Cancelled"), theme.warning),
    };
    let pct = build.pct.load(std::sync::atomic::Ordering::Relaxed);
    let step = build.step.lock().clone();
    let detail = build.detail.lock().clone();
    v_flex()
        .w_full()
        .px_4()
        .py_3()
        .gap_2()
        .border_b_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .rounded(px(4.0))
                        .px_2()
                        .py_1()
                        .text_xs()
                        .font_semibold()
                        .text_color(color)
                        .bg(color.opacity(0.12))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(build.name.clone()),
                )
                .when(outcome == Outcome::Running, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(step.clone()),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format_duration(build.elapsed())),
                )
                .when(outcome == Outcome::Running, |this| {
                    this.child(
                        Button::new("build-dashboard-cancel")
                            .icon(IconName::Square)
                            .label(t!("Build.Button.Cancel").to_string())
                            .danger()
                            .on_click(|_, _, _| crate::runner::cancel_build()),
                    )
                }),
        )
        .when(outcome == Outcome::Running, |this| {
            this.child(Progress::new().value(pct as f32)).child(
                h_flex()
                    .w_full()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().truncate().child(detail))
                    .child(format!("{pct}%")),
            )
        })
}

fn chart_card(
    label: String,
    value: String,
    series: &VecDeque<Point>,
    color: Hsla,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let data: Vec<Point> = series.iter().copied().collect();
    v_flex()
        .w_full()
        .p_3()
        .gap_2()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .rounded(px(6.0))
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(div().text_sm().font_bold().text_color(color).child(value)),
        )
        .when(!data.is_empty(), |this| {
            this.child(
                div().h(px(48.0)).w_full().child(
                    AreaChart::<_, SharedString, f64>::new(data)
                        .x(|_| "".into())
                        .y(|point: &Point| point.0)
                        .stroke(color)
                        .fill(color.opacity(0.15))
                        .linear()
                        .tick_margin(0)
                        .max_points(HISTORY),
                ),
            )
        })
}

fn processes(rows: &[ProcessRow], running: bool, peak: usize, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let heading = if running {
        t!("Build.Dashboard.Processes", count = rows.len()).to_string()
    } else {
        t!("Build.Dashboard.ProcessesPeak", count = peak).to_string()
    };
    v_flex()
        .w_full()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(heading),
        )
        .when(rows.is_empty(), |this| {
            this.child(
                div()
                    .py_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if running {
                        t!("Build.Dashboard.NoProcesses").to_string()
                    } else {
                        t!("Build.Dashboard.NotRunning").to_string()
                    }),
            )
        })
        .children(rows.iter().map(|row| process_row(row, cx)))
}

fn process_row(row: &ProcessRow, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (kind, color) = match row.kind {
        ProcessKind::Rustc => (t!("Build.Dashboard.Kind.Rustc"), theme.info),
        ProcessKind::BuildScript => (t!("Build.Dashboard.Kind.BuildScript"), theme.warning),
        ProcessKind::Linker => (t!("Build.Dashboard.Kind.Linker"), theme.success),
        ProcessKind::Cargo => (t!("Build.Dashboard.Kind.Cargo"), theme.primary),
        ProcessKind::Other => (t!("Build.Dashboard.Kind.Other"), theme.muted_foreground),
    };
    h_flex()
        .id(ElementId::Integer(u64::from(row.pid)))
        .w_full()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(px(4.0))
        .bg(theme.background)
        .text_xs()
        .child(
            div()
                .w(px(78.0))
                .flex_none()
                .text_color(color)
                .child(kind.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.foreground)
                .child(row.label.clone()),
        )
        .child(
            div()
                .w(px(48.0))
                .flex_none()
                .text_right()
                .text_color(theme.muted_foreground)
                .child(format!("{:.0}%", row.cpu)),
        )
        .child(
            div()
                .w(px(64.0))
                .flex_none()
                .text_right()
                .text_color(theme.muted_foreground)
                .child(format_bytes(row.memory)),
        )
        .child(
            div()
                .w(px(44.0))
                .flex_none()
                .text_right()
                .text_color(theme.muted_foreground)
                .child(format_duration(Duration::from_secs(row.run_time))),
        )
}

fn empty_state(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .text_color(theme.muted_foreground)
        .child(div().text_sm().child(t!("Build.Dashboard.NoBuild").to_string()))
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn format_bytes(bytes: u64) -> String {
    let mib = mib(bytes);
    if mib >= 1024.0 {
        format!("{:.1} GB", mib / 1024.0)
    } else {
        format!("{mib:.0} MB")
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for BuildDashboard {
    type Params = ();

    fn window_name() -> &'static str {
        WINDOW_NAME
    }

    fn window_options(_: &()) -> WindowOptions {
        window_manager::default_window_options(1400.0, 860.0)
    }

    fn build(_: (), _window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(BuildDashboard::new)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in `gpui::test`, which would replace `#[test]`.
    use super::{
        format_bytes, format_duration, History, ProcessKind, ProcessRow, Sample, HISTORY,
    };
    use std::time::Duration;

    #[test]
    fn sizes_and_durations_read_naturally() {
        assert_eq!(format_bytes(512 * 1024 * 1024), "512 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 + 512 * 1024 * 1024), "3.5 GB");
        assert_eq!(format_duration(Duration::from_secs(75)), "1:15");
        assert_eq!(format_duration(Duration::from_secs(3_725)), "1:02:05");
    }

    #[test]
    fn history_keeps_the_last_samples_and_the_peaks() {
        let mut history = History::default();
        for i in 0..(HISTORY + 20) {
            let sample = Sample {
                build_memory: i as u64 * 1024 * 1024,
                processes: vec![
                    ProcessRow {
                        pid: 1,
                        kind: ProcessKind::Rustc,
                        label: "a".into(),
                        cpu: 0.0,
                        memory: 0,
                        run_time: 0,
                    };
                    i % 7
                ],
                ..Sample::default()
            };
            history.push(&sample, true);
        }
        assert_eq!(history.build_memory.len(), HISTORY);
        assert_eq!(history.peak_build_memory, (HISTORY as u64 + 19) * 1024 * 1024);
        assert_eq!(history.peak_processes, 6);
        // Between builds only the machine's series grow.
        history.push(&Sample::default(), false);
        assert_eq!(history.system_cpu.len(), HISTORY);
        assert_eq!(history.build_cpu.back().map(|p| p.0), Some(0.0));
    }
}
