//! Window management — opens windows via the PulsarWindow trait system.
//!
//! Each method is a single call. Window size, chrome, and construction logic live
//! in the respective window crate''s `PulsarWindow` impl — not here.

use std::path::PathBuf;
use std::sync::Arc;
use gpui::{px, size, Bounds, Context, AppContext as _, Point, UpdateGlobal, Window, WindowBounds, WindowKind, WindowOptions};
use ui::Root;
use ui_common::open_pulsar_window;
use ui_problems::ProblemsWindow;
use ui_flamegraph::FlamegraphWindow;
use ui_type_debugger::TypeDebuggerWindow;
use ui_multiplayer::MultiplayerWindow;
use ui_plugin_manager::PluginManagerWindow;
use ui_git_manager::GitManager;
use ui_settings::SettingsWindow;
use ui_log_viewer::MissionControlPanel;

use super::PulsarApp;
use super::panel_window::PanelWindow;

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
        tracing::trace!("[POPOUT] Creating detached window for panel at position: {:?}", position);
        self.state.popped_out_panels.push(panel.clone());

        let window_bounds = Bounds::new(
            Point { x: position.x - px(100.0), y: position.y - px(30.0) },
            size(px(800.), px(600.)),
        );
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: None,
            window_min_size: Some(gpui::Size { width: px(400.), height: px(300.) }),
            kind: WindowKind::Normal,
            is_resizable: true,
            window_decorations: Some(gpui::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        let center_tabs = self.state.center_tabs.clone();
        let panel_for_popout = panel.clone();
        let parent_window_handle = parent_window.window_handle();

        let _ = window_manager::WindowManager::update_global(cx, |wm, cx| {
            wm.create_window(
                window_manager::WindowRequest::DetachedPanel,
                window_options,
                move |window, cx| {
                    let panel_window = cx.new(|cx| PanelWindow::new(
                        panel_for_popout, center_tabs, parent_window_handle.into(), window, cx,
                    ));
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

    pub(super) fn toggle_problems(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        open_pulsar_window::<ProblemsWindow>(self.state.problems_drawer.clone(), cx);
    }

    pub(super) fn toggle_type_debugger(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        open_pulsar_window::<TypeDebuggerWindow>(self.state.type_debugger_drawer.clone(), cx);
    }

    pub(super) fn toggle_log_viewer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.mission_control_open {
            self.state.mission_control_open = true;
            open_pulsar_window::<MissionControlPanel>((), cx);
        } else {
            self.state.mission_control_open = false;
        }
    }

    pub(super) fn open_git_manager(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.git_manager_open = true;
        let path = self.state.project_path.clone().unwrap_or_else(|| PathBuf::from("."));
        open_pulsar_window::<GitManager>(path, cx);
    }

    pub(super) fn toggle_multiplayer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        open_pulsar_window::<MultiplayerWindow>(self.state.project_path.clone(), cx);
    }

    pub(super) fn toggle_plugin_manager(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        open_pulsar_window::<PluginManagerWindow>((), cx);
    }

    pub(super) fn open_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        open_pulsar_window::<SettingsWindow>((), cx);
    }

    pub(super) fn toggle_flamegraph(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        FlamegraphWindow::open(cx);
    }
}
