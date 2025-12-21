//! Constructor methods for PulsarApp

use std::{path::PathBuf, sync::Arc};
use gpui::{AppContext, Context, Entity, Window};
use ui::dock::DockItem;
use ui_editor::{FileManagerDrawer, LevelEditorPanel, ProblemsDrawer, TerminalDrawer};
use ui_entry::EntryScreen;
use plugin_manager::PluginManager;
use engine_backend::services::rust_analyzer_manager::RustAnalyzerManager;

use super::{PulsarApp, event_handlers};

impl PulsarApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_internal(None, None, None, true, window, cx)
    }

    pub fn new_with_project(
        project_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        tracing::debug!(
            "PulsarApp::new_with_project called with path: {:?}",
            project_path
        );
        Self::new_internal(Some(project_path), None, None, true, window, cx)
    }

    /// Create a new PulsarApp with window_id for GPU renderer registration
    pub fn new_with_project_and_window_id(
        project_path: PathBuf,
        window_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        tracing::debug!(
            "PulsarApp::new_with_project_and_window_id called with path: {:?}, window_id: {}",
            project_path, window_id
        );
        Self::new_internal(Some(project_path), None, Some(window_id), true, window, cx)
    }

    /// Create a new PulsarApp with a pre-initialized rust analyzer
    pub fn new_with_project_and_analyzer(
        project_path: PathBuf,
        rust_analyzer: Entity<RustAnalyzerManager>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        tracing::debug!(
            "PulsarApp::new_with_project_and_analyzer called with path: {:?}",
            project_path
        );
        let app = Self::new_internal(Some(project_path.clone()), Some(rust_analyzer.clone()), None, true, window, cx);

        // Start rust-analyzer in the background
        rust_analyzer.update(cx, |analyzer, cx| {
            analyzer.start(project_path, window, cx);
        });

        app
    }

    /// Create a new window that shares the rust analyzer from an existing window
    pub fn new_with_shared_analyzer(
        project_path: Option<PathBuf>,
        rust_analyzer: Entity<RustAnalyzerManager>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_internal(project_path, Some(rust_analyzer), None, false, window, cx)
    }

    pub(super) fn new_internal(
        project_path: Option<PathBuf>,
        shared_rust_analyzer: Option<Entity<RustAnalyzerManager>>,
        window_id: Option<u64>,
        create_level_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create the main dock area
        let dock_area = cx.new(|cx| ui::dock::DockArea::new("main-dock", Some(1), window, cx));
        let weak_dock = dock_area.downgrade();

        // Create center dock item with level editor tab if requested
        let center_dock_item = if create_level_editor {
            let level_editor = if let Some(wid) = window_id {
                cx.new(|cx| LevelEditorPanel::new_with_window_id(wid, window, cx))
            } else {
                cx.new(|cx| LevelEditorPanel::new(window, cx))
            };
            DockItem::tabs(
                vec![Arc::new(level_editor.clone())],
                Some(0),
                &weak_dock,
                window,
                cx,
            )
        } else {
            DockItem::tabs(
                vec![],
                None,
                &weak_dock,
                window,
                cx,
            )
        };

        let center_tabs = if let DockItem::Tabs { view, .. } = &center_dock_item {
            view.clone()
        } else {
            panic!("Expected tabs dock item");
        };

        dock_area.update(cx, |dock, cx| {
            dock.set_center(center_dock_item, window, cx);
        });

        // Create entry screen only if no project path is provided
        let entry_screen = if project_path.is_none() {
            let screen = cx.new(|cx| EntryScreen::new(window, cx));
            Some(screen)
        } else {
            None
        };

        // Store project_path before moving it
        let has_project = project_path.is_some();

        // Create drawers
        let file_manager_drawer = cx.new(|cx| FileManagerDrawer::new(project_path.clone(), window, cx));
        let problems_drawer = cx.new(|cx| ProblemsDrawer::new(window, cx));
        let terminal_drawer = cx.new(|cx| TerminalDrawer::new(window, cx));

        // Subscribe to drawer events
        cx.subscribe_in(&file_manager_drawer, window, event_handlers::on_file_selected).detach();
        cx.subscribe_in(&file_manager_drawer, window, event_handlers::on_popout_file_manager).detach();
        cx.subscribe_in(&problems_drawer, window, event_handlers::on_navigate_to_diagnostic).detach();

        // Create rust analyzer manager or use shared one
        let rust_analyzer = if let Some(shared_analyzer) = shared_rust_analyzer {
            shared_analyzer
        } else {
            let analyzer = cx.new(|cx| RustAnalyzerManager::new(window, cx));

            // Start rust analyzer if we have a project
            if let Some(ref project) = project_path {
                analyzer.update(cx, |analyzer, cx| {
                    analyzer.start(project.clone(), window, cx);
                });
            }

            analyzer
        };

        // Subscribe to analyzer events
        cx.subscribe_in(&rust_analyzer, window, event_handlers::on_analyzer_event).detach();

        // Subscribe to tab panel events
        cx.subscribe_in(&center_tabs, window, event_handlers::on_tab_panel_event).detach();

        // Subscribe to entry screen events
        if let Some(screen) = &entry_screen {
            cx.subscribe_in(screen, window, event_handlers::on_project_selected).detach();
        }

        // Initialize plugin manager
        tracing::info!("🔌 Initializing plugin system");
        let mut plugin_manager = PluginManager::new();

        let plugins_dir = std::path::Path::new("plugins/editor");
        tracing::info!("📂 Loading plugins from: {:?}", plugins_dir);

        match plugin_manager.load_plugins_from_dir(plugins_dir, &*cx) {
            Err(e) => {
                tracing::error!("❌ Failed to load editor plugins: {}", e);
            }
            Ok(_) => {
                let loaded_plugins = plugin_manager.get_plugins();
                tracing::info!("✅ Loaded {} editor plugin(s)", loaded_plugins.len());
                for plugin in loaded_plugins {
                    tracing::info!("   📦 {} v{} by {}", plugin.name, plugin.version, plugin.author);
                }
            }
        }

        let app = Self {
            state: crate::app::state::AppState {
                dock_area,
                project_path,
                entry_screen,
                file_manager_drawer,
                drawer_open: false,
                problems_drawer,
                terminal_drawer,
                center_tabs,
                script_editor: None,
                daw_editors: Vec::new(),
                database_editors: Vec::new(),
                struct_editors: Vec::new(),
                enum_editors: Vec::new(),
                trait_editors: Vec::new(),
                alias_editors: Vec::new(),
                next_tab_id: 1,
                plugin_manager,
                rust_analyzer,
                analyzer_status_text: "Idle".to_string(),
                analyzer_detail_message: String::new(),
                analyzer_progress: 0.0,
                window_id,
                shown_welcome_notification: false,
                command_palette_open: false,
                command_palette: None,
                active_type_picker_editor: None,
                focus_handle: cx.focus_handle(),
            },
        };

        // Update Discord presence with initial tab if project is loaded
        if has_project && create_level_editor {
            app.update_discord_presence(cx);
        }

        app
    }

    /// Get the global rust analyzer manager
    pub fn rust_analyzer(&self) -> &Entity<RustAnalyzerManager> {
        &self.state.rust_analyzer
    }

    /// Get the current workspace root
    pub fn workspace_root(&self) -> Option<&PathBuf> {
        self.state.project_path.as_ref()
    }
}
