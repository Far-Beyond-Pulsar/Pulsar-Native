//! Window management - opens windows via the PulsarWindow trait system.
//!
//! Each method is a single call. Window size, chrome, and construction logic live
//! in the respective window crate''s `PulsarWindow` impl - not here.

use gpui::{AppContext as _, Context, UpdateGlobal, Window};
use std::path::PathBuf;
use std::sync::Arc;
use ui::dock::DockPlacement;
use ui::Root;
use ui_about::AboutWindow;
use ui_common::PulsarWindowExt as _;
use ui_documentation::DocumentationWindow;
use ui_flamegraph::FlamegraphWindow;
use ui_git_manager::GitManager;
use ui_log_viewer::MissionControlPanel;
use ui_multiplayer::MultiplayerWindow;
use ui_plugin_manager::PluginManagerWindow;
use ui_problems::ProblemsWindow;
use ui_settings::SettingsWindow;
use ui_type_debugger::TypeDebuggerWindow;
use window_manager::WindowConfig;

use super::panel_window::PanelWindow;
use super::PulsarApp;

impl PulsarApp {
    /// Create a detached pop-out window for a panel.
    /// Uses a custom layout (position follows cursor), so it stays manual.
    pub(super) fn create_detached_window(
        &mut self,
        panel: Arc<dyn ui::dock::PanelView>,
        position: gpui::Point<gpui::Pixels>,
        parent_window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::trace!(
            "[POPOUT] Creating detached window for panel at position: {:?}",
            position
        );
        self.state.popped_out_panels.push(panel.clone());

        let center_tabs = self.state.center_tabs.clone();
        let panel_for_popout = panel.clone();
        let parent_window_handle = parent_window.window_handle();

        let _ = window_manager::WindowManager::update_global(cx, |wm, cx| {
            wm.create_window(
                window_manager::WindowRequest::DetachedPanel,
                WindowConfig::detached_panel(position),
                move |window, cx| {
                    let panel_window = cx.new(|cx| {
                        PanelWindow::new(
                            panel_for_popout,
                            center_tabs,
                            parent_window_handle,
                            window,
                            cx,
                        )
                    });
                    cx.new(|cx| Root::new(panel_window.into(), window, cx))
                },
                cx,
            )
        });
    }

    pub(super) fn toggle_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.drawer_open = !self.state.drawer_open;
        cx.notify();
    }

    /// Close the file manager drawer
    pub(super) fn close_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.drawer_open = false;
        cx.notify();
    }

    /// Open the file manager drawer (respects suppress_drawer_for_drag flag)
    pub(super) fn open_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.suppress_drawer_for_drag {
            self.state.drawer_open = true;
            cx.notify();
        }
    }

    pub(super) fn toggle_problems(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        ProblemsWindow::open(self.state.problems_drawer.clone(), cx);
    }

    pub(super) fn toggle_type_debugger(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        TypeDebuggerWindow::open(self.state.type_debugger_drawer.clone(), cx);
    }

    pub(super) fn toggle_log_viewer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.mission_control_open {
            self.state.mission_control_open = true;
            MissionControlPanel::open((), cx);
        } else {
            self.state.mission_control_open = false;
        }
    }

    pub(super) fn open_git_manager(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.git_manager_open = true;
        let path = self
            .state
            .project_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        GitManager::open(path, cx);
    }

    pub(super) fn toggle_multiplayer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        MultiplayerWindow::open(self.state.project_path.clone(), cx);
    }

    pub(super) fn toggle_agent_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.dock_area.update(cx, |dock, cx| {
            dock.toggle_dock(DockPlacement::Left, window, cx);
        });
        cx.notify();
    }

    pub(super) fn toggle_plugin_manager(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        PluginManagerWindow::open((), cx);
    }

    pub fn open_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        SettingsWindow::open((), cx);
    }

    pub fn open_about(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        AboutWindow::open((), cx);
    }

    pub fn open_documentation(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        DocumentationWindow::open((), cx);
    }

    pub(super) fn toggle_flamegraph(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        FlamegraphWindow::open(std::sync::Arc::new(ui_flamegraph::TraceData::new()), cx);
    }

    pub(super) fn toggle_project_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.project_switcher_open {
            use ui_common::command_palette::GenericPalette;

            let delegate = crate::project_switcher::ProjectSwitcherDelegate::new();
            let view = cx.new(|cx| GenericPalette::new(delegate, window, cx));

            let view_for_dismiss = view.clone();
            let window_handle = window.window_handle();
            cx.subscribe_in(
                &view,
                window,
                move |this, _, _: &gpui::DismissEvent, window, cx| {
                    let selected = view_for_dismiss.update(cx, |palette, _| {
                        palette.delegate_mut().selected_project.take()
                    });

                    if let Some(selected) = selected {
                        let project_path = std::path::PathBuf::from(&selected.path);
                        let originating_window_handle = window_handle.clone();
                        let on_complete: std::sync::Arc<
                            dyn Fn(std::path::PathBuf, &mut gpui::App) + Send + Sync,
                        > = std::sync::Arc::new(move |path, cx| {
                            crate::PulsarRoot::open(path, cx);
                            // Close the originating window only after the target editor opens.
                            cx.update_window(originating_window_handle, |_, win, _| {
                                win.remove_window()
                            });
                        });

                        cx.defer({
                            let path = project_path.clone();
                            let callback = on_complete.clone();
                            move |cx| {
                                ui_loading_screen::LoadingScreen::open((path, callback), cx);
                            }
                        });
                    }

                    this.state.project_switcher_open = false;
                    this.state.project_switcher_view = None;
                    this.state.focus_handle.focus(window, cx);
                    cx.notify();
                },
            )
            .detach();

            self.state.project_switcher_open = true;
            self.state.project_switcher_view = Some(view);
        } else {
            self.state.project_switcher_open = false;
            self.state.project_switcher_view = None;
        }

        cx.notify();
    }
}
