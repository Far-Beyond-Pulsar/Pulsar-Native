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

/// Legacy dispatch target of PBGC's bytecode backend for custom event
/// dispatch nodes (`emit_custom_event`). It does nothing.
///
/// Not offered in the Blueprint palette (#872): engine events are handled
/// and sent with the "On <Event>", "Send <Event> to" and "Broadcast
/// <Event>" nodes, which compile to module subscriptions and the
/// `event::send` / `event::emit` natives on the engine event hub (#924).
/// The old `on_event` and `remove_event_listener` placeholders are gone:
/// a subscription ends when its instance despawns.
///
/// # Emit Event (legacy)
/// Legacy no-op kept for PBGC's bytecode backend.
#[blueprint(type: crate::NodeTypes::fn_, category: "Events")]
pub fn emit_event() {}

// =============================================================================
// Engine Lifecycle Events
// =============================================================================

/// Tick event - runs every frame with a delta-time value.
///
/// Connect the execution chain to "Body" and read the `delta_time` output
/// to access the time (in seconds) since the last frame.
///
/// # On Tick
/// Entry point called every frame. `delta_time` is seconds since last frame.
#[blueprint(type: NodeTypes::event, category: "Events")]
pub fn on_tick(_delta_time: f32) {
    exec_output!("Body");
}

/// End Play event - runs when the object is destroyed or the scene stops.
///
/// Use this to release resources, stop effects, or clean up state.
///
/// # On End Play
/// Entry point called when the owning object is removed from the scene.
#[blueprint(type: NodeTypes::event, category: "Events")]
pub fn on_end_play() {
    exec_output!("Body");
}

// =============================================================================
// Time Utilities
// =============================================================================

/// Returns the time in seconds elapsed since the last frame.
///
/// This is a pure node; connect its output directly into time-driven logic.
///
/// # Get Delta Time
/// Returns the current frame delta time in seconds.
#[blueprint(type: crate::NodeTypes::pure, category: "Events")]
pub fn get_delta_time() -> f32 {
    // Resolved at runtime by the blueprint executor via engine context.
    0.0
}

// =============================================================================
// Input Events
// =============================================================================

/// Fires when a keyboard key is pressed or released.
///
/// # Inputs
/// - `key`: The key identifier string (e.g. `"Space"`, `"W"`, `"Escape"`)
///
/// # Outputs
/// - `pressed`: `true` on key-down, `false` on key-up
///
/// # On Input Key
/// Entry point for raw keyboard key press/release events.
#[blueprint(type: NodeTypes::event, category: "Input")]
pub fn on_input_key(_key: String, _pressed: bool) {
    exec_output!("Body");
}

/// Fires when a named input action is triggered.
///
/// Input actions are mapped strings (e.g. `"Jump"`, `"Fire"`) that abstract
/// over raw keys and controller buttons.
///
/// # Inputs
/// - `action`: The action name string
/// - `pressed`: `true` on action start, `false` on action end
///
/// # On Input Action
/// Entry point for named input action events.
#[blueprint(type: NodeTypes::event, category: "Input")]
pub fn on_input_action(_action: String, _pressed: bool) {
    exec_output!("Body");
}
