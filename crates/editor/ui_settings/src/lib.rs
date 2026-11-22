//! The settings windows over the settings database: Project Settings (the
//! open project's settings, the engine's and every enabled plugin's) and
//! Editor Settings (the editor's own, for every project; plugins cannot add
//! to it). Both are one `SettingsScreen`: a searchable navigation tree and a
//! virtualized page of sections.

pub mod components;
mod handlers;
pub mod utils;
pub mod window;

pub use components::SettingsScreen;
pub use utils::model::WindowKind;
pub use window::{EditorSettingsWindow, ProjectSettingsWindow, SettingsParams};
