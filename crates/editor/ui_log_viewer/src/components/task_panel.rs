//! The Tasks window: every editor task, live, with an audit view per task.
//!
//! The list shows what is queued, running, stalled, failed or done. Selecting
//! a task opens its detail: where it was created, which thread runs it, what it
//! is doing right now (polling, waiting, blocked), the watchdog's reason when
//! it stalls, and its own log in the same log viewer Mission Control uses,
//! fed from the task's log buffer (see `editor_task_queue::log`), so it can
//! never show another task's lines.
//!
//! This lives in the Mission Control crate because it embeds that crate's log
//! drawer. The editor shell opens it by window name (`EditorTasksWindow`).

use crate::components::log_drawer::LogDrawer;
use editor_task_queue::{
    StallKind, TaskActivity, TaskDuration, TaskId, TaskModel, TaskSnapshot, TaskStatus, ThreadRole,
    ThreadSnapshot, ThreadState,
};
use gpui::{prelude::FluentBuilder as _, *};
use std::{
    rc::Rc,
    time::{Duration, SystemTime},
};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    resizable::{h_resizable, resizable_panel, v_resizable, ResizableState},
    v_flex, v_virtual_list, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    TitleBar,
};

const ROW_HEIGHT: f32 = 72.0;
const DETAIL_WIDTH: f32 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskTab {
    All,
    Waiting,
    InProgress,
    Stalled,
    Failed,
    Succeeded,
}

impl TaskTab {
    const ALL: [Self; 6] = [
        Self::All,
        Self::Waiting,
        Self::InProgress,
        Self::Stalled,
        Self::Failed,
        Self::Succeeded,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Waiting => "Waiting",
            Self::InProgress => "In Progress",
            Self::Stalled => "Stalled",
            Self::Failed => "Failed",
            Self::Succeeded => "Succeeded",
        }
    }

    fn includes(self, task: &TaskSnapshot) -> bool {
        match self {
            Self::All => true,
            Self::Waiting => task.status == TaskStatus::Queued,
            Self::InProgress => task.status == TaskStatus::Running,
            Self::Stalled => task.stall.is_some(),
            Self::Failed => matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled),
            Self::Succeeded => task.status == TaskStatus::Succeeded,
        }
    }
}

/// What the window lists. Tasks, services and renderers are task records
/// (finite work, persistent loops, and the engine's render runtimes with what
/// each says about itself: owner, scene, size, mode, frame time); Threads is
/// the registry of every thread, grouped by role, which says what each one is
/// doing whether or not a task is behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Tasks,
    Threads,
    Services,
    Renderers,
}

impl View {
    const ALL: [Self; 4] = [Self::Tasks, Self::Threads, Self::Services, Self::Renderers];

    fn label(self) -> &'static str {
        match self {
            Self::Tasks => "Tasks",
            Self::Threads => "Threads",
            Self::Services => "Services",
            Self::Renderers => "Renderers",
        }
    }

    /// Whether `task` is listed in this view (Threads lists threads).
    fn lists(self, task: &TaskSnapshot) -> bool {
        let persistent = task.duration == TaskDuration::Persistent;
        let renderer = task.category == RUNTIME_CATEGORY;
        match self {
            Self::Tasks => !persistent,
            Self::Threads => false,
            Self::Services => persistent && !renderer,
            Self::Renderers => renderer,
        }
    }
}

use editor_task_queue::categories::RENDER_RUNTIME as RUNTIME_CATEGORY;

pub struct TaskQueuePanel {
    focus_handle: FocusHandle,
    search: Entity<InputState>,
    view: View,
    selected_tab: TaskTab,
    visible: Vec<TaskSnapshot>,
    /// The task open in the detail pane, with the drawer following its log.
    selected: Option<(TaskId, Entity<LogDrawer>)>,
    /// List | detail split, and the detail's facts | log split. Both are
    /// dragged by the user; they are recreated when the detail opens so the
    /// panel count always matches.
    split: Entity<ResizableState>,
    detail_split: Entity<ResizableState>,
    _refresh_task: Task<()>,
}

