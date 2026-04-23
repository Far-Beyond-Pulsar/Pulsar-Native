//! Window Manager crate provides a centralized, command-based
//! system for creating, closing and otherwise manipulating windows.
//!
//! The manager exposes a `WindowManager` global that can be installed
//! into a GPUI application and used by any component or subsystem.

pub mod commands;
pub mod hooks;
pub mod manager;
pub mod pulsar_window;
pub mod state;
pub mod telemetry;
pub mod validation;

pub use commands::{CloseWindowCommand, CreateWindowCommand, WindowCommand, WindowCommandResult};
pub use hooks::{HookContext, HookRegistry, HookType, WindowHook};
pub use pulsar_window::{default_window_options, PulsarWindow};
pub use state::{WindowInfo, WindowState};
pub use telemetry::TelemetrySender;
pub use ui_types_common::window_types::WindowRequest;
pub use validation::{ValidationRule, WindowError, WindowResult, WindowValidator};

pub use manager::WindowManager;
