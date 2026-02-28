//! Window management and creation logic

use std::path::PathBuf;
use std::sync::Arc;
use gpui::{px, size, Bounds, Context, Point, UpdateGlobal, Window, WindowBounds, WindowKind, WindowOptions};
use gpui::AppContext;
use ui::Root;
use ui_problems::ProblemsWindow;
use ui_flamegraph::{FlamegraphWindow, TraceData};
use ui_type_debugger::TypeDebuggerWindow;
use ui_multiplayer::MultiplayerWindow;
use ui_plugin_manager::PluginManagerWindow;
use ui_git_manager::create_git_manager_component;
use ui_settings::create_settings_component;
use engine_state::{EngineContext, WindowContext, WindowRequest};

use super::PulsarApp;
use super::panel_window::PanelWindow;

impl PulsarApp {
    /// Create a detached window with a panel in a dedicated popup window
    pub(super) fn create_detached_window(
        &mut self,
        panel: Arc<dyn ui::dock::PanelView>,
        position: gpui::Point<gpui::Pixels>,
        parent_window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::trace!("[POPOUT] Creating detached window for panel at position: {:?}", position);

        // Track the panel so we can restore it when the window closes
        self.state.popped_out_panels.push(panel.clone());

        let window_size = size(px(800.), px(600.));
        let window_bounds = Bounds::new(
            Point {
                x: position.x - px(100.0),
                y: position.y - px(30.0),
            },
            window_size,
        );

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: None,
            window_min_size: Some(gpui::Size {
                width: px(400.),
                height: px(300.),
            }),
            kind: WindowKind::Normal,
            is_resizable: true,
            window_decorations: Some(gpui::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        tracing::trace!("[POPOUT] Opening window with options");

        // Register with engine context
        if let Some(ec) = EngineContext::global() {
            let wid = ec.next_window_id();
            ec.register_window(wid, WindowContext::new(wid, WindowRequest::DetachedPanel));
            tracing::trace!("[POPOUT] Registered detached panel window id={}", wid);
        }

        // Store reference to the center tabs and parent window handle for restoration
        let center_tabs = self.state.center_tabs.clone();
        let panel_for_popout = panel.clone();
        let parent_window_handle = parent_window.window_handle();

        // Replace direct cx.open_window with window_manager::WindowManager::global().create_window
        let _ = window_manager::WindowManager::update_global(cx, |wm, cx| {
            wm.create_window(
                engine_state::WindowRequest::DetachedPanel,
                window_options,
                move |window: &mut gpui::Window, cx: &mut gpui::App| {
                    tracing::trace!("[POPOUT] Inside window creation callback");
                    let panel_window = cx.new(|cx| PanelWindow::new(
                        panel_for_popout, 
                        center_tabs, 
                        parent_window_handle.into(),
                        window, 
                        cx
                    ));
                    tracing::trace!("[POPOUT] PanelWindow created successfully");
                    cx.new(|cx| Root::new(panel_window.into(), window, cx))
                },
                cx,
            )
        });
        tracing::trace!("[POPOUT] Window opened successfully");
    }

    pub(super) fn toggle_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.drawer_open = !self.state.drawer_open;
        cx.notify();
    }

    pub(super) fn toggle_problems(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ec) = EngineContext::global() {
            let wid = ec.next_window_id();
            ec.register_window(wid, WindowContext::new(wid, WindowRequest::Problems));
            tracing::debug!("opening problems window id={}", wid);
        }