impl TaskQueuePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let task = cx.spawn(async move |this, cx| loop {
            smol::Timer::after(Duration::from_millis(250)).await;
            let _ = cx.update(|cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |_, cx| cx.notify());
                }
            });
        });

        Self {
            focus_handle: cx.focus_handle(),
            search: cx.new(|cx| InputState::new(window, cx).placeholder("Search tasks...")),
            // Saves and other short background jobs may finish before the
            // task panel is opened. Keep completed work visible by default so
            // the queue also serves as an audit history.
            view: View::Tasks,
            selected_tab: TaskTab::All,
            visible: Vec::new(),
            selected: None,
            split: ResizableState::new(cx),
            detail_split: ResizableState::new(cx),
            _refresh_task: task,
        }
    }

    fn select(&mut self, id: TaskId, cx: &mut Context<Self>) {
        if self.selected.as_ref().is_some_and(|(open, _)| *open == id) {
            return;
        }
        let Some(log) = editor_task_queue::global().log(id) else {
            return;
        };
        let drawer = cx.new(|cx| LogDrawer::for_task(log, cx));
        drawer.update(cx, |drawer, cx| drawer.start_monitoring(cx));
        if self.selected.is_none() {
            self.split = ResizableState::new(cx);
            self.detail_split = ResizableState::new(cx);
        }
        self.selected = Some((id, drawer));
        cx.notify();
    }
}

