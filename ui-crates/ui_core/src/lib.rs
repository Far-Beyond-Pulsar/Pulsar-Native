//! Core UI Application
//!
//! Core application components including PulsarApp and PulsarRoot

pub mod app;
pub mod flow;
pub mod unified_palette;

// Re-export main app types
pub use app::{PulsarApp, PulsarRoot, ToggleCommandPalette, ToggleFileManager, ToggleProblems, ToggleTerminal};
pub use flow::AppState;
pub use unified_palette::{AnyPaletteDelegate, AnyPaletteItem};

// Re-export file_utils from ui_common
pub use ui_common::file_utils;

// Re-export actions from ui crate
pub use ui::OpenSettings;
