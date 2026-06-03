//! Settings UI
//!
//! Application and project settings

pub mod settings;
pub mod settings_modern;
pub mod window;

// Re-export main types
pub use settings::{SettingsScreen, SettingsScreenProps};
pub use settings_modern::ModernSettingsScreen;
pub use window::SettingsWindow;

/// Register `SettingsWindow` in the global [`WindowRegistry`].
/// Call from `main.rs` after the registry global is set.
pub fn init(cx: &mut gpui::App) {
    use ui_common::PulsarWindowExt as _;
    SettingsWindow::register(cx);
}
