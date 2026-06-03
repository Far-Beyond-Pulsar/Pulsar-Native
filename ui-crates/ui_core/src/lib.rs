//! Core UI Application
//!
//! Core application components including PulsarApp and PulsarRoot

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

// Modules
pub mod actions;
pub mod app;
pub mod builtin_editors;
pub mod custom_providers;
pub mod project_switcher;
pub mod root;

// Re-export main types
pub use app::PulsarApp;
pub use root::PulsarRoot;

// Re-export actions
pub use actions::{
    ActivateOpenEditor, OpenFile, ToggleAgentChat, ToggleCommandPalette, ToggleFileManager,
    ToggleMultiplayer, ToggleProblems,
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
/// Each handler opens the corresponding window via the `PulsarWindowExt::open` method,
/// fully decoupled from `PulsarApp` or any particular window hierarchy.
pub fn init(cx: &mut gpui::App) {
    use ui_about::AboutWindow;
    use ui_common::menu::{AboutApp, Preferences, Settings, ShowDocumentation};
    use ui_common::PulsarWindowExt as _;
    use ui_documentation::DocumentationWindow;
    use ui_settings::SettingsWindow;

    cx.on_action(|_: &Settings, cx| {
        tracing::debug!("[MENU] global: Settings → SettingsWindow");
        SettingsWindow::open((), cx);
    });
    cx.on_action(|_: &Preferences, cx| {
        tracing::debug!("[MENU] global: Preferences → SettingsWindow");
        SettingsWindow::open((), cx);
    });
    cx.on_action(|_: &AboutApp, cx| {
        AboutWindow::open((), cx);
    });
    cx.on_action(|_: &ShowDocumentation, cx| {
        DocumentationWindow::open((), cx);
    });
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}

// Re-export builtin editor registration
pub use builtin_editors::register_all_builtin_editors;
