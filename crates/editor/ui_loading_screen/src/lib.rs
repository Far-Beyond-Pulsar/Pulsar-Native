//! Loading screen — runs background tasks, shows progress, then opens the editor.

mod preload;
mod recent_projects;
mod screen;
mod tasks;

use gpui::AppContext;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use preload::{take_preloaded_files, PreloadedFileEntry};
pub use screen::LoadingScreen;

fn prepare_project_filesystem(path: &Path) {
    if !engine_fs::is_cloud_path(path) {
        engine_fs::virtual_fs::reset_to_local();
    }
}

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
        prepare_project_filesystem(&path);
        cx.new(|cx| LoadingScreen::new_with_on_complete(path, on_complete, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_fs::{RemoteConfig, RemoteFsProvider};

    struct ProviderGuard(Arc<dyn engine_fs::FsProvider>);

    impl Drop for ProviderGuard {
        fn drop(&mut self) {
            engine_fs::virtual_fs::set_provider(self.0.clone());
        }
    }

    #[test]
    fn local_project_restores_local_provider_after_cloud_project() {
        let _guard = ProviderGuard(engine_fs::virtual_fs::provider());
        let remote = RemoteFsProvider::new(RemoteConfig {
            server_url: "http://127.0.0.1".to_string(),
            workspace_id: "workspace".to_string(),
            environment_id: "environment".to_string(),
            auth_token: None,
        });
        engine_fs::virtual_fs::set_provider(Arc::new(remote));

        prepare_project_filesystem(Path::new("cloud+pulsar://127.0.0.1/workspace/environment"));
        assert!(engine_fs::virtual_fs::is_remote());

        prepare_project_filesystem(Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(!engine_fs::virtual_fs::is_remote());
    }
}
