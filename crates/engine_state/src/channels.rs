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
    /// Open problems/diagnostics window
    Problems,
    /// Open type debugger window
    TypeDebugger,
    /// Open mission control / log viewer window
    LogViewer,
    /// Open multiplayer/collaboration window
    Multiplayer,
    /// Open plugin manager window
    PluginManager,
    /// Open flamegraph profiler window
    Flamegraph,
    /// Open file manager as standalone window
    FileManager { project_path: Option<String> },
    /// Open detached panel window
    DetachedPanel,
    /// Close specific window
    CloseWindow { window_id: u64 },
}

pub type WindowRequestSender = Sender<WindowRequest>;
pub type WindowRequestReceiver = Receiver<WindowRequest>;

/// Create a window request channel
pub fn window_request_channel() -> (WindowRequestSender, WindowRequestReceiver) {
    channel()
}
