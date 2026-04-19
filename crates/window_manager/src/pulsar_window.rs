//! The PulsarWindow trait — implement this once to define a top-level window.
//!
//! No central dispatch table, no WindowRequest enum variants, no verbose WindowOptions
//! repetition. Each crate declares its own window with max customization, min boilerplate.

use gpui::{App, Entity, Render, Window, WindowOptions};

/// Implement this trait for any type you want to open as a top-level window.
///
/// # Minimal example (zero-config window)
/// ```ignore
/// impl PulsarWindow for SettingsWindow {
///     type Params = ();
///     fn window_name() -> &'static str { "SettingsWindow" }
///     fn window_options(_: &()) -> WindowOptions { default_window_options(700.0, 500.0) }
///     fn build(_: (), _window: &mut Window, cx: &mut App) -> Entity<Self> {
///         cx.new(|cx| SettingsWindow::new(cx))
///     }
/// }
/// ```
///
/// # Drawer window example (with inner entity)
/// ```ignore
/// impl PulsarWindow for ProblemsWindow {
///     type Params = Entity<ProblemsDrawer>;
///     fn window_name() -> &'static str { "ProblemsWindow" }
///     fn window_options(_: &Entity<ProblemsDrawer>) -> WindowOptions {
///         default_window_options(900.0, 600.0)
///     }
///     fn build(drawer: Entity<ProblemsDrawer>, _window: &mut Window, cx: &mut App) -> Entity<Self> {
///         cx.new(|cx| Self::new(drawer, cx))
///     }
/// }
/// ```
pub trait PulsarWindow: Render + Sized + 'static {
    /// Data passed to the window when it opens. Use `()` for zero-param windows.
    type Params: Send + 'static;

    /// Telemetry identifier (e.g. `"ProblemsWindow"`). Should be unique per window type.
    fn window_name() -> &'static str;

    /// Window size, position, and chrome options.
    /// Override to customise; call `default_window_options(width, height)` for defaults.
    fn window_options(params: &Self::Params) -> WindowOptions;

    /// Build and return the root entity for this window.
    /// Called inside the GPUI open_window callback; wrapped in `Root` automatically.
    fn build(params: Self::Params, window: &mut Window, cx: &mut App) -> Entity<Self>;
}

/// Convenience constructor for standard client-decorated windows.
/// `width` and `height` are logical pixels; min size is half of each.
pub fn default_window_options(width: f32, height: f32) -> WindowOptions {
    use gpui::{Bounds, Point, Size, WindowBounds, WindowDecorations, WindowIcon, WindowKind, px};

    // Embed the Pulsar icon at compile time so it is always available, even
    // when running outside an app bundle.
    #[cfg(target_os = "macos")]
    static ICON_PNG: &[u8] = include_bytes!("../../../assets/images/logo_sqrkl_mac.png");

    #[cfg(not(target_os = "macos"))]
    static ICON_PNG: &[u8] = include_bytes!("../../../assets/images/logo_sqrkl.png");

    let app_icon = WindowIcon::from_png_bytes(ICON_PNG)
        .map_err(|e| tracing::warn!("Failed to decode app icon: {e}"))
        .ok();

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point { x: px(100.0), y: px(100.0) },
            size: Size { width: px(width), height: px(height) },
        })),
        titlebar: None,
        kind: WindowKind::Normal,
        is_resizable: true,
        window_decorations: Some(WindowDecorations::Client),
        window_min_size: Some(Size {
            width: px(width * 0.5),
            height: px(height * 0.5),
        }),
        app_icon,
        ..Default::default()
    }
}
