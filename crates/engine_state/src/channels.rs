//! Communication Channels
//!
//! Channels for inter-component communication

use std::sync::mpsc::{channel, Sender, Receiver};

/// Window creation/management request
#[derive(Debug, Clone)]
pub enum WindowRequest {
    /// Open entry/launcher window
    Entry,
    /// Open settings window
    Settings,
    /// Open about window
    About,
    /// Open documentation window
    Documentation,
    /// Open project editor
    ProjectEditor { project_path: String },
    /// Open project splash screen
    ProjectSplash { project_path: String },
    /// Open git manager window
    GitManager { project_path: String },
    /// Close specific window
    CloseWindow { window_id: u64 },
}

pub type WindowRequestSender = Sender<WindowRequest>;
pub type WindowRequestReceiver = Receiver<WindowRequest>;

/// Create a window request channel
pub fn window_request_channel() -> (WindowRequestSender, WindowRequestReceiver) {
    channel()
}
