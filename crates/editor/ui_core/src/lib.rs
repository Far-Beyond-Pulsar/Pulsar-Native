//! Core UI Application
//!
//! Core application components including PulsarApp and PulsarRoot

// Force-link window crates so their `inventory::submit!` registrants
// are included in the binary and discovered by `register_all_windows`.
use ui_about as _;
use ui_documentation as _;
use ui_fab_search as _;
use ui_flamegraph as _;
use ui_git_manager as _;
use ui_multiplayer as _;
use ui_plugin_manager as _;
use ui_settings as _;

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

/// Initialize ui_core: populate the [`WindowRegistry`] and wire GPUI menu actions to it.
///
/// Must be called once from the `gpui_app.run` callback (alongside `ui::init`).
/// Global `cx.on_action` handlers fire regardless of focus, so popup menus always
/// reach their target even when focus is in a disconnected render layer.
///
/// Adding a new menu-triggered window: call `MyWindow::register(cx)` here and add one
/// `cx.on_action` line. Nothing else needs to change.
/// Register GPUI menu actions through the [`WindowRegistry`].
///
/// Window crates call their own `init(cx)` to self-register before this runs.
/// `ui_core` does not import any window crate here — it only maps existing GPUI
/// actions to registry name lookups.
pub fn init(cx: &mut gpui::App) {
    use gpui::UpdateGlobal as _;
    use ui_common::menu::{AboutApp, Preferences, Settings, ShowDocumentation};

    root::register_window_wrappers(cx);

    // Register global keybindings for actions that must work anywhere in the app
    cx.bind_keys([
        gpui::KeyBinding::new::<ToggleCommandPalette>("alt-space", ToggleCommandPalette {}, None),
        gpui::KeyBinding::new::<ToggleFileManager>("ctrl-space", ToggleFileManager {}, None),
    ]);

    // File-browser shortcuts (Ctrl/Cmd + C/X/V/A), scoped to the file manager focus.
    ui_file_manager::init(cx);

    cx.on_action(|_: &Settings, cx| {
        tracing::debug!("[MENU] Settings");
        window_manager::WindowRegistry::update_global(cx, |reg, cx| reg.open("SettingsWindow", cx));
    });
    cx.on_action(|_: &Preferences, cx| {
        tracing::debug!("[MENU] Preferences");
        window_manager::WindowRegistry::update_global(cx, |reg, cx| reg.open("SettingsWindow", cx));
    });
    cx.on_action(|_: &AboutApp, cx| {
        window_manager::WindowRegistry::update_global(cx, |reg, cx| reg.open("AboutWindow", cx));
    });
    cx.on_action(|_: &ShowDocumentation, cx| {
        window_manager::WindowRegistry::update_global(cx, |reg, cx| {
            reg.open("DocumentationWindow", cx)
        });
    });
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}

// Re-export builtin editor registration
pub use builtin_editors::register_all_builtin_editors;
