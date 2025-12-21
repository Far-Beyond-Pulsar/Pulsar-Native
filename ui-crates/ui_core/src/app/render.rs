//! Rendering implementation for PulsarApp

use std::time::Duration;
use gpui::{prelude::*, div, px, relative, rgb, Animation, AnimationExt as _, App, Context, Focusable, FocusHandle, Hsla, IntoElement, MouseButton, Render, Window};
use ui::{
    h_flex, v_flex, ActiveTheme as _, ContextModal as _, StyledExt as _, button::{Button, ButtonVariants as _}, Icon, IconName,
};
use ui::notification::Notification;
use engine_backend::services::rust_analyzer_manager::AnalyzerStatus;

use super::PulsarApp;
use crate::actions::*;

impl PulsarApp {
    pub(super) fn render_footer(&self, drawer_open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let analyzer = self.state.rust_analyzer.read(cx);
        let status = analyzer.status();
        let is_running = analyzer.is_running();

        let error_count = self
            .state.problems_drawer
            .read(cx)
            .count_by_severity(ui_problems::DiagnosticSeverity::Error);
        let warning_count = self
            .state.problems_drawer
            .read(cx)
            .count_by_severity(ui_problems::DiagnosticSeverity::Warning);

        let (status_color, status_icon) = match status {
            AnalyzerStatus::Ready => (cx.theme().success, IconName::CheckCircle),
            AnalyzerStatus::Indexing { .. } | AnalyzerStatus::Starting => {
                (cx.theme().warning, IconName::Loader)
            }
            AnalyzerStatus::Error(_) => (cx.theme().danger, IconName::TriangleAlert),
            AnalyzerStatus::Stopped => (cx.theme().muted_foreground, IconName::Circle),
            AnalyzerStatus::Idle => (cx.theme().muted_foreground, IconName::Circle),
        };

        div()
            .w_full()
            .relative()
            .when(self.state.analyzer_progress > 0.0 && self.state.analyzer_progress < 1.0, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h(px(2.))
                        .w(relative(self.state.analyzer_progress))
                        .bg(cx.theme().primary)
                        .shadow_md(),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .h(px(28.))
                    .items_center()
                    .px_3()
                    .gap_2()
                    .bg(cx.theme().background)
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Button::new("toggle-files")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::Folder)
                                            .size(px(16.))
                                            .text_color(if drawer_open {
                                                cx.theme().primary
                                            } else {
                                                cx.theme().muted_foreground
                                            })
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .when(drawer_open, |s| {
                                        s.bg(cx.theme().primary.opacity(0.15))
                                    })
                                    .tooltip("Toggle Files (Ctrl+B)")
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_drawer(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("toggle-problems")
                                    .ghost()
                                    .icon(
                                        Icon::new(if error_count > 0 {
                                            IconName::Close
                                        } else if warning_count > 0 {
                                            IconName::TriangleAlert
                                        } else {
                                            IconName::CheckCircle
                                        })
                                        .size(px(16.))
                                        .text_color(if error_count > 0 {
                                            cx.theme().danger
                                        } else if warning_count > 0 {
                                            cx.theme().warning
                                        } else {
                                            cx.theme().success
                                        })
                                    )
                                    .relative()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .when(error_count + warning_count > 0, |this| {
                                        this.child(
                                            div()
                                                .absolute()
                                                .top(px(-4.))
                                                .right(px(-4.))
                                                .min_w(px(16.))
                                                .h(px(16.))
                                                .px_1()
                                                .rounded(px(8.))
                                                .bg(if error_count > 0 {
                                                    cx.theme().danger
                                                } else {
                                                    cx.theme().warning
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_bold()
                                                        .text_color(rgb(0xFFFFFF))
                                                        .child(format!("{}", error_count + warning_count)),
                                                ),
                                        )
                                    })
                                    .tooltip(format!(
                                        "{} Errors, {} Warnings",
                                        error_count, warning_count
                                    ))
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_problems(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("toggle-terminal")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::Terminal)
                                            .size(px(16.))
                                            .text_color(cx.theme().muted_foreground)
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .tooltip("Terminal")
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_terminal(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("toggle-multiplayer")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::User)
                                            .size(px(16.))
                                            .text_color(cx.theme().muted_foreground)
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .tooltip("Multiplayer Collaboration")
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_multiplayer(window, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .w(px(1.))
                                    .h(px(18.))
                                    .bg(cx.theme().border),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(status_icon)
                                    .size(px(14.))
                                    .text_color(status_color),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child("rust-analyzer"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(status_color)
                                    .child(self.state.analyzer_status_text.clone()),
                            )
                            .when(!self.state.analyzer_detail_message.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                                        .child("—"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.state.analyzer_detail_message.clone()),
                                )
                            })
                            .child(
                                h_flex()
                                    .gap_0p5()
                                    .ml_2()
                                    .when(is_running, |this| {
                                        this.child(
                                            Button::new("analyzer-stop")
                                                .ghost()
                                                .icon(
                                                    Icon::new(IconName::X)
                                                        .size(px(12.))
                                                        .text_color(cx.theme().muted_foreground)
                                                )
                                                .p_1()
                                                .rounded(px(3.))
                                                .hover(|s| s.bg(cx.theme().danger.opacity(0.2)))
                                                .tooltip("Stop")
                                                .on_click(cx.listener(|app, _, window, cx| {
                                                    app.state.rust_analyzer.update(cx, |analyzer, cx| {
                                                        analyzer.stop(window, cx);
                                                    });
                                                })),
                                        )
                                    })
                                    .child(
                                        Button::new("analyzer-restart")
                                            .ghost()
                                            .icon(
                                                Icon::new(IconName::Undo)
                                                    .size(px(12.))
                                                    .text_color(cx.theme().muted_foreground)
                                            )
                                            .p_1()
                                            .rounded(px(3.))
                                            .tooltip(if is_running { "Restart" } else { "Start" })
                                            .on_click(cx.listener(move |app, _, window, cx| {
                                                if let Some(project) = app.state.project_path.clone() {
                                                    app.state.rust_analyzer.update(cx, |analyzer, cx| {
                                                        if is_running {
                                                            analyzer.restart(window, cx);
                                                        } else {
                                                            analyzer.start(project, window, cx);
                                                        }
                                                    });
                                                }
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(18.))
                            .bg(cx.theme().border),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(14.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .children(
                                        self.state.project_path
                                            .as_ref()
                                            .and_then(|path| path.file_name())
                                            .map(|name| name.to_string_lossy().to_string())
                                            .or(Some("No Project".to_string())),
                                    ),
                            ),
                    ),
            )
    }
}

impl Focusable for PulsarApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.state.focus_handle.clone()
    }
}

impl Render for PulsarApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Show welcome notification on first render if project was loaded
        if !self.state.shown_welcome_notification && self.state.project_path.is_some() {
            self.state.shown_welcome_notification = true;
            if let Some(ref path) = self.state.project_path {
                let project_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Project");
                window.push_notification(
                    Notification::info("Project Loaded")
                        .message(format!("Welcome to {}", project_name)),
                    cx
                );
            }
        }

