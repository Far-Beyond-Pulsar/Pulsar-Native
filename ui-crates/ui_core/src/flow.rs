//! Application Flow Manager
//!
//! Manages the application lifecycle and component transitions
//! 
//! Note: This is currently managed by engine_state::WindowRequest and window 
//! spawning in the main engine. This module is kept for future use.

/// Represents the current state of the application
#[derive(Clone, Debug, PartialEq)]
pub enum AppState {
    Loading,
    ProjectLauncher,
    Entry,
    Editor,
}

// Flow manager is currently unused - flow is managed by WindowRequest system
// All window management is handled through engine_state::WindowRequest
