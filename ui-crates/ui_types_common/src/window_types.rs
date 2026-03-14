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
    /// Open FAB asset marketplace search
    FabSearch,
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
    /// Any window opened via the PulsarWindow trait. type_name comes from PulsarWindow::window_name().
    Custom { type_name: &'static str },
}

pub type WindowId = u64;
