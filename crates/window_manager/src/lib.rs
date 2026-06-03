//! Window Manager crate provides a centralized, command-based
//! system for creating, closing and otherwise manipulating windows.
//!
//! The manager exposes a `WindowManager` global that can be installed
//! into a GPUI application and used by any component or subsystem.

pub mod commands;
pub mod configs;
pub mod hooks;
pub mod manager;
pub mod pulsar_window;
pub mod registry;
pub mod state;
pub mod telemetry;
pub mod validation;

pub use commands::{CloseWindowCommand, CreateWindowCommand, WindowCommand, WindowCommandResult};
pub use configs::WindowConfig;
pub use hooks::{HookContext, HookRegistry, HookType, WindowHook};
pub use pulsar_window::{default_window_options, PulsarWindow};
pub use state::{WindowInfo, WindowState};
pub use telemetry::TelemetrySender;
pub use ui_types_common::window_types::WindowRequest;
pub use validation::{ValidationRule, WindowError, WindowResult, WindowValidator};

pub use manager::WindowManager;
pub use registry::{WindowRegistrant, WindowRegistry};
pub use ui_gen_macros::register_window;

/// Call once after [`WindowManager`] and [`WindowRegistry`] globals are installed.
/// Runs every [`WindowRegistrant`] submitted via `inventory::submit!` across all
/// linked crates — no per-crate `init()` calls needed in `main.rs`.
pub fn register_all_windows(cx: &mut gpui::App) {
    for registrant in inventory::iter::<WindowRegistrant>() {
        (registrant.register)(cx);
    }
}
