//! Type Debugger UI
//!
//! Runtime type database inspection and debugging

rust_i18n::i18n!("locales", fallback = "en");

mod handlers;
mod screen;
pub mod components;
pub mod utils;
pub mod window;

pub use screen::TypeDebuggerDrawer;
pub use utils::{NavigateToType, FilterAll, FilterAliases, FilterStructs, FilterEnums, FilterTraits};
pub use window::TypeDebuggerWindow;

pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
