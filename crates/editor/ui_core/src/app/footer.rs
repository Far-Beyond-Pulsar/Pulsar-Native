//! The editor footer: the toggle buttons, the language server entries, the
//! buttons plugins add, and the project switcher.

use gpui::UpdateGlobal as _;
use gpui::{
    div, prelude::*, px, relative, rgb, AnyElement, Context, Div, Hsla, IntoElement, MouseButton,
};
use plugin_editor_api::{StatusbarAction, StatusbarButtonDefinition, StatusbarPosition};
use rust_i18n::t;
use std::path::PathBuf;
use ui::{
    button::{Button, ButtonVariants as _},
    dock::DockPlacement,
    h_flex, ActiveTheme as _, Icon, IconName, StyledExt as _,
};
use ui_multiuser_status::render_status_bar_indicator;

use super::PulsarApp;

/// The thin progress bar along the footer's top edge while language servers
/// work.
fn server_progress_bar(progress: f32, color: Hsla) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .h(px(2.))
        .w(relative(progress))
        .bg(color)
        .shadow_md()
}

/// A small count badge in a button's top-right corner.
fn count_badge(color: Hsla, label: String) -> Div {
    div()
        .absolute()
        .top(px(-4.))
        .right(px(-4.))
        .min_w(px(16.))
        .h(px(16.))
        .px_1()
        .rounded(px(8.))
        .bg(color)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_xs()
                .font_bold()
                .text_color(rgb(0xFFFFFF))
                .child(label),
        )
}

fn files_button(drawer_open: bool, cx: &mut Context<PulsarApp>) -> Button {
    Button::new("toggle-files")
        .ghost()
        .label("Files")
        .icon(
            Icon::new(IconName::Folder)
                .size(px(16.))
                .text_color(if drawer_open {
                    cx.theme().primary
                } else {
                    cx.theme().muted_foreground
                }),
        )
        .px_3()
        .py_1()
        .rounded(px(4.))
        .when(drawer_open, |s| s.bg(cx.theme().primary.opacity(0.15)))
        .tooltip("Toggle Files (Ctrl+Space)")
        .on_click(cx.listener(|app, _, window, cx| {
            app.toggle_drawer(window, cx);
        }))
}

fn tasks_button(active_task_count: usize, cx: &mut Context<PulsarApp>) -> Button {
    Button::new("open-editor-tasks")
        .ghost()
        .label(if active_task_count > 0 {
            format!("Tasks {active_task_count}")
        } else {
            "Tasks".to_string()
        })
        .icon(
            Icon::new(IconName::TaskList)
                .size(px(16.))
                .text_color(cx.theme().muted_foreground),
        )
        .px_2()
        .py_1()
        .rounded(px(4.))
        .tooltip("Open editor tasks")
        .on_click(|_, _, cx| {
            window_manager::WindowRegistry::update_global(cx, |reg, cx| {
                reg.open("EditorTasksWindow", cx)
            });
        })
}

fn problems_button(
    error_count: usize,
    warning_count: usize,
    cx: &mut Context<PulsarApp>,
) -> Button {
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
            }),
        )
        .relative()
        .px_2()
        .py_1()
        .rounded(px(4.))
        .when(error_count + warning_count > 0, |this| {
            this.child(count_badge(
                if error_count > 0 {
                    cx.theme().danger
                } else {
                    cx.theme().warning
                },
                format!("{}", error_count + warning_count),
            ))
        })
        .tooltip(format!(
            "{} Errors, {} Warnings",
            error_count, warning_count
        ))
        .on_click(cx.listener(|app, _, window, cx| {
            app.toggle_problems(window, cx);
        }))
}

fn type_debugger_button(type_count: usize, cx: &mut Context<PulsarApp>) -> Button {
    Button::new("toggle-type-debugger")
        .ghost()
        .icon(Icon::new(IconName::Database).size(px(16.)))
        .relative()
        .px_2()
        .py_1()
        .rounded(px(4.))
        .when(type_count > 0, |this| {
            this.child(count_badge(cx.theme().accent, format!("{}", type_count)))
        })
        .tooltip(format!("{} Types", type_count))
        .on_click(cx.listener(|app, _, window, cx| {
            app.toggle_type_debugger(window, cx);
        }))
}

