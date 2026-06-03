//! Named window configuration presets.
//!
//! Use these instead of hand-building WindowOptions at each call site.

use gpui::{
    px, size, Bounds, Point, Size, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowIcon, WindowKind, WindowOptions,
};

use crate::wrappers::WindowContentWrapper;

#[cfg(target_os = "macos")]
static ICON_PNG: &[u8] = include_bytes!("../../../assets/images/logo_sqrkl_mac.png");

#[cfg(not(target_os = "macos"))]
static ICON_PNG: &[u8] = include_bytes!("../../../assets/images/logo_sqrkl.png");

fn app_icon() -> Option<WindowIcon> {
    WindowIcon::from_png_bytes(ICON_PNG)
        .map_err(|e| tracing::warn!("Failed to decode app icon: {e}"))
        .ok()
}

fn base(ox: f32, oy: f32, w: f32, h: f32, min_w: f32, min_h: f32) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point {
                x: px(ox),
                y: px(oy),
            },
            size: Size {
                width: px(w),
                height: px(h),
            },
        })),
        titlebar: None,
        kind: WindowKind::Normal,
        is_resizable: true,
        window_decorations: Some(WindowDecorations::Client),
        window_min_size: Some(Size {
            width: px(min_w),
            height: px(min_h),
        }),
        window_background: WindowBackgroundAppearance::Opaque,
        app_icon: app_icon(),
        ..Default::default()
    }
}

/// Named window configuration presets.
pub struct WindowConfig;

/// A complete window profile: native options plus optional UI wrapper intent.
pub struct WindowProfile {
    options: WindowOptions,
    wrapper: WindowContentWrapper,
}

impl WindowProfile {
    pub fn new(options: WindowOptions, wrapper: WindowContentWrapper) -> Self {
        Self { options, wrapper }
    }

    pub fn options(self) -> WindowOptions {
        self.options
    }

    pub fn wrapper(&self) -> WindowContentWrapper {
        self.wrapper
    }
}

impl WindowConfig {
    /// Full editor window — 1600×900, client decorations.
    pub fn editor() -> WindowOptions {
        Self::editor_profile().options()
    }

    /// Full editor window profile. Requests the editor UI wrapper.
    pub fn editor_profile() -> WindowProfile {
        WindowProfile::new(
            base(50.0, 50.0, 1600.0, 900.0, 800.0, 600.0),
            WindowContentWrapper::Editor,
        )
    }

    /// Entry / project-selection window — 1100×700.
    pub fn entry() -> WindowOptions {
        Self::entry_profile().options()
    }

    /// Entry / project-selection profile.
    pub fn entry_profile() -> WindowProfile {
        WindowProfile::new(
            base(100.0, 100.0, 1100.0, 700.0, 800.0, 500.0),
            WindowContentWrapper::None,
        )
    }

    /// General-purpose dialog: settings, about, docs, etc.
    /// `width` and `height` are logical pixels; min size is half of each.
    pub fn dialog(width: f32, height: f32) -> WindowOptions {
        Self::dialog_profile(width, height).options()
    }

    /// General-purpose dialog profile.
    pub fn dialog_profile(width: f32, height: f32) -> WindowProfile {
        WindowProfile::new(
            base(100.0, 100.0, width, height, width * 0.5, height * 0.5),
            WindowContentWrapper::None,
        )
    }

    /// Panel popped out of the dock. Positioned near the given cursor.
    pub fn detached_panel(cursor: gpui::Point<gpui::Pixels>) -> WindowOptions {
        Self::detached_panel_profile(cursor).options()
    }

    /// Detached panel profile.
    pub fn detached_panel_profile(cursor: gpui::Point<gpui::Pixels>) -> WindowProfile {
        WindowProfile::new(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Point {
                        x: cursor.x - px(100.0),
                        y: cursor.y - px(30.0),
                    },
                    size(px(800.0), px(600.0)),
                ))),
                titlebar: None,
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(WindowDecorations::Client),
                window_min_size: Some(Size {
                    width: px(400.0),
                    height: px(300.0),
                }),
                window_background: WindowBackgroundAppearance::Opaque,
                app_icon: app_icon(),
                ..Default::default()
            },
            WindowContentWrapper::None,
        )
    }
}
