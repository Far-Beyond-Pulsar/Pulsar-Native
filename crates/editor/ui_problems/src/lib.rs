//! Problems drawer and window: diagnostics list, filtering and quick-fix diffs.
//!
//! Owns: the `Diagnostic` / `DiagnosticSeverity` / `Hint` model, the
//! `ProblemsDrawer` view (severity filter, text search, grouping by file,
//! de-duplicating republished sets) and the side-by-side diff alignment used
//! for code-action previews.
//!
//! Entry points: `ProblemsDrawer` (embedded in the editor), `ProblemsWindow`
//! (stand-alone window), `NavigateToDiagnostic` (event emitted on click).
//!
//! Must not depend on: the language-server client, the engine runtime or any
//! other `ui_*` crate beyond `ui_common`. Producers push `Diagnostic`s in; this
//! crate never fetches them itself.

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

pub mod components;
mod handlers;
mod screen;
pub mod utils;
pub mod window;

// Re-export main types
pub use screen::ProblemsDrawer;
pub use utils::{Diagnostic, DiagnosticSeverity, Hint, NavigateToDiagnostic};
pub use window::ProblemsWindow;

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
