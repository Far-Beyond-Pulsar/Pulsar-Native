//! Problems UI
//!
//! Diagnostics and error display

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

mod problems_drawer;
pub mod window;

// Re-export main types
pub use problems_drawer::{ProblemsDrawer, NavigateToDiagnostic, Diagnostic, DiagnosticSeverity, Hint};
pub use window::ProblemsWindow;

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
