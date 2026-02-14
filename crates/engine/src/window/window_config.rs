//! Window Configuration
//!
//! Centralizes window metadata (titles, sizes, etc.) to eliminate duplicate match statements.

use engine_state::WindowRequest;

/// Window metadata configuration
pub struct WindowConfig {
    pub title: &'static str,
    pub width: f64,
    pub height: f64,
}

impl WindowConfig {
    /// Get window configuration for a given window request type
    pub fn for_request(request: &WindowRequest) -> Option<Self> {
        match request {
            WindowRequest::Entry => Some(Self {
                title: "Pulsar Engine",
                width: 1280.0,
                height: 720.0,
            }),
            WindowRequest::Settings => Some(Self {
                title: "Settings",
                width: 800.0,
                height: 600.0,
            }),
            WindowRequest::About => Some(Self {
                title: "About Pulsar Engine",
                width: 600.0,
                height: 900.0,
            }),
            WindowRequest::Documentation => Some(Self {
                title: "Documentation",
                width: 1400.0,
                height: 900.0,
            }),
            WindowRequest::ProjectEditor { .. } => Some(Self {
                title: "Pulsar Engine - Project Editor",
                width: 1920.0,
                height: 1080.0,
            }),
            WindowRequest::ProjectSplash { .. } => Some(Self {
                title: "Loading Project...",
                width: 960.0,
                height: 540.0,
            }),
            WindowRequest::CloseWindow { .. } => None,
        }
    }
}
