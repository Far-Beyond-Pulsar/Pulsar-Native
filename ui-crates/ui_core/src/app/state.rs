//! Application state structure

use std::path::PathBuf;
use gpui::{Entity, FocusHandle};
use ui::dock::{DockArea, TabPanel};
use ui_file_manager::FileManagerDrawer;
use ui_problems::ProblemsDrawer;
// use ui_level_editor::LevelEditorPanel;
// use ui_daw_editor::DawEditorPanel;
use ui_type_debugger::TypeDebuggerDrawer;
use ui_entry::EntryScreen;
use ui_common::command_palette::{GenericPalette, Palette, PaletteId, PaletteViewDelegate};
use plugin_manager::PluginManager;
use engine_backend::services::rust_analyzer_manager::RustAnalyzerManager;

/// Core application state
pub struct AppState {
    // Dock system
    pub dock_area: Entity<DockArea>,
    pub center_tabs: Entity<TabPanel>,

    // Project management
    pub project_path: Option<PathBuf>,
    pub entry_screen: Option<Entity<EntryScreen>>,

    // Drawers
    pub file_manager_drawer: Entity<FileManagerDrawer>,
    pub drawer_open: bool,
    pub drawer_height: f32,
    pub drawer_resizing: bool,
    pub problems_drawer: Entity<ProblemsDrawer>,
    pub type_debugger_drawer: Entity<TypeDebuggerDrawer>,

    // Editor tracking - commented out as these editors have been migrated to plugins
    // pub daw_editors: Vec<Entity<DawEditorPanel>>,
    // pub database_editors: Vec<Entity<ui_editor_table::DataTableEditor>>,
    // pub struct_editors: Vec<Entity<ui_struct_editor::StructEditor>>,
    // pub enum_editors: Vec<Entity<ui_enum_editor::EnumEditor>>,
    // pub trait_editors: Vec<Entity<ui_trait_editor::TraitEditor>>,
    // pub alias_editors: Vec<Entity<ui_alias_editor::AliasEditor>>,

    // Tab management
    pub next_tab_id: usize,

    // Plugin system
    pub plugin_manager: PluginManager,

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

    // Type picker tracking - commented out as ui_alias_editor has been migrated to plugins
    // pub active_type_picker_editor: Option<Entity<ui_alias_editor::AliasEditor>>,

    // Focus management
    pub focus_handle: FocusHandle,
}
