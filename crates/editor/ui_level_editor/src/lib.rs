//! Level Editor UI
//!
//! 3D scene editing and level design

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

// Force-link crates that register engine classes via inventory::submit!.
// Without an explicit symbol reference the linker can dead-strip these
// crates before inventory collects their EngineClass registrations.
use helio_component as _;
use pulsar_physics as _;

use gpui::AppContext;

mod level_editor;
pub use level_editor::ai::sessions as ai_sessions;
pub use level_editor::ai::tools as ai_tools;
pub use level_editor::{LevelEditorPanel, LevelEditorState, SceneDatabase, SceneObjectData};

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
