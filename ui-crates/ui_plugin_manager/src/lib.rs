//! Plugin Manager UI
//!
//! A popup window for viewing and managing loaded plugins

mod window;

pub use window::PluginManagerWindow;

pub fn init(cx: &mut gpui::App) {
    use ui_common::PulsarWindowExt as _;
    PluginManagerWindow::register(cx);
}
