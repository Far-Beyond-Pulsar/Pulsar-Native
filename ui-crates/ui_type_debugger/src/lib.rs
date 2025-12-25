//! Type Debugger UI
//!
//! Runtime type database inspection and debugging

mod type_debugger_drawer;
pub mod window;

// Re-export main types
pub use type_debugger_drawer::{TypeDebuggerDrawer, NavigateToType};
pub use window::TypeDebuggerWindow;
