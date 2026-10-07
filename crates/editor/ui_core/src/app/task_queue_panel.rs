use gpui::{prelude::FluentBuilder as _, *};
use editor_task_queue::{TaskSnapshot, TaskStatus};
use std::rc::Rc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    v_flex, v_virtual_list, ActiveTheme as _, Icon, IconName, StyledExt,
};

pub struct TaskQueuePanel {
    search: Entity<InputState>,
    visible: Vec<TaskSnapshot>,
}

impl TaskQueuePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            search: cx.new(|cx| InputState::new(window, cx).placeholder("Search tasks...")),
            visible: Vec::new(),
        }
    }
}

impl Render for TaskQueuePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).text().to_string().to_ascii_lowercase();
        self.visible = editor_task_queue::global()
            .snapshots()
            .into_iter()
            .filter(|task| {
                query.is_empty()
                    || task.title.to_ascii_lowercase().contains(&query)
                    || task.category.to_ascii_lowercase().contains(&query)
                    || task.detail.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&query)
                    || task.error.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&query)
            })
            .collect();
        self.visible.sort_by(|a, b| {
            b.starred
                .cmp(&a.starred)
                .then_with(|| b.submitted_at.cmp(&a.submitted_at))
        });

        let count = self.visible.len();
        let row_sizes = Rc::new(vec![size(px(0.0), px(66.0)); count]);
        v_flex()
            .w(px(520.0))
            .h(px(420.0))
            .overflow_hidden()
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .shadow_lg()
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .font_semibold()
                                    .text_sm()
                                    .child(format!("Editor Tasks ({count})")),
                            ),
                    )
                    .child(TextInput::new(&self.search).w_full()),
            )
            .child(if count == 0 {
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if query.is_empty() { "No editor tasks" } else { "No matching tasks" })
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
                                    return div().h(px(66.0)).into_any_element();
                                };
                                let id = task.id;
                                let starred = task.starred;
                                let active = matches!(task.status, TaskStatus::Queued | TaskStatus::Running);
                                let status_color = match task.status {
                                    TaskStatus::Queued => cx.theme().muted_foreground,
                                    TaskStatus::Running => cx.theme().primary,
                                    TaskStatus::Succeeded => cx.theme().success,
                                    TaskStatus::Failed => cx.theme().danger,
                                    TaskStatus::Cancelled => cx.theme().muted_foreground,
                                };
                                let progress_label = task
                                    .detail
                                    .clone()
                                    .or_else(|| task.error.clone())
                                    .unwrap_or_else(|| task.duration_label().to_string());
                                h_flex()
                                    .w_full()
                                    .h(px(66.0))
                                    .gap_2()
                                    .items_center()
                                    .px_3()
                                    .border_b_1()
                                    .border_color(cx.theme().border.opacity(0.5))
                                    .child(
                                        Button::new(("editor-task-star", id.get()))
                                            .ghost()
                                            .xsmall()
                                            .icon(
                                                Icon::new(IconName::Star)
                                                    .size(px(14.0))
                                                    .text_color(if starred { cx.theme().warning } else { cx.theme().muted_foreground }),
                                            )
                                            .tooltip(if starred { "Remove priority" } else { "Prioritize task" })
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                let queue = editor_task_queue::global();
                                                let current = queue.snapshots().into_iter().find(|item| item.id == id).is_some_and(|item| item.starred);
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
                                                    .child(div().text_xs().text_color(status_color).child(status_label(task.status))),
                                            )
                                            .child(
                                                h_flex()
                                                    .w_full()
                                                    .gap_2()
                                                    .items_center()
                                                    .child(div().flex_1().min_w_0().text_xs().text_color(cx.theme().muted_foreground).child(format!("{} · {}", task.category, progress_label)))
                                                    .when_some(task.progress.filter(|progress| active), |row, progress| {
                                                        row.child(div().text_xs().text_color(cx.theme().muted_foreground).child(format!("{:>3.0}%", progress * 100.0)))
                                                    }),
                                            )
                                            .when_some(task.progress.filter(|progress| active), |column, progress| {
                                                column.child(
                                                    div()
                                                        .w_full()
                                                        .h(px(3.0))
                                                        .rounded(px(2.0))
                                                        .bg(cx.theme().muted.opacity(0.25))
                                                        .child(div().h_full().w(relative(progress)).rounded(px(2.0)).bg(cx.theme().primary)),
                                                )
                                            }),
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

fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "Queued",
        TaskStatus::Running => "Running",
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