        let problems_drawer = self.state.problems_drawer.clone();

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(100.0),
                        y: px(100.0),
                    },
                    size: size(px(900.0), px(600.0)),
                })),
                titlebar: None,
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(500.),
                    height: px(300.),
                }),
                ..Default::default()
            },
            |window, cx| {
                let problems_window = cx.new(|cx| ProblemsWindow::new(problems_drawer, cx));
                cx.new(|cx| Root::new(problems_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_type_debugger(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ec) = EngineContext::global() {
            let wid = ec.next_window_id();
            ec.register_window(wid, WindowContext::new(wid, WindowRequest::TypeDebugger));
            tracing::debug!("opening type debugger window id={}", wid);
        }

        let type_debugger_drawer = self.state.type_debugger_drawer.clone();

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(120.0),
                        y: px(120.0),
                    },
                    size: size(px(1000.0), px(700.0)),
                })),
                titlebar: None,
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(600.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            |window, cx| {
                let type_debugger_window = cx.new(|cx| TypeDebuggerWindow::new(type_debugger_drawer, cx));
                cx.new(|cx| Root::new(type_debugger_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_log_viewer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.mission_control_open = !self.state.mission_control_open;

        if self.state.mission_control_open {
            if let Some(ec) = EngineContext::global() {
                let wid = ec.next_window_id();
                ec.register_window(wid, WindowContext::new(wid, WindowRequest::LogViewer));
                tracing::debug!("opening log viewer window id={}", wid);
            }

            let mission_control = self.state.mission_control.clone();

            let _ = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point {
                            x: px(140.0),
                            y: px(140.0),
                        },
                        size: size(px(1920.0), px(1080.0)),
                    })),
                    titlebar: None,
                    kind: WindowKind::Normal,
                    is_resizable: true,
                    window_decorations: Some(gpui::WindowDecorations::Client),
                    window_min_size: Some(gpui::Size {
                        width: px(800.),
                        height: px(500.),
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    // Start monitoring when window is created
                    mission_control.update(cx, |mc, cx| {
                        mc.start_monitoring(cx);
                    });
                    cx.new(|cx| Root::new(mission_control.into(), window, cx))
                },
            );
        }
    }

    pub(super) fn open_git_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.git_manager_open = true;
        let project_path = self.state.project_path.clone()
            .unwrap_or_else(|| PathBuf::from("."));

        if let Some(ec) = EngineContext::global() {
            let wid = ec.next_window_id();
            ec.register_window(wid, WindowContext::new(wid, WindowRequest::GitManager { project_path: project_path.to_string_lossy().to_string() }));
            tracing::debug!("opening git manager window id={}", wid);
        }

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point { x: px(160.0), y: px(120.0) },
                    size: size(px(1280.0), px(800.0)),
                })),
                titlebar: None,
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(800.),
                    height: px(500.),
                }),
                ..Default::default()
            },
            move |window, cx| create_git_manager_component(window, cx, project_path),
        );
    }

    pub(super) fn toggle_multiplayer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ec) = EngineContext::global() {
            let wid = ec.next_window_id();
            ec.register_window(wid, WindowContext::new(wid, WindowRequest::Multiplayer));
            tracing::debug!("opening multiplayer window id={}", wid);
        }

        let project_path = self.state.project_path.clone();

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(200.0),
                        y: px(200.0),
                    },
                    size: size(px(500.0), px(600.0)),
                })),
                titlebar: None,
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(400.),
                    height: px(500.),
                }),
                ..Default::default()
            },
            move |window, cx| {
                let multiplayer_window = cx.new(|cx| MultiplayerWindow::new(project_path, window, cx));
                cx.new(|cx| Root::new(multiplayer_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_plugin_manager(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ec) = EngineContext::global() {
            let wid = ec.next_window_id();
            ec.register_window(wid, WindowContext::new(wid, WindowRequest::PluginManager));
            tracing::debug!("opening plugin manager window id={}", wid);
        }

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(250.0),
                        y: px(250.0),
                    },
                    size: size(px(600.0), px(500.0)),
                })),
                titlebar: None,
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(450.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            move |window, cx| {
                // PluginManager is now globally accessible
                let plugin_manager_window = cx.new(|cx| {
                    PluginManagerWindow::new_global(cx)
                });
                cx.new(|cx| Root::new(plugin_manager_window.into(), window, cx))
            },
        );
    }


    pub(super) fn open_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // spawn a dedicated settings window
        if let Some(ec) = EngineContext::global() {
            let wid = ec.next_window_id();
            ec.register_window(wid, WindowContext::new(wid, WindowRequest::Settings));
            tracing::debug!("opening settings window id={}", wid);
        }

        let ec_for_cb = EngineContext::global();
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point { x: px(150.0), y: px(150.0) },
                    size: size(px(700.0), px(500.0)),
                })),
                titlebar: None,
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size { width: px(400.), height: px(300.) }),
                ..Default::default()
            },
            move |window, cx| {
                // engine_context is only used by component, so unwrap is safe
                ui_settings::create_settings_component(window, cx, ec_for_cb.as_ref().unwrap())
            },
        );
    }

    pub(super) fn toggle_flamegraph(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Open flamegraph window with DTrace profiling option
        let _ = ui_flamegraph::FlamegraphWindow::open(cx);
    }
}