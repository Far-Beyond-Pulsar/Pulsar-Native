//! Main Editor Window Module
//!
//! This module contains the main editor window and its associated components:
//!
//! - `tabs/` - All editor tab types (script, level, blueprint, DAW, specialized)
//! - `drawers/` - Collapsible side panels (file manager, terminal, problems)

pub mod drawers;
pub mod tabs;

// Re-export tabs for convenience
pub use tabs::{
    BlueprintEditorPanel, DawEditorPanel, LevelEditorPanel, ScriptEditorPanel, TextEditorEvent,
};

// Re-export drawers for convenience
pub use drawers::{FileManagerDrawer, ProblemsDrawer, TerminalDrawer};