impl Focusable for TaskQueuePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TaskQueuePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all_tasks = editor_task_queue::global().snapshots();
        let threads = editor_task_queue::thread_snapshots();
        let view = self.view;
        let live = |view: View| {
            all_tasks
                .iter()
                .filter(|task| view.lists(task) && !task.status.is_finished())
                .count()
        };
        let view_counts = [
            all_tasks
                .iter()
                .filter(|task| View::Tasks.lists(task))
                .count(),
            threads
                .iter()
                .filter(|thread| thread.state != ThreadState::Exited)
                .count(),
            live(View::Services),
            live(View::Renderers),
        ];
        let stalled = |view: View| {
            all_tasks
                .iter()
                .filter(|task| view.lists(task) && task.stall.is_some())
                .count()
        };
        let view_stalled = [
            stalled(View::Tasks),
            0,
            stalled(View::Services),
            stalled(View::Renderers),
        ];
        // Tasks, Services and Renderers share the list; Threads has its own.
        let snapshots: Vec<TaskSnapshot> = all_tasks
            .iter()
            .filter(|task| view.lists(task))
            .cloned()
            .collect();
        let strip = strip_text(view, &threads);
        let query = self.search.read(cx).text().to_string().to_ascii_lowercase();
        let counts =
            TaskTab::ALL.map(|tab| snapshots.iter().filter(|task| tab.includes(task)).count());
        let worker_counts = editor_task_queue::global().worker_counts();
        let selected_snapshot = self
            .selected
            .as_ref()
            .and_then(|(id, _)| all_tasks.iter().find(|task| task.id == *id).cloned());
        let selected_id = self.selected.as_ref().map(|(id, _)| *id);
        self.visible = snapshots
            .into_iter()
            .filter(|task| self.selected_tab.includes(task))
            .filter(|task| matches_query(task, &query))
            .collect();
        self.visible.sort_by(|a, b| {
            b.starred
                .cmp(&a.starred)
                .then_with(|| b.submitted_at.cmp(&a.submitted_at))
        });

        let count = self.visible.len();
        let row_sizes = Rc::new(vec![size(px(0.0), px(ROW_HEIGHT)); count]);
        let list = if view == View::Threads {
            render_threads(&threads, &all_tasks, &query, cx)
        } else if count == 0 {
            v_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(if query.is_empty() {
                    format!(
                        "No {} {}",
                        self.selected_tab.label().to_ascii_lowercase(),
                        match view {
                            View::Services => "services",
                            View::Renderers => "renderers",
                            _ => "tasks",
                        }
                    )
                } else {
                    "No matching tasks".to_string()
                })
                .into_any_element()
        } else {
            v_virtual_list(
                cx.entity().clone(),
                "editor-task-queue-list",
                row_sizes,
                move |this, range: std::ops::Range<usize>, _window, cx: &mut Context<Self>| {
                    range
                        .map(|index| {
                            let Some(task) = this.visible.get(index).cloned() else {
                                return div().h(px(ROW_HEIGHT)).into_any_element();
                            };
                            let open = selected_id == Some(task.id);
                            render_row(task, open, cx)
                        })
                        .collect::<Vec<_>>()
                },
            )
            .flex_1()
            .min_w_0()
            .into_any_element()
        };

        let detail = match (&self.selected, selected_snapshot) {
            (Some((_, drawer)), Some(task)) => Some(
                render_detail(&task, drawer.clone(), self.detail_split.clone(), cx)
                    .into_any_element(),
            ),
            // The task was forgotten (history is bounded); show nothing.
            _ => None,
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(TitleBar::new().child("Editor Tasks"))
            .child(
                v_flex()
                    .w_full()
                    .gap_3()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .font_semibold()
                                    .text_lg()
                                    .child("Editor Tasks"),
                            )
                            .child(TextInput::new(&self.search).w(px(320.0))),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_4()
                            .child(worker_count_control(
                                "On-demand",
                                worker_counts.on_demand,
                                true,
                                cx,
                            ))
                            .child(worker_count_control(
                                "Dedicated",
                                worker_counts.dedicated,
                                false,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Worker counts apply to editor tasks only. Select a task for its log."),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .children(View::ALL.into_iter().enumerate().map(|(index, tab)| {
                                let selected = view == tab;
                                let stalled = view_stalled[index];
                                Button::new(("editor-task-view", index))
                                    .label(format!(
                                        "{} ({}){}",
                                        tab.label(),
                                        view_counts[index],
                                        if stalled > 0 {
                                            format!("  ⚠ {stalled} stalled")
                                        } else {
                                            String::new()
                                        }
                                    ))
                                    .when(selected, |button| button.primary())
                                    .when(!selected, |button| button.outline())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.view = tab;
                                        cx.notify();
                                    }))
                            })),
                    )
                    .when(view != View::Threads, |header| header.child(h_flex().w_full().gap_2().children(
                        TaskTab::ALL.into_iter().enumerate().map(|(index, tab)| {
                            let selected = self.selected_tab == tab;
                            Button::new(("editor-task-tab", index))
                                .label(format!("{} ({})", tab.label(), counts[index]))
                                .when(selected, |button| button.primary())
                                .when(!selected, |button| button.ghost())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected_tab = tab;
                                    cx.notify();
                                }))
                        }),
                    )))
                    .when_some(strip, |header, text| {
                        header.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(text),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().w_full().child(match detail {
                Some(detail) => h_resizable("editor-tasks-split")
                    .state(self.split.clone())
                    .child(resizable_panel().child(list))
                    .child(
                        resizable_panel()
                            .size(px(DETAIL_WIDTH))
                            .size_range(px(320.0)..px(2400.0))
                            .child(detail),
                    )
                    .into_any_element(),
                None => div().size_full().flex().child(list).into_any_element(),
            }))
    }
}

fn matches_query(task: &TaskSnapshot, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let contains = |text: Option<&str>| {
        text.unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
    };
    task.title.to_ascii_lowercase().contains(query)
        || task.category.to_ascii_lowercase().contains(query)
        || contains(task.detail.as_deref())
        || contains(task.error.as_deref())
        || contains(task.phase.as_deref())
        || task.origin.to_ascii_lowercase().contains(query)
}

fn status_color(status: TaskStatus, cx: &App) -> Hsla {
    match status {
        TaskStatus::Queued => cx.theme().muted_foreground,
        TaskStatus::Running => cx.theme().primary,
        TaskStatus::Succeeded => cx.theme().success,
        TaskStatus::Failed => cx.theme().danger,
        TaskStatus::Cancelled => cx.theme().warning,
    }
}

