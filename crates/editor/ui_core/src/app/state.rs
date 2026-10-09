//! Application state structure

use gpui::{Entity, FocusHandle, Task};
use std::path::PathBuf;
use std::sync::Arc;
use ui::dock::{DockArea, PanelView, TabPanel};
use ui_file_manager::FileManagerDrawer;
use ui_problems::ProblemsDrawer;
// use ui_level_editor::LevelEditorPanel;
// use ui_daw_editor::DawEditorPanel;
use engine_backend::services::RustAnalyzerManager;
use ui_common::command_palette::{GenericPalette, Palette, PaletteId, PaletteViewDelegate};
use ui_log_viewer::MissionControlPanel;
use ui_type_debugger::TypeDebuggerDrawer;

/// Core application state
pub struct AppState {
    /// Hold-Tab radial quick-action menu (Pulsar-Native#387).
    pub radial: super::radial_menu::RadialHost,

    // Dock system
    pub dock_area: Entity<DockArea>,
    pub center_tabs: Entity<TabPanel>,

    // Project management
    pub project_path: Option<PathBuf>,

    // Drawers
    pub file_manager_drawer: Entity<FileManagerDrawer>,
    pub drawer_open: bool,
    pub drawer_docked: bool,
    pub drawer_height: f32,
    pub drawer_resizing: bool,
    pub drawer_resize_start_y: f32,
    pub drawer_resize_start_height: f32,
    pub suppress_drawer_for_drag: bool, // Auto-close drawer during asset drag
    pub problems_drawer: Entity<ProblemsDrawer>,
    pub type_debugger_drawer: Entity<TypeDebuggerDrawer>,
    pub mission_control: Entity<MissionControlPanel>,
    pub mission_control_open: bool,
    pub git_manager_open: bool,
    pub task_queue_refresh_task: Option<Task<()>>,

    // Editor tracking - commented out as these editors have been migrated to plugins
    // pub daw_editors: Vec<Entity<DawEditorPanel>>,
    // pub database_editors: Vec<Entity<ui_editor_table::DataTableEditor>>,
    // pub struct_editors: Vec<Entity<ui_struct_editor::StructEditor>>,
    // pub enum_editors: Vec<Entity<ui_enum_editor::EnumEditor>>,
    // pub trait_editors: Vec<Entity<ui_trait_editor::TraitEditor>>,
    // pub alias_editors: Vec<Entity<ui_alias_editor::AliasEditor>>,

    // Tab management
    pub next_tab_id: usize,

    // Note: PluginManager is now globally accessible via plugin_manager::global()

    // Rust Analyzer
    pub rust_analyzer: Entity<RustAnalyzerManager>,
    pub analyzer_status_text: String,
    pub analyzer_detail_message: String,
    pub analyzer_progress: f32,

    // Window management
    pub window_id: Option<u64>,

    // Notifications
    pub shown_welcome_notification: bool,

    // Command Palette
    pub command_palette_open: bool,
    pub command_palette_id: Option<PaletteId>,
    pub command_palette: Option<Entity<Palette>>,
    pub command_palette_view: Option<Entity<GenericPalette<PaletteViewDelegate>>>,

    // Project Switcher
    pub project_switcher_open: bool,
    pub project_switcher_view: Option<Entity<crate::project_switcher::ProjectSwitcherView>>,

    // Type picker tracking - commented out as ui_alias_editor has been migrated to plugins
    // pub active_type_picker_editor: Option<Entity<ui_alias_editor::AliasEditor>>,

    // Focus management
    pub focus_handle: FocusHandle,

    // Popped out panels tracking (panel, source tab panel)
    pub popped_out_panels: Vec<Arc<dyn PanelView>>,

    // Multiuser status refresh listener
    pub multiuser_refresh_task: Option<Task<()>>,

    // Git auto-fetch listener for the primary project window
    pub git_auto_fetch_task: Option<Task<()>>,

    // Dock layout persistence (see `layout_persistence`)
    /// This window saves and restores the project layout.
    pub layout_persist: bool,
    /// Last known window geometry, saved with the layout.
    pub window_bounds: Option<gpui::WindowBounds>,
    /// The last windowed (not maximized / fullscreen) bounds: the size to
    /// return to when leaving those states.
    pub window_restore_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    /// The saved layout has been restored (or there was none); saving is safe.
    pub layout_ready: bool,
    /// Pending debounced save; dropping it cancels the save.
    pub layout_save_task: Option<Task<()>>,

    // Navigation history
    pub navigation: super::navigation::NavigationHistory,

    /// Unified left sidebar (Pulsar-Native#1000).
    pub nav_sidebar: super::nav_sidebar::NavSidebarState,
}

impl AppState {
    pub fn push_navigation(&mut self, path: PathBuf) {
        self.navigation.visit(path);
    }

    pub fn go_back(&mut self) -> Option<PathBuf> {
        self.navigation.back()
    }

    pub fn go_forward(&mut self) -> Option<PathBuf> {
        self.navigation.forward()
    }
}
