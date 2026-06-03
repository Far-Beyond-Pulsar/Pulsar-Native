//! Plugin Manager UI
//!
//! A popup window for viewing and managing loaded plugins

mod window;

pub use window::PluginManagerWindow;

inventory::submit! {
    window_manager::WindowRegistrant { register: |cx| {
        use ui_common::PulsarWindowExt as _;
        PluginManagerWindow::register(cx);
    }}
}
