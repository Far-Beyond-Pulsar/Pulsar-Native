//! Window management - opens windows via the PulsarWindow trait system.
//!
//! Each method is a single call. Window size, chrome, and construction logic live
//! in the respective window crate''s `PulsarWindow` impl - not here.

use gpui::{AppContext as _, Context, UpdateGlobal, Window};
use ui::dock::DockPlacement;
use ui_common::PulsarWindowExt as _;
use window_manager::WindowRegistry;

use super::PulsarApp;

impl PulsarApp {
    pub(super) fn toggle_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.state.chrome.drawer_docked {
            return;
        }
        self.state.chrome.drawer_open = !self.state.chrome.drawer_open;
        cx.notify();
    }

    /// Close the file manager drawer
    pub(super) fn close_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.chrome.drawer_open = false;
        cx.notify();
    }

    /// Open the file manager drawer (respects suppress_drawer_for_drag flag)
    pub(super) fn open_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.chrome.suppress_drawer_for_drag {
            self.state.chrome.drawer_open = true;
            cx.notify();
        }
    }

    pub(super) fn toggle_problems(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        WindowRegistry::update_global(cx, |reg, cx| reg.open("ProblemsWindow", cx));
    }

    pub(super) fn toggle_type_debugger(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        WindowRegistry::update_global(cx, |reg, cx| reg.open("TypeDebuggerWindow", cx));
    }

    pub(super) fn toggle_log_viewer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.chrome.mission_control_open {
            self.state.chrome.mission_control_open = true;
            WindowRegistry::update_global(cx, |reg, cx| reg.open("MissionControlPanel", cx));
        } else {
            self.state.chrome.mission_control_open = false;
        }
    }

    pub(super) fn open_git_manager(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.chrome.git_manager_open = true;
        WindowRegistry::update_global(cx, |reg, cx| reg.open("GitManagerWindow", cx));
    }

    pub(super) fn toggle_multiplayer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        WindowRegistry::update_global(cx, |reg, cx| reg.open("MultiplayerWindow", cx));
    }

    pub(super) fn toggle_agent_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.dock_area.update(cx, |dock, cx| {
            dock.toggle_dock(DockPlacement::Left, window, cx);
        });
        cx.notify();
    }

    pub(super) fn toggle_plugin_manager(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        WindowRegistry::update_global(cx, |reg, cx| reg.open("PluginManagerWindow", cx));
    }

    pub(super) fn toggle_flamegraph(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        WindowRegistry::update_global(cx, |reg, cx| reg.open("FlamegraphWindow", cx));
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
