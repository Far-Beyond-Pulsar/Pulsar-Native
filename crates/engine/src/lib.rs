//! Pulsar Engine Library
//!
//! This library provides access to core engine functionality for UI crates and other components.

// Re-export core modules that UI needs
pub mod assets;
pub mod settings;

// Re-export render from backend where it actually lives
pub use engine_backend::subsystems::render;

// Re-export compiler and graph from ui crate (canonical location)
pub use ui::compiler;
pub use ui::graph;

// Re-export themes from ui crate (where it belongs)
pub use ui::themes;

// Re-export engine state
pub use engine_state;

// Re-export Assets type for convenience
pub use assets::Assets;

// Re-export action types that UI needs
pub use gpui::Action;

// Re-export OpenSettings from ui crate
pub use ui::OpenSettings;

// Engine constants
pub const ENGINE_NAME: &str = env!("CARGO_PKG_NAME");
pub const ENGINE_LICENSE: &str = env!("CARGO_PKG_LICENSE");
pub const ENGINE_AUTHORS: &str = env!("CARGO_PKG_AUTHORS");
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const ENGINE_HOMEPAGE: &str = env!("CARGO_PKG_HOMEPAGE");
pub const ENGINE_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const ENGINE_DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");
pub const ENGINE_LICENSE_FILE: &str = env!("CARGO_PKG_LICENSE_FILE");

// Re-export project path functions from engine_state
pub use engine_state::{set_project_path, get_project_path};
