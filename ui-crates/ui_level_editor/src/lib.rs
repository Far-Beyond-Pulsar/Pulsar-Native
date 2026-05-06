//! Level Editor UI
//!
//! 3D scene editing and level design

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

// Force-link crates that register engine classes via inventory::submit!.
// Without an explicit symbol reference the linker can dead-strip these
// crates before inventory collects their EngineClass registrations.
extern crate pulsar_physics;
extern crate pulsar_rendering;

use gpui::AppContext;

mod level_editor;

// Re-export main types
pub use level_editor::{LevelEditorPanel, SceneDatabase};

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}

impl window_manager::PulsarWindow for LevelEditorPanel {
    type Params = ();

    fn window_name() -> &'static str {
        "LevelEditorPanel"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1600.0, 900.0)
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        cx.new(|cx| LevelEditorPanel::new(window, cx))
    }
}
