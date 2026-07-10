//! Loading screen — runs background tasks, shows progress, then opens the editor.

mod preload;
mod recent_projects;
mod screen;
mod tasks;

use gpui::AppContext;
use std::path::PathBuf;
use std::sync::Arc;

pub use preload::{take_preloaded_files, PreloadedFileEntry};
pub use screen::LoadingScreen;

impl window_manager::PulsarWindow for LoadingScreen {
    type Params = (PathBuf, Arc<dyn Fn(PathBuf, &mut gpui::App) + Send + Sync>);

    fn window_name() -> &'static str {
        "LoadingScreen"
    }

    fn window_options(_: &Self::Params) -> gpui::WindowOptions {
        use gpui::{
            px, Bounds, Point, Size, WindowBounds, WindowDecorations, WindowIcon, WindowKind,
        };
        #[cfg(not(target_os = "macos"))]
        static ICON_PNG: &[u8] = include_bytes!("../../../../assets/images/logo_sqrkl.png");
        #[cfg(target_os = "macos")]
        static ICON_PNG: &[u8] = include_bytes!("../../../../assets/images/logo_sqrkl_mac.png");
        let app_icon = WindowIcon::from_png_bytes(ICON_PNG)
            .map_err(|e| tracing::warn!("Failed to decode app icon: {e}"))
            .ok();
        gpui::WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point {
                    x: px(200.0),
                    y: px(150.0),
                },
                size: Size {
                    width: px(960.0),
                    height: px(540.0),
                },
            })),
            titlebar: None,
            kind: WindowKind::Normal,
            is_resizable: false,
            window_decorations: Some(WindowDecorations::Client),
            window_min_size: None,
            app_icon,
            window_background: gpui::WindowBackgroundAppearance::Opaque,
            ..Default::default()
        }
    }

    fn build(
        params: Self::Params,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> gpui::Entity<Self> {
        let (path, on_complete) = params;
        cx.new(|cx| LoadingScreen::new_with_on_complete(path, on_complete, window, cx))
    }
}
