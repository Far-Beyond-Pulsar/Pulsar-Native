//! Window Manager crate provides a centralized, command-based
//! system for creating, closing and otherwise manipulating windows.
//!
//! The manager exposes a `WindowManager` global that can be installed
//! into a GPUI application and used by any component or subsystem.

pub mod commands;
pub mod hooks;
pub mod pulsar_window;
pub mod state;
pub mod validation;
pub mod telemetry;
pub mod manager;

pub use commands::{WindowCommand, WindowCommandResult, CreateWindowCommand, CloseWindowCommand};
pub use hooks::{HookContext, HookRegistry, HookType, WindowHook};
pub use pulsar_window::{PulsarWindow, default_window_options};
pub use state::{WindowState, WindowInfo};
pub use validation::{WindowValidator, ValidationRule, WindowError, WindowResult};
pub use telemetry::TelemetrySender;
pub use ui_types_common::window_types::WindowRequest;

pub use manager::WindowManager;