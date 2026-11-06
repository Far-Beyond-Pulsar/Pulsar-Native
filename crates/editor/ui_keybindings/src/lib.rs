//! The keyboard shortcuts window and the per-project keybinding overrides.
//!
//! See `.agents/KEYBINDINGS.md`.

mod components;
mod handlers;
mod screen;
pub mod utils;

pub use screen::KeybindingsWindow;
pub use utils::store::apply_project_keybindings;

#[cfg(test)]
mod tests;