fn render_row(task: TaskSnapshot, open: bool, cx: &mut Context<TaskQueuePanel>) -> AnyElement {
    let id = task.id;
    let starred = task.starred;
    let active = task.status == TaskStatus::Running;
    let status_color = status_color(task.status, cx);
    let progress_label = task
        .stall
        .as_ref()
        .map(|stall| stall.message.clone())
        .or_else(|| {
            (!task.details.is_empty()).then(|| {
                task.details
                    .iter()
                    .map(|(_, value)| value.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
        })
        .or_else(|| (task.duration == TaskDuration::Persistent).then(|| service_summary(&task)))
        .or_else(|| task.detail.clone())
        .or_else(|| task.phase.clone())
        .or_else(|| task.error.clone())
        .unwrap_or_else(|| duration_label(&task).to_string());
    h_flex()
        .id(("editor-task-row", id.get()))
        .w_full()
        .h(px(ROW_HEIGHT))
        .gap_3()
        .items_center()
        .px_4()
        .border_b_1()
        .border_color(cx.theme().border.opacity(0.5))
        .cursor_pointer()
        .when(open, |row| row.bg(cx.theme().primary.opacity(0.08)))
        .hover(|row| row.bg(cx.theme().muted.opacity(0.12)))
        .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
        .child(
            Button::new(("editor-task-star", id.get()))
                .ghost()
                .xsmall()
                .icon(
                    Icon::new(IconName::Star)
                        .size(px(14.0))
                        .text_color(if starred {
                            cx.theme().warning
                        } else {
                            cx.theme().muted_foreground
                        }),
                )
                .tooltip(if starred {
                    "Remove priority"
                } else {
                    "Prioritize task"
                })
                .on_click(cx.listener(move |_, _, _, cx| {
                    let queue = editor_task_queue::global();
                    let current = queue
                        .snapshots()
                        .into_iter()
                        .find(|item| item.id == id)
                        .is_some_and(|item| item.starred);
                    queue.set_starred(id, !current);
                    cx.notify();
                })),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().min_w_0().text_sm().child(task.title.clone()))
                        .when(task.stall.is_some(), |header| {
                            header.child(chip("Stalled", cx.theme().warning, cx))
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(status_color)
                                .child(status_label(task.status)),
                        )
                        .when(active, |header| {
                            header.child(chip(pool_label(&task), cx.theme().primary, cx))
                        }),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .overflow_hidden()
                                .text_color(if task.stall.is_some() {
                                    cx.theme().warning
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(format!("{} · {}", task.category, progress_label)),
                        )
                        .when_some(task.progress.filter(|_| active), |row, progress| {
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{:>3.0}%", progress * 100.0)),
                            )
                        }),
                )
                .when_some(task.progress.filter(|_| active), |column, progress| {
                    column.child(
                        div()
                            .w_full()
                            .h(px(3.0))
                            .rounded(px(2.0))
                            .bg(cx.theme().muted.opacity(0.25))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(progress))
                                    .rounded(px(2.0))
                                    .bg(cx.theme().primary),
                            ),
                    )
                }),
        )
        .into_any_element()
}

fn chip(label: &'static str, color: Hsla, _cx: &App) -> Div {
    div()
        .rounded(px(4.0))
        .px_2()
        .py_1()
        .text_xs()
        .text_color(color)
        .bg(color.opacity(0.12))
        .child(label)
}

