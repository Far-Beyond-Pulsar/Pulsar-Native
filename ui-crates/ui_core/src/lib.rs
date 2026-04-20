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

/// Initialize ui_core: register global action handlers for application menu actions.
///
/// Must be called once from the `gpui_app.run` callback (alongside `ui::init`).
/// Global `cx.on_action` handlers fire regardless of focus or render-tree position,
/// which is necessary because popup menus render in a `deferred` layer that is
/// disconnected from the `PulsarApp` dispatch tree on Windows / Linux.
///
/// Each handler opens the corresponding window directly via `open_pulsar_window`,
/// the same path used everywhere else in the codebase — fully decoupled from
/// `PulsarApp` or any particular window hierarchy.
pub fn init(cx: &mut gpui::App) {
    use ui_common::open_pulsar_window;
    use ui_common::menu::{Settings, Preferences, AboutApp, ShowDocumentation};
    use ui_settings::SettingsWindow;
    use ui_about::AboutWindow;
    use ui_documentation::DocumentationWindow;

    cx.on_action(|_: &Settings, cx| {
        println!("[MENU] global: Settings → SettingsWindow");
        open_pulsar_window::<SettingsWindow>((), cx);
    });
    cx.on_action(|_: &Preferences, cx| {
        println!("[MENU] global: Preferences → SettingsWindow");
        open_pulsar_window::<SettingsWindow>((), cx);
    });
    cx.on_action(|_: &AboutApp, cx| {
        open_pulsar_window::<AboutWindow>((), cx);
    });
    cx.on_action(|_: &ShowDocumentation, cx| {
        open_pulsar_window::<DocumentationWindow>((), cx);
    });
}


    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}

// Re-export builtin editor registration
pub use builtin_editors::register_all_builtin_editors;
