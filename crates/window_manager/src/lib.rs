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
pub use registry::{WindowRegistry, WINDOW_REGISTRANTS};
pub use ui_gen_macros::register_window;

/// Call once after [`WindowManager`] and [`WindowRegistry`] globals are installed.
/// Iterates every element contributed to [`WINDOW_REGISTRANTS`] via
/// `#[window_manager::register_window]` — no per-crate `init()` calls needed.
pub fn register_all_windows(cx: &mut gpui::App) {
    tracing::info!(
        "[WindowManager] register_all_windows: {} registrants",
        WINDOW_REGISTRANTS.len()
    );
    for f in WINDOW_REGISTRANTS.iter() {
        f(cx);
    }
}