fn render_detail(
    task: &TaskSnapshot,
    drawer: Entity<LogDrawer>,
    detail_split: Entity<ResizableState>,
    cx: &mut Context<TaskQueuePanel>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let row = |label: &'static str, value: String| {
        h_flex()
            .w_full()
            .gap_3()
            .py_1()
            .border_b_1()
            .border_color(theme.border.opacity(0.25))
            .child(
                div()
                    .w(px(110.0))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().text_xs().child(value))
    };

    // What the task says about itself (a render runtime's owner, size, mode).
    let detail_rows: Vec<_> = task
        .details
        .iter()
        .map(|(label, value)| {
            h_flex()
                .w_full()
                .gap_3()
                .py_1()
                .border_b_1()
                .border_color(theme.border.opacity(0.25))
                .child(
                    div()
                        .w(px(110.0))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(label.clone()),
                )
                .child(div().flex_1().min_w_0().text_xs().child(value.clone()))
        })
        .collect();

    let activity = match task.activity {
        TaskActivity::NotStarted => "Not started".to_string(),
        TaskActivity::Polling => match task.model {
            TaskModel::Blocking => format!("Running (blocking) for {}", fmt(task.activity_for)),
            _ => format!("Running code for {}", fmt(task.activity_for)),
        },
        TaskActivity::Pending => format!("Waiting on an await for {}", fmt(task.activity_for)),
        TaskActivity::Finished => "Finished".to_string(),
    };
    let heartbeat = if task.beats == 0 {
        "none yet".to_string()
    } else {
        format!(
            "{} beats{}{}{}",
            task.beats,
            task.beat_rate_hz
                .map(|hz| format!(", {hz:.1} Hz"))
                .unwrap_or_default(),
            task.since_beat
                .map(|since| format!(", last {} ago", fmt(since)))
                .unwrap_or_default(),
            if task.idle { " (idle on purpose)" } else { "" }
        )
    };
    let started = task
        .started_at
        .map(|at| format!("{} ago", fmt(since(at))))
        .unwrap_or_else(|| "not yet".into());
    let total = match (task.started_at, task.finished_at) {
        (Some(start), Some(end)) => end.duration_since(start).ok().map(fmt),
        (Some(start), None) => Some(fmt(since(start))),
        _ => None,
    }
    .unwrap_or_else(|| "-".into());

    v_flex()
        .size_full()
        .border_l_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .w_full()
                .px_4()
                .py_2()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(theme.border.opacity(0.5))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_semibold()
                        .text_sm()
                        .child(task.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(status_color(task.status, cx))
                        .child(status_label(task.status)),
                )
                .when(!task.status.is_finished(), |header| {
                    let id = task.id;
                    header.child(
                        Button::new("task-detail-cancel")
                            .ghost()
                            .xsmall()
                            .label("Cancel")
                            .tooltip("Request cancellation; the task must observe it")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                editor_task_queue::global().cancel(id);
                                cx.notify();
                            })),
                    )
                })
                .child(
                    Button::new("task-detail-close")
                        .ghost()
                        .xsmall()
                        .label("Close")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected = None;
                            cx.notify();
                        })),
                ),
        )
        .when_some(task.stall.clone(), |column, stall| {
            let heading = match stall.kind {
                StallKind::QueueWait => "Waiting for a worker",
                StallKind::NeverPolled => "Never polled",
                StallKind::BlockedPoll => "Blocking its worker",
                StallKind::BlockedThread => "Blocking its thread",
                StallKind::Pending => "Waiting with no progress",
                StallKind::NoProgress => "No progress",
                StallKind::NoHeartbeat => "Heartbeat stopped",
            };
            column.child(
                v_flex()
                    .mx_4()
                    .mt_3()
                    .p_3()
                    .gap_1()
                    .rounded(px(6.0))
                    .bg(theme.warning.opacity(0.12))
                    .border_1()
                    .border_color(theme.warning.opacity(0.4))
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(theme.warning)
                            .child(heading),
                    )
                    .child(div().text_xs().child(stall.message)),
            )
        })
        .child(
            div().flex_1().min_h_0().w_full().child(
                v_resizable("editor-task-detail-split")
                    .state(detail_split)
                    .child(
                        resizable_panel()
                            .size(px(260.0))
                            .size_range(px(60.0)..px(1200.0))
                            .child(
                                div()
                                    .id("editor-task-facts")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .child(
                                        v_flex()
                                            .w_full()
                                            .px_4()
                                            .py_2()
                                            .child(row("Category", task.category.clone()))
                                            .children(detail_rows)
                                            .child(row("Created at", task.origin.clone()))
                                            .child(row(
                                                "Runs on",
                                                format!(
                                                    "{} ({})",
                                                    task.worker
                                                        .clone()
                                                        .unwrap_or_else(|| "not assigned".into()),
                                                    model_label(task.model)
                                                ),
                                            ))
                                            .child(row("Started", started))
                                            .child(row("Duration", total))
                                            .child(row(
                                                "Phase",
                                                task.phase.clone().unwrap_or_else(|| "-".into()),
                                            ))
                                            .child(row("Right now", activity))
                                            .child(row(
                                                "Last activity",
                                                format!(
                                                    "{} ago (log, progress or phase)",
                                                    fmt(task.idle_for)
                                                ),
                                            ))
                                            .child(row("Polls", task.polls.to_string()))
                                            .when(
                                                task.duration == TaskDuration::Persistent,
                                                |column| {
                                                    column.child(row("Heartbeat", heartbeat)).child(
                                                        row(
                                                            "Last pass",
                                                            task.last_tick
                                                                .map_or_else(|| "-".into(), fmt),
                                                        ),
                                                    )
                                                },
                                            )
                                            .when_some(task.detail.clone(), |column, detail| {
                                                column.child(row("Detail", detail))
                                            })
                                            .when_some(task.error.clone(), |column, error| {
                                                column.child(row("Error", error))
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        resizable_panel().child(
                            v_flex()
                                .size_full()
                                .child(
                                    div()
                                        .px_4()
                                        .pt_2()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("Task log ({} lines)", task.log_lines)),
                                )
                                .child(div().flex_1().min_h_0().w_full().child(drawer)),
                        ),
                    ),
            ),
        )
}

fn worker_count_control(
    label: &'static str,
    value: usize,
    on_demand: bool,
    cx: &mut Context<TaskQueuePanel>,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .mr_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{label} workers")),
        )
        .child(
            Button::new(if on_demand {
                "task-worker-minus-on-demand"
            } else {
                "task-worker-minus-dedicated"
            })
            .ghost()
            .xsmall()
            .label("−")
            .tooltip(format!("Reduce {label} workers"))
            .on_click(cx.listener(move |_, _, _, cx| {
                let queue = editor_task_queue::global();
                let current = queue.worker_counts();
                if on_demand {
                    queue.set_worker_counts(current.on_demand.saturating_sub(1), current.dedicated);
                } else {
                    queue.set_worker_counts(current.on_demand, current.dedicated.saturating_sub(1));
                }
                cx.notify();
            })),
        )
        .child(
            div()
                .min_w(px(20.0))
                .text_center()
                .text_sm()
                .child(value.to_string()),
        )
        .child(
            Button::new(if on_demand {
                "task-worker-plus-on-demand"
            } else {
                "task-worker-plus-dedicated"
            })
            .ghost()
            .xsmall()
            .label("+")
            .tooltip(format!("Increase {label} workers"))
            .on_click(cx.listener(move |_, _, _, cx| {
                let queue = editor_task_queue::global();
                let current = queue.worker_counts();
                if on_demand {
                    queue.set_worker_counts(current.on_demand + 1, current.dedicated);
                } else {
                    queue.set_worker_counts(current.on_demand, current.dedicated + 1);
                }
                cx.notify();
            })),
        )
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for TaskQueuePanel {
    type Params = ();

    fn window_name() -> &'static str {
        "EditorTasksWindow"
    }

    fn window_options(_: &()) -> WindowOptions {
        window_manager::default_window_options(1480.0, 760.0)
    }

    fn build(_: (), window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| TaskQueuePanel::new(window, cx))
    }
}

fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "Waiting",
        TaskStatus::Running => "In Progress",
        TaskStatus::Succeeded => "Succeeded",
        TaskStatus::Failed => "Failed",
        TaskStatus::Cancelled => "Cancelled",
    }
}

fn pool_label(task: &TaskSnapshot) -> &'static str {
    match task.duration {
        TaskDuration::Short => "On-demand worker",
        TaskDuration::Long => "Dedicated worker",
        TaskDuration::Foreground => "Inline",
        TaskDuration::Persistent => "Service thread",
    }
}

fn duration_label(task: &TaskSnapshot) -> &'static str {
    match task.duration {
        TaskDuration::Short => "Short task",
        TaskDuration::Long => "Long task",
        TaskDuration::Foreground => "Inline task",
        TaskDuration::Persistent => "Persistent service",
    }
}

fn model_label(model: TaskModel) -> &'static str {
    match model {
        TaskModel::Async => "async",
        TaskModel::Blocking => "blocking",
        TaskModel::Foreground => "inline on the calling thread",
        TaskModel::Persistent => "persistent (a loop, or driven by one)",
    }
}

fn since(at: SystemTime) -> Duration {
    at.elapsed().unwrap_or_default()
}

fn fmt(duration: Duration) -> String {
    let millis = duration.as_millis();
    if duration.as_micros() < 1_000 {
        format!("{}µs", duration.as_micros())
    } else if millis < 1_000 {
        format!("{millis}ms")
    } else if millis < 60_000 {
        format!("{:.1}s", duration.as_secs_f32())
    } else {
        format!(
            "{}m {:02}s",
            duration.as_secs() / 60,
            duration.as_secs() % 60
        )
    }
}

