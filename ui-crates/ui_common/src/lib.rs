//! Common UI Utilities
//!
//! Shared helpers and utilities used across all UI components

// DO NOT initialize rust_i18n here - use translations from the ui crate instead
// The ui crate is the central translation repository

pub mod command_palette;
pub mod file_utils;
pub mod helpers;
pub mod menu;
pub mod shared;

// Re-export commonly used types
pub use menu::AppTitleBar;
pub use file_utils::{FileInfo, FileType, find_openable_files};
pub use shared::{StatusBar, ViewportControls, Toolbar, ToolbarButton, PropertyField};

// Re-export diagnostics from ui crate
pub use ui::diagnostics::{Diagnostic, DiagnosticSeverity, TextEdit, CodeAction};