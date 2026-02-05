//! Rendering implementation for PulsarApp

use std::time::Duration;
use gpui::{prelude::*, div, px, relative, rgb, Animation, AnimationExt as _, AnyElement, App, Context, Focusable, FocusHandle, Hsla, IntoElement, MouseButton, MouseMoveEvent, Render, Window};
use ui::{
    h_flex, v_flex, ActiveTheme as _, ContextModal as _, StyledExt as _, button::{Button, ButtonVariants as _}, Icon, IconName,
};
use ui::notification::Notification;
use rust_i18n::t;
use engine_backend::services::rust_analyzer_manager::AnalyzerStatus;
use plugin_editor_api::{StatusbarPosition, StatusbarAction};
use std::path::PathBuf;

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

        let type_count = self
            .state.type_debugger_drawer
            .read(cx)
            .total_count();

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
                                Button::new("toggle-type-debugger")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::Database)
                                            .size(px(16.))
                                            .text_color(cx.theme().accent)
                                    )
                                    .relative()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .when(type_count > 0, |this| {
                                        this.child(
                                            div()
                                                .absolute()
                                                .top(px(-4.))
                                                .right(px(-4.))
                                                .min_w(px(16.))
                                                .h(px(16.))
                                                .px_1()
                                                .rounded(px(8.))
                                                .bg(cx.theme().accent)
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_bold()
                                                        .text_color(rgb(0xFFFFFF))
                                                        .child(format!("{}", type_count)),
                                                ),
                                        )
                                    })
                                    .tooltip(format!("{} Types", type_count))
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_type_debugger(window, cx);
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
                                    .tooltip(t!("StatusBar.Multiplayer").to_string())
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_multiplayer(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("toggle-plugin-manager")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::Puzzle)
                                            .size(px(16.))
                                            .text_color(cx.theme().muted_foreground)
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .tooltip(t!("StatusBar.PluginManager").to_string())
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_plugin_manager(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("toggle-flamegraph")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::Activity)
                                            .size(px(16.))
                                            .text_color(cx.theme().muted_foreground)
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .tooltip(t!("StatusBar.Flamegraph").to_string())
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_flamegraph(window, cx);
                                    })),
                            )
                            // Render plugin statusbar buttons for left position
                            .children(self.render_plugin_statusbar_buttons(StatusbarPosition::Left, cx))
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
                                    .child(t!("StatusBar.RustAnalyzer").to_string()),
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
                                                    Icon::new(IconName::Close)
                                                        .size(px(12.))
                                                        .text_color(cx.theme().muted_foreground)
                                                )
                                                .p_1()
                                                .rounded(px(3.))
                                                .hover(|s| s.bg(cx.theme().danger.opacity(0.2)))
                                                .tooltip(t!("StatusBar.Stop").to_string())
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
                                            .tooltip(if is_running { t!("StatusBar.Restart").to_string() } else { t!("StatusBar.Start").to_string() })
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
                    // Render plugin statusbar buttons for right position
                    .children(
                        self.render_plugin_statusbar_buttons(StatusbarPosition::Right, cx)
                            .into_iter()
                            .map(|btn| btn.into_any_element())
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
    
    /// Render statusbar buttons registered by plugins
    fn render_plugin_statusbar_buttons(&self, position: StatusbarPosition, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let buttons = self.state.plugin_manager.get_statusbar_buttons_for_position(position);
        
        buttons
            .into_iter()
            .enumerate()
            .map(|(idx, btn_def)| {
                let mut button = Button::new(("plugin-statusbar", idx))
                    .ghost()
                    .icon(
                        Icon::new(btn_def.icon.clone())
                            .size(px(16.))
                            .text_color(btn_def.icon_color.unwrap_or_else(|| cx.theme().muted_foreground))
                    )
                    .relative()
                    .px_2()
                    .py_1()
                    .rounded(px(4.));
                
                // Add active styling if specified
                if btn_def.active {
                    button = button.bg(cx.theme().primary.opacity(0.15));
                }
                
                // Add badge if specified
                if let Some(count) = btn_def.badge_count {
                    if count > 0 {
                        button = button.child(
                            div()
                                .absolute()
                                .top(px(-4.))
                                .right(px(-4.))
                                .min_w(px(16.))
                                .h(px(16.))
                                .px_1()
                                .rounded(px(8.))
                                .bg(btn_def.badge_color.unwrap_or_else(|| cx.theme().accent))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_bold()
                                        .text_color(rgb(0xFFFFFF))
                                        .child(count.to_string()),
                                ),
                        );
                    }
                }
                
                // Add tooltip
                button = button.tooltip(btn_def.tooltip.clone());
                
                // Clone what we need for the closure
                let action = btn_def.action.clone();
                let callback = btn_def.custom_callback;
                
                // Add click handler based on action type
                button = match action {
                    StatusbarAction::OpenEditor { editor_id, file_path } => {
                        button.on_click(cx.listener(move |app, _, window, cx| {
                            tracing::info!("Opening editor {:?}", editor_id);
                            
                            let path = file_path.clone().unwrap_or_else(|| PathBuf::new());
                            
                            // Find which plugin owns this editor
                            let plugin_id: Option<plugin_editor_api::PluginId> = app.state.plugin_manager.find_plugin_for_editor(&editor_id);
                            
                            if let Some(plugin_id) = plugin_id {
                                match app.state.plugin_manager.create_editor(
                                    &plugin_id,
                                    &editor_id,
                                    path,
                                    window,
                                    cx
                                ) {
                                    Ok((panel, _editor_instance)) => {
                                        app.state.center_tabs.update(cx, |tabs, cx| {
                                            tabs.add_panel(panel, window, cx);
                                        });
                                        tracing::info!("Successfully opened editor {:?}", editor_id);
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to open editor {:?}: {:?}", editor_id, e);
                                    }
                                }
                            } else {
                                tracing::error!("No plugin found for editor {:?}", editor_id);
                            }
                        }))
                    }
                    StatusbarAction::ToggleDrawer { drawer_id } => {
                        button.on_click(cx.listener(move |_app, _, _window, _cx| {
                            tracing::info!("Plugin statusbar button clicked: toggle drawer {}", drawer_id);
                        }))
                    }
                    StatusbarAction::Custom => {
                        if let Some(cb) = callback {
                            button.on_click(move |_, window, cx| {
                                cb(window, cx);
                            })
                        } else {
                            button
                        }
                    }
                };
                
                button.into_any_element()
            })
            .collect()
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
            self.state.command_palette_view.clone()
        } else {
            None
        };

        let drawer_open = self.state.drawer_open;

        v_flex()
            .size_full()
            .track_focus(&self.state.focus_handle)
            .on_action(cx.listener(Self::on_toggle_file_manager))
            .on_action(cx.listener(Self::on_toggle_problems))
            .on_action(cx.listener(Self::on_toggle_type_debugger))
            .on_action(cx.listener(Self::on_toggle_flamegraph))
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .on_action(cx.listener(Self::on_open_file))
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
                                .h(px(self.state.drawer_height))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                                .child(
                                    v_flex()
                                        .size_full()
                                        .child(
                                            // Resize handle at top  
                                            div()
                                                .id("drawer-resize-handle")
                                                .w_full()
                                                .h(px(6.))
                                                .cursor_ns_resize()
                                                .bg(cx.theme().border.opacity(0.5))
                                                .hover(|style| style.bg(cx.theme().accent).h(px(8.)))
                                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                                    this.state.drawer_resizing = true;
                                                    cx.notify();
                                                }))
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .child(self.state.file_manager_drawer.clone())
                                        )
                                )
                                .with_animation(
                                    "slide-up",
                                    Animation::new(Duration::from_secs_f64(0.2)),
                                    {
                                        let height = self.state.drawer_height;
                                        move |this, delta| this.bottom(px(-height) + delta * px(height))
                                    },
                                ),
                        )
                        .when(self.state.drawer_resizing, |this| {
                            this.on_mouse_move(cx.listener(|app, event: &MouseMoveEvent, window, cx| {
                                let window_height: f32 = window.viewport_size().height.into();
                                let mouse_y: f32 = event.position.y.into();
                                let new_height = window_height - mouse_y;
                                app.state.drawer_height = new_height.clamp(200.0, 700.0);
                                cx.notify();
                            }))
                            .on_mouse_up(MouseButton::Left, cx.listener(|app, _event, _window, cx| {
                                app.state.drawer_resizing = false;
                                cx.notify();
                            }))
                        })
                    }),
            )
            .child(self.render_footer(drawer_open, cx))
            .children(command_palette)
            .into_any_element()
    }
}
