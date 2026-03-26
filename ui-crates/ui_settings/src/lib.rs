//! Settings UI
//!
//! Application and project settings

pub mod settings;
pub mod settings_v2;
pub mod window;

// Re-export main types
pub use window::SettingsWindow;
pub use settings::{SettingsScreen, SettingsScreenProps};
pub use settings_v2::{SettingsScreenV2, SettingsScreenV2Props, SettingsTab};
