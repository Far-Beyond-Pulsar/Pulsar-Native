rust_i18n::i18n!("locales", fallback = "en");

mod screen;
pub mod components;
mod handlers;
pub mod preload;
pub mod utils;

pub use components::FileManagerDrawer;
pub use preload::{store_preloaded_tree, take_preloaded_tree};
pub use screen::FileManagerWindow;
pub use utils::{FileSelected, FolderNode, PopoutFileManagerEvent};

pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
