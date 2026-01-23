//! File Manager UI
//!
//! File browser and management

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

pub mod drawer;
mod file_manager_drawer;
pub mod window;

// Re-export main types
pub use file_manager_drawer::FileManagerDrawer;
pub use drawer::{FileSelected, PopoutFileManagerEvent};
pub use window::FileManagerWindow;

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
