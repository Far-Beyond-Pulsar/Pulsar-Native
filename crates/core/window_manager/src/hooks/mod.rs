//! Window lifecycle hooks: the `WindowHook` trait with its context and hook types, the
//! built-in `LoggingHook` and `TelemetryHook`, and the `HookRegistry`.

pub mod executor;
pub mod lifecycle;
pub mod registry;

pub use executor::{LoggingHook, TelemetryHook};
pub use lifecycle::{HookContext, HookType, WindowHook};
pub use registry::HookRegistry;
