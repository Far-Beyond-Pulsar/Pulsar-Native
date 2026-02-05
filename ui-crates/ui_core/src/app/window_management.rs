//! Window management and creation logic

use std::sync::Arc;
use gpui::{px, size, Bounds, Context, Point, Window, WindowBounds, WindowKind, WindowOptions};
use gpui::AppContext;
use ui::Root;
use ui_problems::ProblemsWindow;
use ui_flamegraph::{FlamegraphWindow, TraceData};
use ui_type_debugger::TypeDebuggerWindow;
use ui_multiplayer::MultiplayerWindow;
use ui_plugin_manager::PluginManagerWindow;

use super::PulsarApp;
use super::panel_window::PanelWindow;

impl PulsarApp {
    /// Create a detached window with a panel in a dedicated popup window
    pub(super) fn create_detached_window(
        &self,
        panel: Arc<dyn ui::dock::PanelView>,
        position: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        println!("[POPOUT] Creating detached window for panel at position: {:?}", position);

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
            titlebar: Some(gpui::TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: None,
            }),
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

        println!("[POPOUT] Opening window with options");

        // Create a dedicated panel window instead of embedding in full PulsarApp
        let result = cx.open_window(window_options, move |window, cx| {
            println!("[POPOUT] Inside window creation callback");
            let panel_window = cx.new(|cx| PanelWindow::new(panel, window, cx));
            println!("[POPOUT] PanelWindow created successfully");
            cx.new(|cx| Root::new(panel_window.into(), window, cx))
        });

        match result {
            Ok(_) => println!("[POPOUT] Window opened successfully"),
            Err(e) => println!("[POPOUT] Failed to open window: {:?}", e),
        }
    }

    pub(super) fn toggle_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.drawer_open = !self.state.drawer_open;
        cx.notify();
    }

    pub(super) fn toggle_problems(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
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
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
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
                let problems_window = cx.new(|cx| ProblemsWindow::new(problems_drawer, window, cx));
                cx.new(|cx| Root::new(problems_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_type_debugger(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
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
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
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
                let type_debugger_window = cx.new(|cx| TypeDebuggerWindow::new(type_debugger_drawer, window, cx));
                cx.new(|cx| Root::new(type_debugger_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_multiplayer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
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
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
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
        let plugin_manager_ptr = &mut self.state.plugin_manager as *mut plugin_manager::PluginManager;

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(250.0),
                        y: px(250.0),
                    },
                    size: size(px(600.0), px(500.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
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
                let plugin_manager_window = cx.new(|cx| unsafe {
                    PluginManagerWindow::new(&mut *plugin_manager_ptr, window, cx)
                });
                cx.new(|cx| Root::new(plugin_manager_window.into(), window, cx))
            },
        );
    }

    pub(super) fn toggle_flamegraph(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Open flamegraph window with DTrace profiling option
        let _ = ui_flamegraph::FlamegraphWindow::open(cx);
    }
}