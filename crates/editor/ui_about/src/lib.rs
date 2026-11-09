//! About window: logo, version/credits and links to the project and docs.
//!
//! Owns: `AboutWindow` and its sections (logo, info, feature cards) and the
//! click handlers that open the repository and documentation URLs.
//!
//! Entry points: `create_about_window` and `AboutWindow`.
//!
//! Must not depend on: engine runtime state or any other editor feature crate.
//! It is a static, read-only window; strings come from `ui_common::translate`.
pub mod components;
mod handlers;
mod screen;
mod utils;

pub use screen::{AboutWindow, create_about_window};
