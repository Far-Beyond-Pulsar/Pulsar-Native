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

inventory::submit! {
    window_manager::WindowRegistrant { register: |cx| {
        use ui_common::PulsarWindowExt as _;
        SettingsWindow::register(cx);
    }}
}