        // Update rust-analyzer progress if indexing
        self.state.rust_analyzer.update(cx, |analyzer, cx| {
            analyzer.update_progress_from_thread(cx);
        });

        // Show entry screen if no project is loaded
        if let Some(screen) = &self.state.entry_screen {
            return screen.clone().into_any_element();
        }

        let command_palette = if self.state.command_palette_open {
            self.state.command_palette.clone()
        } else {
            None
        };

        let drawer_open = self.state.drawer_open;

        v_flex()
            .size_full()
            .track_focus(&self.state.focus_handle)
            .on_action(cx.listener(Self::on_toggle_file_manager))
            .on_action(cx.listener(Self::on_toggle_problems))
            .on_action(cx.listener(Self::on_toggle_terminal))
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .child(
                div()
                    .flex_1()
                    .relative()
                    .child(self.state.dock_area.clone())
                    .when(drawer_open, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .bg(Hsla::black().opacity(0.3))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|app, _, window, cx| {
                                        app.state.drawer_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .h(px(300.))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(self.state.file_manager_drawer.clone())
                                .with_animation(
                                    "slide-up",
                                    Animation::new(Duration::from_secs_f64(0.2)),
                                    |this, delta| this.bottom(px(-300.) + delta * px(300.)),
                                ),
                        )
                    }),
            )
            .child(self.render_footer(drawer_open, cx))
            .children(command_palette)
            .into_any_element()
    }
}
