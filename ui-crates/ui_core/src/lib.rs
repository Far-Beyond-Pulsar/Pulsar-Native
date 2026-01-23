//! Core UI Application
//!
//! Core application components including PulsarApp and PulsarRoot

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

// Modules
pub mod app;
pub mod actions;
pub mod root;
pub mod builtin_editors;

// Re-export main types
pub use app::PulsarApp;
pub use root::PulsarRoot;

// Re-export actions
pub use actions::{
    ToggleCommandPalette,
    ToggleFileManager,
    ToggleProblems,
    ToggleMultiplayer,
    OpenFile,
};

// Re-export file_utils from ui_common
pub use ui_common::file_utils;

// Re-export actions from ui crate
pub use ui::OpenSettings;

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}

// Re-export builtin editor registration
pub use builtin_editors::register_all_builtin_editors;