fn multiplayer_button(cx: &mut Context<PulsarApp>) -> Button {
    Button::new("toggle-multiplayer")
        .ghost()
        .icon(
            Icon::new(IconName::User)
                .size(px(16.))
                .text_color(cx.theme().muted_foreground),
        )
        .px_2()
        .py_1()
        .rounded(px(4.))
        .tooltip(t!("StatusBar.Multiplayer").to_string())
        .on_click(cx.listener(|app, _, window, cx| {
            app.toggle_multiplayer(window, cx);
        }))
}

fn agent_chat_button(is_agent_chat_open: bool, cx: &mut Context<PulsarApp>) -> Button {
    Button::new("toggle-agent-chat")
        .ghost()
        .icon(
            Icon::new(IconName::PanelRight)
                .size(px(16.))
                .text_color(if is_agent_chat_open {
                    cx.theme().primary
                } else {
                    cx.theme().muted_foreground
                }),
        )
        .px_2()
        .py_1()
        .rounded(px(4.))
        .when(is_agent_chat_open, |s| {
            s.bg(cx.theme().primary.opacity(0.15))
        })
        .tooltip("Toggle Agent Chat")
        .on_click(cx.listener(|app, _, window, cx| {
            app.toggle_agent_chat(window, cx);
        }))
}

fn plugin_manager_button(cx: &mut Context<PulsarApp>) -> Button {
    Button::new("toggle-plugin-manager")
        .ghost()
        .icon(
            Icon::new(IconName::Puzzle)
                .size(px(16.))
                .text_color(cx.theme().muted_foreground),
        )
        .px_2()
        .py_1()
        .rounded(px(4.))
        .tooltip(t!("StatusBar.PluginManager").to_string())
        .on_click(cx.listener(|app, _, window, cx| {
            app.toggle_plugin_manager(window, cx);
        }))
}

