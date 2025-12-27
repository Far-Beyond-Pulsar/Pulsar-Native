//! Core UI Application
//!
//! Core application components including PulsarApp and PulsarRoot

// Modules
pub mod app;
pub mod flow;
pub mod unified_palette;
pub mod types;
pub mod actions;
pub mod root;
pub mod editor_panel;

// Re-export main types
pub use app::PulsarApp;
pub use root::PulsarRoot;
pub use types::EditorType;
pub use editor_panel::EditorPanel;

// Re-export actions
pub use actions::{
    ToggleCommandPalette,
    ToggleFileManager,
    ToggleProblems,
    ToggleTerminal,
    ToggleMultiplayer,
};

// Re-export flow
pub use flow::AppState;

// Re-export palette types
pub use unified_palette::{AnyPaletteDelegate, AnyPaletteItem};

// Re-export file_utils from ui_common
pub use ui_common::file_utils;

// Re-export actions from ui crate
pub use ui::OpenSettings;
