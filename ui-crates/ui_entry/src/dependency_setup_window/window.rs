//! Main dependency setup window implementation.

use gpui::*;
use ui::{
    button::{Button, ButtonVariants},
    h_flex, v_flex, ActiveTheme, Icon, IconName,
};

use super::checks::check_rust;
use super::installer::run_setup_script;
use super::task::{SetupTask, TaskStatus};

pub struct DependencySetupWindow {
    setup_tasks: Vec<SetupTask>,
    progress: f32,
    is_running: bool,
    setup_complete: bool,
    setup_error: Option<String>,
}

pub struct SetupComplete;

impl EventEmitter<SetupComplete> for DependencySetupWindow {}

impl DependencySetupWindow {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            setup_tasks: vec![
                SetupTask::new("Checking Rust", "Looking for an existing toolchain"),
                SetupTask::new("Installing Rust", "Running rustup — handles toolchains, build tools, and SDKs for this platform"),
            ],
            progress: 0.0,
            is_running: false,
            setup_complete: false,
            setup_error: None,
        }
    }

    pub fn start_setup(&mut self, cx: &mut Context<Self>) {
        if self.is_running {
            return;
        }
        self.is_running = true;
        self.progress = 0.0;
        cx.notify();

        let view = cx.entity().downgrade();

        cx.spawn(async move |_this, mut cx| {
            // Step 1 — check
            cx.update(|cx| {
                if let Some(v) = view.upgrade() {
                    let _ = v.update(cx, |this, cx| {
                        this.update_task(0, TaskStatus::InProgress);
                        cx.notify();
                    });
                }
            });

            let rust_ok = check_rust();

            cx.update(|cx| {
                if let Some(v) = view.upgrade() {
                    let _ = v.update(cx, |this, cx| {
                        this.update_task(0, if rust_ok {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Failed("Not found".to_string())
                        });
                        this.progress = 0.5;
                        cx.notify();
                    });
                }
            });

            if rust_ok {
                cx.update(|cx| {
                    if let Some(v) = view.upgrade() {
                        let _ = v.update(cx, |this, cx| {
                            this.update_task(1, TaskStatus::Completed);
                            this.progress = 1.0;
                            this.setup_complete = true;
                            this.is_running = false;
                            cx.emit(SetupComplete);
                            cx.notify();
                        });
                    }
                });
                return;
            }

            // Step 2 — install (rustup handles platform detection internally)
            cx.update(|cx| {
                if let Some(v) = view.upgrade() {
                    let _ = v.update(cx, |this, cx| {
                        this.update_task(1, TaskStatus::InProgress);
                        cx.notify();
                    });
                }
            });

            let install_ok = cx
                .background_executor()
                .spawn(async { run_setup_script() })
                .await;

            cx.update(|cx| {
                if let Some(v) = view.upgrade() {
                    let _ = v.update(cx, |this, cx| {
                        this.update_task(1, if install_ok {
                            TaskStatus::Completed
                        } else {
                            TaskStatus::Failed("rustup install failed".to_string())
                        });
                        this.progress = 1.0;
                        this.setup_complete = install_ok;
                        this.is_running = false;
                        if install_ok {
                            cx.emit(SetupComplete);
                        }
                        cx.notify();
                    });
                }
            });
        }).detach();
    }

    fn update_task(&mut self, index: usize, status: TaskStatus) {
        if let Some(task) = self.setup_tasks.get_mut(index) {
            task.status = status;
        }
    }

    fn render_task(&self, task: &SetupTask, theme: &ui::Theme) -> impl IntoElement {
        let (icon_name, icon_color) = match &task.status {
            TaskStatus::Pending    => (IconName::Circle,          theme.muted_foreground),
            TaskStatus::InProgress => (IconName::Loader,          theme.accent),
            TaskStatus::Completed  => (IconName::Check,           theme.success_foreground),
            TaskStatus::Failed(_)  => (IconName::WarningTriangle, gpui::red()),
        };

        h_flex()
            .gap_3()
            .items_start()
            .p_3()
            .bg(theme.secondary.opacity(0.3))
            .rounded_md()
            .child(Icon::new(icon_name).size_5().text_color(icon_color))
            .child(
                v_flex()
                    .gap_1()
                    .flex_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(task.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(task.description.clone()),
                    )
                    .children(if let TaskStatus::Failed(ref err) = task.status {
                        Some(
                            div()
                                .text_xs()
                                .text_color(gpui::red())
                                .child(format!("Error: {err}")),
                        )
                    } else {
                        None
                    }),
            )
    }
}

impl Render for DependencySetupWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let progress_w = relative(self.progress.clamp(0.0, 1.0));

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(600.))
                    .gap_6()
                    .p_8()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        Icon::new(IconName::Settings)
                                            .size_6()
                                            .text_color(theme.accent),
                                    )
                                    .child(
                                        div()
                                            .text_2xl()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.foreground)
                                            .child("Dependency Setup"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Installs Rust and all platform requirements via rustup"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .h(px(8.))
                                    .w_full()
                                    .bg(theme.secondary)
                                    .rounded(px(4.))
                                    .relative()
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(4.))
                                            .bg(theme.accent)
                                            .w(progress_w),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{}% Complete", (self.progress * 100.0) as u32)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .children(self.setup_tasks.iter().map(|t| self.render_task(t, theme))),
                    )
                    .children(self.setup_error.as_ref().map(|err| {
                        div()
                            .p_3()
                            .bg(gpui::red().opacity(0.1))
                            .border_1()
                            .border_color(gpui::red())
                            .rounded_md()
                            .child(div().text_sm().text_color(gpui::red()).child(err.clone()))
                    }))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_3()
                            .children((!self.is_running).then(|| {
                                Button::new("cancel")
                                    .label("Cancel")
                                    .ghost()
                                    .on_click(cx.listener(|_, _, _, _| {}))
                            }))
                            .children(
                                (!self.is_running && !self.setup_complete).then(|| {
                                    Button::new("start")
                                        .label("Start Setup")
                                        .primary()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.start_setup(cx);
                                        }))
                                }),
                            )
                            .children(self.is_running.then(|| {
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Installing…")
                            }))
                            .children(self.setup_complete.then(|| {
                                div()
                                    .text_sm()
                                    .text_color(theme.success_foreground)
                                    .child("Complete")
                            })),
                    ),
            )
    }
}
