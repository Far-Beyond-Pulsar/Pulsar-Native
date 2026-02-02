//! Constructor methods for PulsarApp

use std::{path::PathBuf, sync::Arc};
use gpui::{AppContext, Context, Entity, Window};
use ui::dock::DockItem;
use ui::ContextModal;
use ui_file_manager::FileManagerDrawer;
use ui_problems::ProblemsDrawer;
use ui_level_editor::LevelEditorPanel;
use ui_type_debugger::TypeDebuggerDrawer;
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
        
        // Set project path in engine_state for access from other crates
        if let Some(ref path) = project_path {
            let path_str = path.to_string_lossy().to_string();
            println!("[ENGINE_STATE DEBUG] ========================================");
            println!("[ENGINE_STATE DEBUG] Setting project path to: {:?}", path);
            println!("[ENGINE_STATE DEBUG] As string: {:?}", path_str);
            engine_state::set_project_path(path_str.clone());
            println!("[ENGINE_STATE DEBUG] Verification - get_project_path(): {:?}", engine_state::get_project_path());
            println!("[ENGINE_STATE DEBUG] ========================================");
            tracing::info!("Set engine project path to {:?}", path);
        } else {
            println!("[ENGINE_STATE DEBUG] ========================================");
            println!("[ENGINE_STATE DEBUG] NO PROJECT PATH - project_path is None");
            println!("[ENGINE_STATE DEBUG] ========================================");
        }

        // Create drawers
        let file_manager_drawer = cx.new(|cx| FileManagerDrawer::new(project_path.clone(), window, cx));
        let problems_drawer = cx.new(|cx| ProblemsDrawer::new(window, cx));
        let type_debugger_drawer = cx.new(|cx| TypeDebuggerDrawer::new(window, cx));

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
        println!("[SUBSCRIPTION] Setting up subscription to center_tabs (ID: {:?}) for PanelEvent", center_tabs.entity_id());
        cx.subscribe_in(&center_tabs, window, event_handlers::on_tab_panel_event).detach();
        println!("[SUBSCRIPTION] Subscription to center_tabs set up successfully");

        // Subscribe to entry screen events
        if let Some(screen) = &entry_screen {
            cx.subscribe_in(screen, window, event_handlers::on_project_selected).detach();
        }

        // Initialize palette manager global
        ui_common::command_palette::PaletteManager::init(cx);

        // Initialize plugin manager
        tracing::debug!("🔌 Initializing plugin system");
        let mut plugin_manager = PluginManager::new();
        
        // Register built-in editors
        tracing::debug!("📝 Registering built-in editors");
        crate::register_all_builtin_editors(plugin_manager.builtin_registry_mut());
        
        // Register them with the file type and editor registries
        plugin_manager.register_builtin_editors();
        tracing::debug!("✅ Built-in editors registered");

        let plugins_dir = std::path::Path::new("plugins/editor");
        tracing::debug!("📂 Loading plugins from: {:?}", plugins_dir);

        match plugin_manager.load_plugins_from_dir(plugins_dir, &*cx) {
            Err(e) => {
                tracing::error!("❌ Failed to load editor plugins: {}", e);
            }
            Ok(_) => {
                let loaded_plugins = plugin_manager.get_plugins();
                tracing::debug!("✅ Loaded {} editor plugin(s)", loaded_plugins.len());
                for plugin in loaded_plugins {
                    tracing::debug!("   📦 {} v{} by {}", plugin.name, plugin.version, plugin.author);
                }
            }
        }

        let mut app = Self {
            state: crate::app::state::AppState {
                dock_area,
                project_path,
                entry_screen,
                file_manager_drawer,
                drawer_open: false,
                drawer_height: 400.0,
                drawer_resizing: false,
                problems_drawer,
                type_debugger_drawer,
                center_tabs,
                // script_editor: None, // Migrated to plugins
                // daw_editors: Vec::new(),
                // database_editors: Vec::new(),
                // struct_editors: Vec::new(),
                // enum_editors: Vec::new(),
                // trait_editors: Vec::new(),
                // alias_editors: Vec::new(),
                next_tab_id: 1,
                plugin_manager,
                rust_analyzer,
                analyzer_status_text: "Idle".to_string(),
                analyzer_detail_message: String::new(),
                analyzer_progress: 0.0,
                window_id,
                shown_welcome_notification: false,
                command_palette_open: false,
                command_palette_id: None,
                command_palette: None,
                command_palette_view: None,
                // active_type_picker_editor: None, // Migrated to plugins
                focus_handle: cx.focus_handle(),
            },
        };

        // Update file manager drawer with registered file types from plugin manager
        let file_types: Vec<plugin_editor_api::FileTypeDefinition> = app.state.plugin_manager
            .file_type_registry()
            .get_all_file_types()
            .into_iter()
            .cloned()
            .collect();

        app.state.file_manager_drawer.update(cx, |drawer, cx| {
            drawer.update_file_types(file_types);
            cx.notify();
        });

        // Sync TypeDatabase to UI if we have a project
        if has_project {
            if let Some(engine_state) = engine_state::EngineState::global() {
                if let Some(type_database) = engine_state.type_database() {
                    let types = type_database.all();
                    tracing::debug!("📊 Syncing {} types to TypeDebuggerDrawer", types.len());
                    app.state.type_debugger_drawer.update(cx, |drawer, cx| {
                        drawer.set_types(types, cx);
                    });
                }
            }
            
            // Set project root for problems drawer to display relative paths
            app.state.problems_drawer.update(cx, |drawer, cx| {
                drawer.set_project_root(app.state.project_path.clone(), cx);
            });

            // Set project root for type debugger drawer to display relative paths
            app.state.type_debugger_drawer.update(cx, |drawer, cx| {
                drawer.set_project_root(app.state.project_path.clone(), cx);
            });
        }

        // Update Discord presence with initial tab if project is loaded
        if has_project && create_level_editor {
            app.update_discord_presence(cx);
        }

        // Register command palette
        {
            use ui_common::command_palette::PaletteManager;
            use ui::IconName;
            use crate::actions::*;

            let (palette_id, palette_ref) = PaletteManager::register_palette("commands", window, cx);

            // Populate with command items
            palette_ref.update(cx, |palette, cx| {
                palette.add_item(
                    "Toggle File Manager",
                    "Show or hide the file manager panel",
                    IconName::Folder,
                    "View",
                    |window, cx| {
                        window.dispatch_action(Box::new(ToggleFileManager), cx);
                    },
                    cx,
                );

                palette.add_item(
                    "Open Settings",
                    "Open application settings",
                    IconName::Settings,
                    "Application",
                    |window, cx| {
                        window.dispatch_action(Box::new(ui::OpenSettings), cx);
                    },
                    cx,
                );

                palette.add_item(
                    "Toggle Multiplayer",
                    "Enable or disable multiplayer collaboration",
                    IconName::User,
                    "Application",
                    |window, cx| {
                        window.dispatch_action(Box::new(ToggleMultiplayer), cx);
                    },
                    cx,
                );

                palette.add_item(
                    "Toggle Problems",
                    "Show or hide the problems panel",
                    IconName::TriangleAlert,
                    "View",
                    |window, cx| {
                        window.dispatch_action(Box::new(ToggleProblems), cx);
                    },
                    cx,
                );

                palette.add_item(
                    "Build Project",
                    "Build the current project",
                    IconName::Hammer,
                    "Project",
                    |window, cx| {
                        window.push_notification(
                            ui::notification::Notification::info("Build")
                                .message("Building project..."),
                            cx
                        );
                    },
                    cx,
                );

                palette.add_item(
                    "Run Project",
                    "Run the current project",
                    IconName::Play,
                    "Project",
                    |window, cx| {
                        window.push_notification(
                            ui::notification::Notification::info("Run")
                                .message("Running project..."),
                            cx
                        );
                    },
                    cx,
                );

                // Add files if we have a project path
                if let Some(ref project_path) = app.state.project_path {
                    use ui_common::file_utils::find_openable_files;
                    let files = find_openable_files(project_path, Some(1000));

                    for file in files {
                        let path = file.path.clone();
                        palette.add_item(
                            file.name.clone(),
                            file.path.to_string_lossy().to_string(),
                            IconName::SubmitDocument,
                            "Files",
                            move |window, cx| {
                                window.dispatch_action(Box::new(OpenFile { path: path.clone() }), cx);
                            },
                            cx,
                        );
                    }
                }
            });

            app.state.command_palette_id = Some(palette_id);
            app.state.command_palette = Some(palette_ref);
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
