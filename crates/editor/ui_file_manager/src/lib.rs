rust_i18n::i18n!("locales", fallback = "en");

mod screen;
pub mod components;
pub mod configurator;
mod handlers;
pub mod preload;
pub mod utils;

pub use components::FileManagerDrawer;
pub use preload::{store_preloaded_tree, take_preloaded_tree};
pub use screen::FileManagerWindow;
pub use utils::{FileSelected, FolderNode, PopoutFileManagerEvent};

/// Register the file-browser keyboard shortcuts. Scoped to the `FileManagerDrawer`
/// key context so they only fire while the file browser is focused (and don't
/// shadow global copy/paste elsewhere). Call once during app init.
pub fn init(cx: &mut gpui::App) {
    use crate::utils::actions::{Copy, Cut, Paste, SelectAll};
    const CTX: Option<&str> = Some("FileManagerDrawer");
    cx.bind_keys([
        gpui::KeyBinding::new("ctrl-c", Copy, CTX),
        gpui::KeyBinding::new("cmd-c", Copy, CTX),
        gpui::KeyBinding::new("ctrl-x", Cut, CTX),
        gpui::KeyBinding::new("cmd-x", Cut, CTX),
        gpui::KeyBinding::new("ctrl-v", Paste, CTX),
        gpui::KeyBinding::new("cmd-v", Paste, CTX),
        gpui::KeyBinding::new("ctrl-a", SelectAll, CTX),
        gpui::KeyBinding::new("cmd-a", SelectAll, CTX),
    ]);
}

pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
