use editor_task_queue::{TaskSnapshot, TaskStatus};
use gpui::{prelude::FluentBuilder as _, *};
use std::{rc::Rc, time::Duration};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    v_flex, v_virtual_list, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    TitleBar,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskTab {
    Waiting,
    InProgress,
    Failed,
    Succeeded,
}

impl TaskTab {
    const ALL: [Self; 4] = [
        Self::Waiting,
        Self::InProgress,
        Self::Failed,
        Self::Succeeded,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::InProgress => "In Progress",
            Self::Failed => "Failed",
            Self::Succeeded => "Succeeded",
        }
    }

    fn includes(self, status: TaskStatus) -> bool {
        match self {
            Self::Waiting => status == TaskStatus::Queued,
            Self::InProgress => status == TaskStatus::Running,
            Self::Failed => matches!(status, TaskStatus::Failed | TaskStatus::Cancelled),
            Self::Succeeded => status == TaskStatus::Succeeded,
        }
    }
}

pub struct TaskQueuePanel {
    focus_handle: FocusHandle,
    search: Entity<InputState>,
    selected_tab: TaskTab,
    visible: Vec<TaskSnapshot>,
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
            selected_tab: TaskTab::Waiting,
            visible: Vec::new(),
            _refresh_task: task,
        }
    }
}

impl Focusable for TaskQueuePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TaskQueuePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshots = editor_task_queue::global().snapshots();
        let query = self.search.read(cx).text().to_string().to_ascii_lowercase();
        let counts = TaskTab::ALL.map(|tab| {
            snapshots
                .iter()
                .filter(|task| tab.includes(task.status))
                .count()
        });
        self.visible = snapshots
            .into_iter()
            .filter(|task| self.selected_tab.includes(task.status))
            .filter(|task| {
                query.is_empty()
                    || task.title.to_ascii_lowercase().contains(&query)
                    || task.category.to_ascii_lowercase().contains(&query)
                    || task
                        .detail
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&query)
                    || task
                        .error
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .collect();
        self.visible.sort_by(|a, b| {
            b.starred
                .cmp(&a.starred)
                .then_with(|| b.submitted_at.cmp(&a.submitted_at))
        });

        let count = self.visible.len();
        let row_sizes = Rc::new(vec![size(px(0.0), px(72.0)); count]);
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
                    .child(h_flex().w_full().gap_2().children(
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
                    )),
            )
            .child(if count == 0 {
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if query.is_empty() {
                        format!(
                            "No {} tasks",
                            self.selected_tab.label().to_ascii_lowercase()
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
                                    return div().h(px(72.0)).into_any_element();
                                };
                                let id = task.id;
                                let starred = task.starred;
                                let active = task.status == TaskStatus::Running;
                                let status_color = match task.status {
                                    TaskStatus::Queued => cx.theme().muted_foreground,
                                    TaskStatus::Running => cx.theme().primary,
                                    TaskStatus::Succeeded => cx.theme().success,
                                    TaskStatus::Failed => cx.theme().danger,
                                    TaskStatus::Cancelled => cx.theme().warning,
                                };
                                let progress_label = task
                                    .detail
                                    .clone()
                                    .or_else(|| task.error.clone())
                                    .unwrap_or_else(|| task.duration_label().to_string());
                                h_flex()
                                    .w_full()
                                    .h(px(72.0))
                                    .gap_3()
                                    .items_center()
                                    .px_4()
                                    .border_b_1()
                                    .border_color(cx.theme().border.opacity(0.5))
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
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .text_sm()
                                                            .child(task.title.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(status_color)
                                                            .child(status_label(task.status)),
                                                    ),
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
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(format!(
                                                                "{} · {}",
                                                                task.category, progress_label
                                                            )),
                                                    )
                                                    .when_some(
                                                        task.progress.filter(|_| active),
                                                        |row, progress| {
                                                            row.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(
                                                                        cx.theme().muted_foreground,
                                                                    )
                                                                    .child(format!(
                                                                        "{:>3.0}%",
                                                                        progress * 100.0
                                                                    )),
                                                            )
                                                        },
                                                    ),
                                            )
                                            .when_some(
                                                task.progress.filter(|_| active),
                                                |column, progress| {
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
                                                },
                                            ),
                                    )
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .flex_1()
                .into_any_element()
            })
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for TaskQueuePanel {
    type Params = ();

    fn window_name() -> &'static str {
        "EditorTasksWindow"
    }

    fn window_options(_: &()) -> WindowOptions {
        window_manager::default_window_options(980.0, 680.0)
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

trait TaskSnapshotExt {
    fn duration_label(&self) -> &'static str;
}

impl TaskSnapshotExt for TaskSnapshot {
    fn duration_label(&self) -> &'static str {
        match self.duration {
            editor_task_queue::TaskDuration::Short => "Short task",
            editor_task_queue::TaskDuration::Long => "Long task",
        }
    }
}
