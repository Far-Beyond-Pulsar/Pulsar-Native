//! Window Manager crate provides a centralized, command-based
//! system for creating, closing and otherwise manipulating windows.
//!
//! The manager exposes a `WindowManager` global that can be installed
//! into a GPUI application and used by any component or subsystem.

pub mod commands;
pub mod hooks;
pub mod state;
pub mod validation;
pub mod telemetry;

pub use commands::{WindowCommand, WindowCommandResult, CreateWindowCommand, CloseWindowCommand};
pub use hooks::{HookContext, HookRegistry, HookType, WindowHook};
pub use state::{WindowState, WindowInfo};
pub use validation::{WindowValidator, ValidationRule, WindowError, WindowResult};
pub use telemetry::TelemetrySender;

use engine_state::WindowId;
use gpui::{App, AnyView, AnyWindowHandle, Window, WindowOptions};

/// Primary orchestration object. Implements `gpui::Global` and should be
/// installed via `cx.set_global` early during application startup.

pub struct WindowManager {
    // defined in manager.rs
}

// re-export internal parts for convenience
pub use crate::manager::WindowManager;

// Additional helper types could be added here.