/// One line under the view tabs: micro tasks on Tasks, the pools on Threads.
fn strip_text(view: View, threads: &[ThreadSnapshot]) -> Option<String> {
    match view {
        View::Tasks => {
            let micro = editor_task_queue::micro_snapshots();
            if micro.is_empty() {
                return None;
            }
            let top = micro
                .iter()
                .take(4)
                .map(|stat| {
                    format!(
                        "{} x{} (mean {}, max {}{})",
                        stat.name,
                        stat.count,
                        fmt(stat.mean()),
                        fmt(stat.max),
                        if stat.slow > 0 {
                            format!(", {} slow", stat.slow)
                        } else {
                            String::new()
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("  |  ");
            Some(format!(
                "Micro tasks (counted, not listed): {top}{}",
                if micro.len() > 4 {
                    format!("  |  +{} more", micro.len() - 4)
                } else {
                    String::new()
                }
            ))
        }
        View::Threads => {
            let tokio = editor_task_queue::pools::tokio_metrics().map_or_else(
                || "tokio: not started".to_string(),
                |m| {
                    format!(
                        "tokio: {} workers, {} live tasks, {} queued",
                        m.workers, m.alive_tasks, m.queue_depth
                    )
                },
            );
            let rayon = threads
                .iter()
                .filter(|thread| thread.pool.as_deref() == Some("rayon"))
                .count();
            Some(format!(
                "{tokio}  |  rayon: {rayon} workers (state not reported by rayon)  |  Threads this process created outside the task system do not appear here."
            ))
        }
        View::Services => None,
        View::Renderers => Some(
            "The engine's render runtimes: one scheduler draws them all (the \"Helio runtimes\" service). Each lists its owner, scene, size, update mode, state and CPU frame time; its log is what its renderer logged while it drew."
                .to_string(),
        ),
    }
}

/// `Hz | pass time | phase` for a persistent task's row.
fn service_summary(task: &TaskSnapshot) -> String {
    let mut parts = Vec::new();
    match (task.beat_rate_hz, task.idle) {
        (_, true) => parts.push("idle".to_string()),
        (Some(hz), _) => parts.push(format!("{hz:.0} Hz")),
        _ => {}
    }
    if let Some(tick) = task.last_tick {
        parts.push(format!("pass {}", fmt(tick)));
    }
    if let Some(phase) = &task.phase {
        parts.push(phase.clone());
    }
    if parts.is_empty() {
        "starting".to_string()
    } else {
        parts.join(" · ")
    }
}

fn thread_matches(thread: &ThreadSnapshot, title: Option<&str>, query: &str) -> bool {
    query.is_empty()
        || thread.name.to_ascii_lowercase().contains(query)
        || thread.owner.to_ascii_lowercase().contains(query)
        || thread
            .pool
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
        || thread
            .phase
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
        || title
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
}

/// The Threads view: every registered thread, grouped by role.
fn render_threads(
    threads: &[ThreadSnapshot],
    tasks: &[TaskSnapshot],
    query: &str,
    cx: &mut Context<TaskQueuePanel>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let title_of = |id: TaskId| {
        tasks
            .iter()
            .find(|task| task.id == id)
            .map(|task| task.title.clone())
    };
    let cell = |width: f32, text: String, color: Hsla| {
        div()
            .w(px(width))
            .flex_shrink_0()
            .truncate()
            .text_color(color)
            .child(text)
    };
    let mut body = v_flex().w_full().child(
        h_flex()
            .w_full()
            .px_4()
            .py_1()
            .gap_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .child(div().w(px(8.0)).flex_shrink_0())
            .child(cell(230.0, "Thread".into(), theme.muted_foreground))
            .child(cell(150.0, "Owner / pool".into(), theme.muted_foreground))
            .child(cell(130.0, "State".into(), theme.muted_foreground))
            .child(cell(110.0, "Busy".into(), theme.muted_foreground))
            .child(div().flex_1().min_w_0().child("Doing")),
    );
    let mut shown = 0;
    for role in ThreadRole::ALL {
        let group: Vec<(&ThreadSnapshot, Option<String>)> = threads
            .iter()
            .map(|thread| (thread, thread.task.and_then(title_of)))
            .filter(|(thread, title)| {
                thread.role == role && thread_matches(thread, title.as_deref(), query)
            })
            .collect();
        if group.is_empty() {
            continue;
        }
        let running = group
            .iter()
            .filter(|(thread, _)| thread.state == ThreadState::Running)
            .count();
        body = body.child(
            h_flex()
                .w_full()
                .px_4()
                .py_1()
                .bg(theme.muted.opacity(0.1))
                .text_xs()
                .font_semibold()
                .child(format!(
                    "{}  ·  {} thread(s), {} running",
                    role.label(),
                    group.len(),
                    running
                )),
        );
        for (thread, title) in group {
            shown += 1;
            body = body.child(render_thread_row(thread, title, cx));
        }
    }
    if shown == 0 {
        body = body.child(
            div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No matching threads"),
        );
    }
    v_flex()
        .id("editor-thread-list")
        .flex_1()
        .min_w_0()
        .overflow_y_scroll()
        .child(body)
        .into_any_element()
}

fn render_thread_row(
    thread: &ThreadSnapshot,
    title: Option<String>,
    cx: &mut Context<TaskQueuePanel>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let (color, state_text) = match thread.state {
        ThreadState::Running => (theme.success, format!("Running {}", fmt(thread.state_for))),
        ThreadState::Parked => (
            theme.muted_foreground,
            format!("Waiting {}", fmt(thread.state_for)),
        ),
        ThreadState::Unknown => (theme.muted_foreground.opacity(0.6), "Not reported".into()),
        ThreadState::Exited => (theme.warning, "Exited".into()),
    };
    let busy = thread.busy.map_or_else(
        || "-".to_string(),
        |busy| {
            let alive = thread.alive_for.as_secs_f64().max(0.001);
            format!("{} ({:.0}%)", fmt(busy), busy.as_secs_f64() / alive * 100.0)
        },
    );
    let doing = title
        .map(|title| format!("▶ {title}"))
        .or_else(|| thread.phase.clone())
        .unwrap_or_default();
    let owner = match (&thread.pool, thread.owner.is_empty()) {
        (Some(pool), true) => pool.clone(),
        (Some(pool), false) => format!("{} / {pool}", thread.owner),
        (None, _) => thread.owner.clone(),
    };
    let cell = |width: f32, text: String, color: Hsla| {
        div()
            .w(px(width))
            .flex_shrink_0()
            .truncate()
            .text_color(color)
            .child(text)
    };
    let task = thread.task;
    h_flex()
        .id(("editor-thread-row", thread.id))
        .w_full()
        .h(px(28.0))
        .px_4()
        .gap_3()
        .items_center()
        .text_xs()
        .border_b_1()
        .border_color(theme.border.opacity(0.3))
        .when(thread.state == ThreadState::Exited, |row| row.opacity(0.6))
        .when_some(task, |row, id| {
            row.cursor_pointer()
                .hover(|row| row.bg(theme.muted.opacity(0.12)))
                .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
        })
        .child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .flex_shrink_0()
                .rounded_full()
                .bg(color),
        )
        .child(cell(230.0, thread.name.clone(), theme.foreground))
        .child(cell(150.0, owner, theme.muted_foreground))
        .child(cell(130.0, state_text, color))
        .child(cell(110.0, busy, theme.foreground))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(doing),
        )
        .into_any_element()
}
