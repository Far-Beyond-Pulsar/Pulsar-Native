//! Level Editor UI
//!
//! 3D scene editing and level design

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

mod level_editor;

// Re-export main types
pub use level_editor::{
    LevelEditorPanel,
    SceneDatabase,
    GizmoState,
    GizmoType,
};

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
