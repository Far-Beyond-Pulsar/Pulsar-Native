//! The settings window (`SettingsWindow`, `ModernSettingsScreen`) over the global and
//! project settings.

pub mod components;
mod handlers;
mod screen;
pub mod utils;
pub mod window;

pub use screen::ModernSettingsScreen;
pub use window::{SettingsParams, SettingsWindow};
