//! Application lifecycle and event handling module
//!
//! This module contains handlers for application lifecycle events and
//! the main event dispatcher that routes window events to specialized handlers.

pub mod lifecycle;
pub mod events;
pub mod close;

pub use lifecycle::{handle_resumed, handle_about_to_wait};
pub use events::dispatch_window_event;
pub use close::handle_close_requested;
