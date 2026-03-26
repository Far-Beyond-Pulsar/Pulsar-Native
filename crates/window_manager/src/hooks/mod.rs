pub mod lifecycle;
pub mod registry;
pub mod executor;

pub use lifecycle::{HookContext, HookType, WindowHook};
pub use registry::HookRegistry;
pub use executor::{LoggingHook, TelemetryHook};
