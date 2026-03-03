// Shared window types for UI and engine_state

#[derive(Debug, Clone)]
pub enum WindowRequest {
    Entry,
    Settings,
    About,
    Documentation,
    ProjectEditor { project_path: String },
    ProjectSplash { project_path: String },
    GitManager { project_path: String },
    Problems,
    TypeDebugger,
    LogViewer,
    Multiplayer,
    PluginManager,
    Flamegraph,
    FileManager { project_path: Option<String> },
    DetachedPanel,
    Component,
    CloseWindow { window_id: u64 },
}

pub type WindowId = u64;
