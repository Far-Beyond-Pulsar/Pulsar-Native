//! Common UI Utilities
//!
//! Shared helpers and utilities used across all UI components

// Initialize translations for ui_common (menus, titlebar, etc.)
rust_i18n::i18n!("locales", fallback = "en");

/// Translate a key to the current locale
#[inline]
pub fn translate(key: &str) -> String {
    rust_i18n::t!(key).into_owned()
}

/// Get the current locale
#[inline]
pub fn locale() -> impl std::ops::Deref<Target = str> {
    rust_i18n::locale()
}

/// Set the current locale
#[inline]
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale)
}

pub mod command_palette;
pub mod file_utils;
pub mod generic_window;
pub mod helpers;
pub mod menu;
pub mod open_window;
pub mod panel;
pub mod shared;
pub mod shared_state;

pub use open_window::open_pulsar_window;

pub use shared_state::SharedState;

// Re-export commonly used types
pub use file_utils::{find_openable_files, FileInfo, FileType};
pub use menu::AppTitleBar;
pub use panel::{PanelBase, PanelEvent};
pub use shared::{PropertyField, StatusBar, Toolbar, ToolbarButton, ViewportControls};

// Re-export diagnostics from ui crate
pub use ui::diagnostics::{CodeAction, Diagnostic, DiagnosticSeverity, TextEdit};