impl PulsarApp {
    pub(super) fn render_footer(
        &self,
        drawer_open: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let server_entries =
            super::language_server_footer::entries(self.state.language_servers.read(cx), cx);
        let server_progress = super::language_server_footer::progress_of(&server_entries);

        div()
            .w_full()
            .relative()
            .when(server_progress > 0.0, |this| {
                this.child(server_progress_bar(server_progress, cx.theme().primary))
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
                    .child(self.footer_toggle_buttons(drawer_open, cx))
                    .child(
                        h_flex()
                            .flex_1()
                            .items_center()
                            .gap_2()
                            // One "LSP Servers" entry: aggregate state and
                            // progress; a click lists every server.
                            .children(self.render_language_server_entries(&server_entries, cx)),
                    )
                    .child(div().w(px(1.)).h(px(18.)).bg(cx.theme().border))
                    // Render plugin statusbar buttons for right position
                    .children(
                        self.render_plugin_statusbar_buttons(StatusbarPosition::Right, cx)
                            .into_iter()
                            .map(|btn| btn.into_any_element()),
                    )
                    .child(render_status_bar_indicator(cx))
                    .child(self.project_switcher_button(cx)),
            )
    }

    /// The footer's left group: the drawer, task, problem, type, multiplayer,
    /// agent chat and plugin manager toggles, then the editor tools' and the
    /// plugins' buttons.
    fn footer_toggle_buttons(&self, drawer_open: bool, cx: &mut Context<Self>) -> Div {
        let error_count = self
            .state
            .problems_drawer
            .read(cx)
            .count_all(ui_problems::FindingSeverity::Error);
        let warning_count = self
            .state
            .problems_drawer
            .read(cx)
            .count_all(ui_problems::FindingSeverity::Warning);

        let type_count = self.state.type_debugger_drawer.read(cx).total_count();
        let active_task_count = editor_task_queue::global()
            .snapshots()
            .iter()
            .filter(|task| {
                matches!(
                    task.status,
                    editor_task_queue::TaskStatus::Queued | editor_task_queue::TaskStatus::Running
                )
            })
            .count();
        let is_agent_chat_open = self
            .state
            .dock_area
            .read(cx)
            .is_dock_open(DockPlacement::Left, cx);

        h_flex()
            .gap_1()
            .items_center()
            .child(files_button(drawer_open, cx))
            .child(tasks_button(active_task_count, cx))
            .child(problems_button(error_count, warning_count, cx))
            .child(type_debugger_button(type_count, cx))
            .child(multiplayer_button(cx))
            .child(agent_chat_button(is_agent_chat_open, cx))
            .child(plugin_manager_button(cx))
            // The editor tools' buttons (`ui_common::editor_tools`).
            .children(self.render_editor_tool_buttons(cx))
            // Render plugin statusbar buttons for left position
            .children(self.render_plugin_statusbar_buttons(StatusbarPosition::Left, cx))
            .child(div().w(px(1.)).h(px(18.)).bg(cx.theme().border))
    }

    /// The project name that opens the project switcher.
    fn project_switcher_button(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(cx.theme().muted.opacity(0.1)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, window, cx| {
                    app.toggle_project_switcher(window, cx);
                }),
            )
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
                        self.state
                            .project_path
                            .as_ref()
                            .and_then(|path| path.file_name())
                            .map(|name| name.to_string_lossy().to_string())
                            .or(Some("No Project".to_string())),
                    ),
            )
    }

    /// The editor tools' status-bar buttons
    /// (`ui_common::editor_tools::EditorToolRegistration`), lit while their
    /// window was opened from here.
    fn render_editor_tool_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        ui_common::editor_tools::editor_tools()
            .into_iter()
            .map(|tool| {
                let lit = self.state.chrome.open_tools.contains(tool.id)
                    || tool.is_open.is_some_and(|is_open| is_open(cx));
                let id = tool.id;
                Button::new(gpui::SharedString::from(format!("editor-tool-{id}")))
                    .ghost()
                    .icon(
                        Icon::new(tool.icon.clone())
                            .size(px(16.))
                            .text_color(if lit {
                                cx.theme().primary
                            } else {
                                cx.theme().muted_foreground
                            }),
                    )
                    .px_2()
                    .py_1()
                    .rounded(px(4.))
                    .when(lit, |s| s.bg(cx.theme().primary.opacity(0.15)))
                    .tooltip((tool.label)())
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.toggle_editor_tool(id, window, cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// Render statusbar buttons registered by plugins
    fn render_plugin_statusbar_buttons(
        &self,
        position: StatusbarPosition,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let buttons = if let Some(pm_lock) = plugin_manager::global() {
            pm_lock
                .read()
                .get_statusbar_buttons_for_position(position)
                .into_iter()
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        buttons
            .into_iter()
            .enumerate()
            .map(|(idx, btn_def)| plugin_statusbar_button(idx, btn_def, cx))
            .collect()
    }

    /// Open the editor `editor_id` (on `file_path`, when given) in the center
    /// tabs, through the plugin that registered it.
    fn open_plugin_editor(
        &mut self,
        editor_id: &plugin_editor_api::EditorId,
        file_path: &Option<PathBuf>,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        tracing::info!("Opening editor {:?}", editor_id);

        let path = file_path.clone().unwrap_or_else(PathBuf::new);

        // Find which plugin owns this editor
        if let Some(pm_lock) = plugin_manager::global() {
            let plugin_id: Option<plugin_editor_api::PluginId> = pm_lock
                .read()
                .editor_registry()
                .get_plugin_for_editor(editor_id)
                .cloned();

            if let Some(plugin_id) = plugin_id {
                let mut pm = pm_lock.write();
                super::refresh_plugin_editor_settings(&mut pm);
                match pm.create_editor(&plugin_id, editor_id, path, window, cx) {
                    Ok(panel) => {
                        self.state.center_tabs.update(cx, |tabs, cx| {
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
        }
    }
}

/// One status-bar button a plugin registered: icon, badge, tooltip and its
/// click action.
fn plugin_statusbar_button(
    idx: usize,
    btn_def: StatusbarButtonDefinition,
    cx: &mut Context<PulsarApp>,
) -> AnyElement {
    let mut button = Button::new(("plugin-statusbar", idx))
        .ghost()
        .icon(
            Icon::new(btn_def.icon.clone()).size(px(16.)).text_color(
                btn_def
                    .icon_color
                    .unwrap_or_else(|| cx.theme().muted_foreground),
            ),
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
            button = button.child(count_badge(
                btn_def.badge_color.unwrap_or_else(|| cx.theme().accent),
                count.to_string(),
            ));
        }
    }

    // Add tooltip
    button = button.tooltip(btn_def.tooltip.clone());

    // Clone what we need for the closure
    let action = btn_def.action.clone();
    let callback = btn_def.custom_callback;

    // Add click handler based on action type
    button = match action {
        StatusbarAction::OpenEditor {
            editor_id,
            file_path,
        } => button.on_click(cx.listener(move |app, _, window, cx| {
            app.open_plugin_editor(&editor_id, &file_path, window, cx);
        })),
        StatusbarAction::ToggleDrawer { drawer_id } => {
            button.on_click(cx.listener(move |_app, _, _window, _cx| {
                tracing::info!(
                    "Plugin statusbar button clicked: toggle drawer {}",
                    drawer_id
                );
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
}
