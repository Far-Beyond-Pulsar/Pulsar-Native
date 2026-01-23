//! Type Debugger UI
//!
//! Runtime type database inspection and debugging

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

mod type_debugger_drawer;
pub mod window;

// Re-export main types
pub use type_debugger_drawer::{TypeDebuggerDrawer, NavigateToType};
pub use window::TypeDebuggerWindow;

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
