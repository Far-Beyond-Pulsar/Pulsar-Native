//! Events Nodes
//!
//! Nodes for event-driven programming and custom events in Pulsar blueprints.
//!
//! # Node Category: Events
//!
//! Provides utilities for emitting and handling custom events.

use crate::{blueprint, exec_output, NodeTypes};

// =============================================================================
// Entry Points / Event Definitions
// =============================================================================

/// Main entry point - defines the standard Rust main() function.
///
/// This event node defines the outer main() function. The execution chain
/// connected to the "Body" output pin will become the function body.
/// # Main
/// Defines the default Rust entry point `fn main()`.
#[blueprint(type: NodeTypes::event, category: "Events")]
pub fn main() {
    exec_output!("Body");
}

/// Begin Play event - runs when the program/engine starts.
///
/// This is typically used in game/engine contexts as an initialization point.
/// # Begin Play
/// Entry point that executes when the engine starts.
#[blueprint(type: NodeTypes::event, category: "Events")]
pub fn begin_play() {
    exec_output!("Body");
}

/// Emit a custom event (placeholder: does nothing).
///
/// # Inputs
/// - `event`: The event name
/// - `payload`: The event payload (as string)
///
/// # Events Emit
/// Emits a custom event with a payload.
#[blueprint(type: crate::NodeTypes::fn_, category: "Events")]
pub fn emit_event(_event: String, _payload: String) {
    // In a real implementation, this would notify listeners
}

/// Register a handler for a custom event (placeholder: does nothing).
///
/// # Inputs
/// - `event`: The event name
///
/// # Events On
/// Registers a handler for a custom event.
#[blueprint(type: crate::NodeTypes::fn_, category: "Events")]
pub fn on_event(_event: String) {
    // In a real implementation, this would register a callback
}

/// Remove an event handler (placeholder: does nothing).
///
/// # Inputs
/// - `event`: The event name
///
/// # Events Remove Listener
/// Removes a handler for a custom event.
#[blueprint(type: crate::NodeTypes::fn_, category: "Events")]
pub fn remove_event_listener(_event: String) {
    // In a real implementation, this would unregister a callback
}
