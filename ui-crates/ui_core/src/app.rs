use gpui::{prelude::*, Animation, AnimationExt as _, SharedString, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockItem, Panel, PanelEvent, TabPanel},
    notification::Notification,
    h_flex, v_flex, ActiveTheme as _, ContextModal as _, Icon, IconName, StyledExt,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use std::{sync::Arc, time::Duration};
use plugin_manager::PluginManager;
use plugin_editor_api::EditorInstance;

// Import from new modular ui-crates
use ui_common::{
    command_palette::{GenericPalette, CommandOrFile, CommandType},
    menu::AppTitleBar,
};
use crate::unified_palette::AnyPaletteDelegate;
use engine_backend::services::rust_analyzer_manager::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};

// Editor panels and drawers (all from ui_editor)
use ui_editor::{
    DawEditorPanel, LevelEditorPanel, ScriptEditorPanel,
    FileManagerDrawer, TerminalDrawer, ProblemsDrawer,
    FileSelected, DrawerFileType as FileType, PopoutFileManagerEvent,
};
// Blueprint editor migrated to plugin system
use ui_alias_editor::ShowTypePickerRequest;

// Standalone windows
use ui_entry::{EntryScreen, ProjectSelected};
use ui_file_manager::FileManagerWindow;
use ui_terminal::{TerminalWindow, Terminal};
use ui_problems::ProblemsWindow;
use ui_multiplayer::MultiplayerWindow;

// EditorType enum
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorType {
    Script,
    Level,
    Daw,
    // Blueprint editor migrated to plugin system
}

impl EditorType {
    pub fn display_name(&self) -> &'static str {
        match self {
            EditorType::Script => "Script Editor",
            EditorType::Level => "Level Editor",
            EditorType::Daw => "DAW Editor",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            EditorType::Script => "Code editor with LSP support",
            EditorType::Level => "3D level design and placement",
            EditorType::Daw => "Digital audio workstation",
        }
    }
}

// Action to toggle the file manager drawer
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleFileManager;

// Action to toggle the problems drawer
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleProblems;

// Action to toggle the terminal
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleTerminal;

// Action to toggle multiplayer collaboration
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleMultiplayer;

// Action to toggle the command palette
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = pulsar_app)]
pub struct ToggleCommandPalette;

// Root wrapper that contains the titlebar, matching gpui-component storybook structure
pub struct PulsarRoot {
    title_bar: Entity<AppTitleBar>,
    app: Entity<PulsarApp>,
}

impl PulsarRoot {
    pub fn new(
        title: impl Into<SharedString>,
        app: Entity<PulsarApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_bar = cx.new(|cx| AppTitleBar::new(title, window, cx));
        Self { title_bar, app }
    }
}

impl Render for PulsarRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::Root;
        
        let drawer_layer = Root::render_drawer_layer(window, cx);
        let modal_layer = Root::render_modal_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .size_full()
            .child(
                v_flex()
                    .size_full()
                    .child(self.title_bar.clone())
                    .child(div().flex_1().overflow_hidden().child(self.app.clone()))
            )
            .children(drawer_layer)
            .children(modal_layer)
            .children(notification_layer)
    }
}

pub struct PulsarApp {
    dock_area: Entity<DockArea>,
    project_path: Option<PathBuf>,
    entry_screen: Option<Entity<EntryScreen>>,
    file_manager_drawer: Entity<FileManagerDrawer>,
    drawer_open: bool,
    problems_drawer: Entity<ProblemsDrawer>,
    terminal_drawer: Entity<TerminalDrawer>,
    // Tab management
    center_tabs: Entity<TabPanel>,
    script_editor: Option<Entity<ScriptEditorPanel>>,
    // Blueprint editors migrated to plugin system
    daw_editors: Vec<Entity<DawEditorPanel>>,
    database_editors: Vec<Entity<ui_editor_table::DataTableEditor>>,
    // Type System Editors
    struct_editors: Vec<Entity<ui_struct_editor::StructEditor>>,
    enum_editors: Vec<Entity<ui_enum_editor::EnumEditor>>,
    trait_editors: Vec<Entity<ui_trait_editor::TraitEditor>>,
    alias_editors: Vec<Entity<ui_alias_editor::AliasEditor>>,
    next_tab_id: usize,
    // Plugin Manager
    plugin_manager: PluginManager,
    // Rust Analyzer
    rust_analyzer: Entity<RustAnalyzerManager>,
    analyzer_status_text: String,
    analyzer_detail_message: String, // Detailed progress message (e.g., "Indexing workspace (50/200 crates)")
    analyzer_progress: f32, // 0.0 to 1.0 for progress bar
    // Window identification for GPU renderer registration
    window_id: Option<u64>,
    // Notification tracking
    shown_welcome_notification: bool,
    // Command Palette (unified generic palette)
    command_palette_open: bool,
    command_palette: Option<Entity<GenericPalette<AnyPaletteDelegate>>>,
    // Track which alias editor requested the type picker (for type selection callback)
    active_type_picker_editor: Option<Entity<ui_alias_editor::AliasEditor>>,
    // Focus management
    focus_handle: FocusHandle,
}

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
    /// This is used when loading from the loading screen to avoid re-initializing
    /// The analyzer is started in the background immediately after the editor opens
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
        
        // Start rust-analyzer in the background NOW - don't block editor opening
        rust_analyzer.update(cx, |analyzer, cx| {
            analyzer.start(project_path, window, cx);
        });
        
        app
    }

    /// Create a new window that shares the rust analyzer from an existing window
    /// This is used for detached windows and doesn't create a default Level Editor tab
    pub fn new_with_shared_analyzer(
        project_path: Option<PathBuf>,
        rust_analyzer: Entity<RustAnalyzerManager>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_internal(project_path, Some(rust_analyzer), None, false, window, cx)
    }

    /// Get the global rust analyzer manager
    pub fn rust_analyzer(&self) -> &Entity<RustAnalyzerManager> {
        &self.rust_analyzer
    }

    /// Get the current workspace root
    pub fn workspace_root(&self) -> Option<&PathBuf> {
        self.project_path.as_ref()
    }

    /// Create a detached window with a panel, sharing the rust analyzer
    fn create_detached_window(
        &self,
        panel: Arc<dyn ui::dock::PanelView>,
        position: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui::{px, size, Bounds, Point, WindowBounds, WindowKind, WindowOptions};
        use ui::Root;

        let window_size = size(px(800.), px(600.));
        let window_bounds = Bounds::new(
            Point {
                x: position.x - px(100.0),
                y: position.y - px(30.0),
            },
            window_size,
        );

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: None, // No traffic lights
            }),
            window_min_size: Some(gpui::Size {
                width: px(400.),
                height: px(300.),
            }),
            kind: WindowKind::Normal,
            is_resizable: true,
            window_decorations: Some(gpui::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };

        let project_path = self.project_path.clone();
        let rust_analyzer = self.rust_analyzer.clone();

        let _ = cx.open_window(window_options, move |window, cx| {
            // Create PulsarApp with shared rust analyzer
            let app = cx.new(|cx| {
                let app = Self::new_with_shared_analyzer(
                    project_path.clone(),
                    rust_analyzer.clone(),
                    window,
                    cx,
                );

                // Add the panel to the center tabs
                app.center_tabs.update(cx, |tabs, cx| {
                    tabs.add_panel(panel.clone(), window, cx);
                });

                app
            });

            cx.new(|cx| Root::new(app.into(), window, cx))
        });
    }

    fn new_internal(
        project_path: Option<PathBuf>,
        shared_rust_analyzer: Option<Entity<RustAnalyzerManager>>,
        window_id: Option<u64>,
        create_level_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create the main dock area
        let dock_area = cx.new(|cx| DockArea::new("main-dock", Some(1), window, cx));
        let weak_dock = dock_area.downgrade();

        // Create center dock item with level editor tab if requested
        let center_dock_item = if create_level_editor {
            // Pass window_id to LevelEditorPanel for GPU renderer registration
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
            // Create empty tabs for detached windows
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

        // Initialize editor tracking
        let script_editor = None;
        // Blueprint editors migrated to plugin system
        let daw_editors = Vec::new();
        let database_editors = Vec::new();
        let struct_editors = Vec::new();
        let enum_editors = Vec::new();
        let trait_editors = Vec::new();
        let alias_editors = Vec::new();

        // Create entry screen only if no project path is provided
        let entry_screen = if project_path.is_none() {
            let screen = cx.new(|cx| EntryScreen::new(window, cx));
            Some(screen)
        } else {
            None
        };

        // Create file manager drawer with the project path if provided
        let file_manager_drawer =
            cx.new(|cx| FileManagerDrawer::new(project_path.clone(), window, cx));
        cx.subscribe_in(&file_manager_drawer, window, Self::on_file_selected)
            .detach();
        cx.subscribe_in(&file_manager_drawer, window, Self::on_popout_file_manager)
            .detach();

        // Create problems drawer
        let problems_drawer = cx.new(|cx| ProblemsDrawer::new(window, cx));
        cx.subscribe_in(&problems_drawer, window, Self::on_navigate_to_diagnostic)
            .detach();

        // Create terminal drawer
        let terminal_drawer = cx.new(|cx| TerminalDrawer::new(window, cx));

        // Create rust analyzer manager or use shared one
        let rust_analyzer = if let Some(shared_analyzer) = shared_rust_analyzer {
            // Use the shared rust analyzer from another window
            shared_analyzer
        } else {
            // Create a new rust analyzer for this window
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
        cx.subscribe_in(&rust_analyzer, window, Self::on_analyzer_event)
            .detach();

        // Subscribe to PanelEvent on center_tabs to handle tab close and cleanup
        cx.subscribe_in(&center_tabs, window, Self::on_tab_panel_event)
            .detach();

        // Subscribe to ProjectSelected events from entry screen or project selector
        if let Some(screen) = &entry_screen {
            cx.subscribe_in(screen, window, Self::on_project_selected)
                .detach();
        }

        // Initialize plugin manager and load plugins
        tracing::info!("🔌 Initializing plugin system");
        let mut plugin_manager = PluginManager::new();

        // Load plugins from plugins/editor directory
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
            dock_area,
            project_path,
            entry_screen,
            file_manager_drawer,
            drawer_open: false,
            problems_drawer,
            terminal_drawer,
            center_tabs,
            script_editor,
            daw_editors,
            database_editors,
            struct_editors,
            enum_editors,
            trait_editors,
            alias_editors,
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
        };

        app
    }

    fn on_analyzer_event(
        &mut self,
        _manager: &Entity<RustAnalyzerManager>,
        event: &AnalyzerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AnalyzerEvent::StatusChanged(status) => {
                match status {
                    AnalyzerStatus::Idle => {
                        self.analyzer_status_text = "Idle".to_string();
                        self.analyzer_detail_message = String::new();
                        self.analyzer_progress = 0.0;
                    }
                    AnalyzerStatus::Starting => {
                        self.analyzer_status_text = "Starting...".to_string();
                        self.analyzer_detail_message = "Initializing language server".to_string();
                        self.analyzer_progress = 0.0;
                    }
                    AnalyzerStatus::Indexing { progress, message } => {
                        self.analyzer_status_text = "Indexing".to_string();
                        self.analyzer_detail_message = message.clone();
                        self.analyzer_progress = *progress;
                    }
                    AnalyzerStatus::Ready => {
                        self.analyzer_status_text = "Ready".to_string();
                        self.analyzer_detail_message = "Code intelligence active".to_string();
                        self.analyzer_progress = 1.0;
                        // Show success notification
                        window.push_notification(
                            Notification::success("Rust Analyzer Ready")
                                .message("Code intelligence is now available"),
                            cx
                        );
                    }
                    AnalyzerStatus::Error(e) => {
                        self.analyzer_status_text = "Error".to_string();
                        self.analyzer_detail_message = e.to_string();
                        self.analyzer_progress = 0.0;
                        // Show error notification
                        window.push_notification(
                            Notification::error("Analyzer Error")
                                .message(e.to_string()),
                            cx
                        );
                    }
                    AnalyzerStatus::Stopped => {
                        self.analyzer_status_text = "Stopped".to_string();
                        self.analyzer_detail_message = String::new();
                        self.analyzer_progress = 0.0;
                    }
                };
                cx.notify();
            }
            AnalyzerEvent::IndexingProgress { progress, message } => {
                self.analyzer_status_text = "Indexing".to_string();
                self.analyzer_detail_message = message.clone();
                self.analyzer_progress = *progress;
                cx.notify();
            }
            AnalyzerEvent::Ready => {
                self.analyzer_status_text = "Ready".to_string();
                self.analyzer_detail_message = "Code intelligence active".to_string();
                self.analyzer_progress = 1.0;
                // Show success notification
                window.push_notification(
                    Notification::success("Code Intelligence Ready")
                        .message("Rust Analyzer indexing complete"),
                    cx
                );
                cx.notify();
            }
            AnalyzerEvent::Error(e) => {
                self.analyzer_status_text = "Error".to_string();
                self.analyzer_detail_message = e.to_string();
                self.analyzer_progress = 0.0;
                // Show error notification
                window.push_notification(
                    Notification::error("Analyzer Error").message(e.to_string()),
                    cx
                );
                cx.notify();
            }
            AnalyzerEvent::Diagnostics(diagnostics) => {
                // Convert and forward diagnostics to the problems drawer
                let problems_diagnostics: Vec<ui_problems::Diagnostic> = diagnostics.iter().map(|d| {
                    ui_problems::Diagnostic {
                        file_path: d.file_path.clone(),
                        line: d.line,
                        column: d.column,
                        severity: match d.severity {
                            ui_common::DiagnosticSeverity::Error => ui_problems::DiagnosticSeverity::Error,
                            ui_common::DiagnosticSeverity::Warning => ui_problems::DiagnosticSeverity::Warning,
                            ui_common::DiagnosticSeverity::Information => ui_problems::DiagnosticSeverity::Information,
                            ui_common::DiagnosticSeverity::Hint => ui_problems::DiagnosticSeverity::Hint,
                        },
                        message: d.message.clone(),
                        source: d.source.clone(),
                    }
                }).collect();
                self.problems_drawer.update(cx, |drawer, cx| {
                    drawer.set_diagnostics(problems_diagnostics, cx);
                });
                cx.notify();
            }
        }
    }

    fn on_project_selected(
        &mut self,
        _selector: &Entity<EntryScreen>,
        event: &ProjectSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project_path = Some(event.path.clone());
        self.entry_screen = None; // Hide entry screen once project is loaded

        // Update file manager with project path
        self.file_manager_drawer.update(cx, |drawer, cx| {
            drawer.set_project_path(event.path.clone(), cx);
        });

        // Start rust analyzer for the project
        self.rust_analyzer.update(cx, |analyzer, cx| {
            analyzer.start(event.path.clone(), window, cx);
        });

        println!("Project selected: {:?}", event.path);
        cx.notify();
    }

    fn on_tab_panel_event(
        &mut self,
        _tabs: &Entity<TabPanel>,
        event: &PanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PanelEvent::MoveToNewWindow(panel, position) => {
                // Create a new window with the same project and shared rust analyzer
                self.create_detached_window(panel.clone(), *position, window, cx);
            }
            PanelEvent::TabClosed(entity_id) => {
                // Blueprint editors removed - handled by plugin system
                self.daw_editors
                    .retain(|e| e.entity_id() != *entity_id);
                self.database_editors
                    .retain(|e| e.entity_id() != *entity_id);
                self.struct_editors
                    .retain(|e| e.entity_id() != *entity_id);
                self.enum_editors
                    .retain(|e| e.entity_id() != *entity_id);
                self.trait_editors
                    .retain(|e| e.entity_id() != *entity_id);
                self.alias_editors
                    .retain(|e| e.entity_id() != *entity_id);
            }
            _ => {}
        }
    }

    fn on_file_selected(
        &mut self,
        _drawer: &Entity<FileManagerDrawer>,
        event: &FileSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!(
            "FileSelected event received - path: {:?}, type: {:?}",
            event.path, event.file_type
        );

        // Try to open via plugin system first
        match self.plugin_manager.create_editor_for_file(&event.path, window, cx) {
            Ok((panel, _editor_instance)) => {
                tracing::info!("✅ Plugin system successfully created editor for: {:?}", event.path);

                // Add the panel to the tab system
                self.center_tabs.update(cx, |tabs, cx| {
                    tabs.add_panel(panel, window, cx);
                });

                // Close the drawer
                self.drawer_open = false;
                cx.notify();
                return;
            }
            Err(e) => {
                tracing::debug!("Plugin manager couldn't open file: {} - falling through to legacy code", e);
                // Fall through to legacy code
            }
        }

        // Legacy hardcoded file opening (will be replaced by plugin system)
        match event.file_type {
            FileType::Class => {
                tracing::error!("Blueprint editor tried to load via legacy code, but it's now plugin-only!");
                // Blueprint editor has been fully migrated to plugin system
                // If we get here, it means the plugin system failed to load the file
            }
            FileType::Script => {
                tracing::warn!("Opening script tab using legacy code (should use plugin system)");
                self.open_script_tab(event.path.clone(), window, cx);
            }
            FileType::DawProject => {
                tracing::warn!("Opening DAW tab using legacy code: {:?}", event.path);
                self.open_daw_tab(event.path.clone(), window, cx);
            }
            FileType::Database => {
                tracing::warn!("Opening database tab using legacy code: {:?}", event.path);
                self.open_database_tab(event.path.clone(), window, cx);
            }
            FileType::StructType => {
                tracing::warn!("Opening struct editor using legacy code: {:?}", event.path);
                self.open_struct_tab(event.path.clone(), window, cx);
            }
            FileType::EnumType => {
                tracing::warn!("Opening enum editor using legacy code: {:?}", event.path);
                self.open_enum_tab(event.path.clone(), window, cx);
            }
            FileType::TraitType => {
                tracing::warn!("Opening trait editor using legacy code: {:?}", event.path);
                self.open_trait_tab(event.path.clone(), window, cx);
            }
            FileType::AliasType => {
                tracing::warn!("Opening alias editor using legacy code: {:?}", event.path);
                self.open_alias_tab(event.path.clone(), window, cx);
            }
            _ => {
                tracing::debug!("Unknown file type, ignoring");
            }
        }

        // Close the drawer after opening a file
        self.drawer_open = false;
        cx.notify();
    }

    fn on_popout_file_manager(
        &mut self,
        _drawer: &Entity<FileManagerDrawer>,
        event: &PopoutFileManagerEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui::{px, size, Bounds, Point, WindowBounds, WindowKind, WindowOptions};
        use ui::Root;

        let project_path = event.project_path.clone();

        // Get a clone of self as Entity to pass to the file manager window
        let _parent_app = cx.entity().clone();

        // Close the drawer when popping out
        self.drawer_open = false;
        cx.notify();

        // Open the file manager window
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(100.0),
                        y: px(100.0),
                    },
                    size: size(px(1000.0), px(700.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None, // No traffic lights
                }),
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(600.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            move |window, cx| {
                // Create a new file manager drawer for the window (use new_in_window)
                let new_drawer =
                    cx.new(|cx| FileManagerDrawer::new_in_window(project_path.clone(), window, cx));

                let file_manager_window =
                    cx.new(|cx| FileManagerWindow::new(new_drawer, window, cx));

                cx.new(|cx| Root::new(file_manager_window.into(), window, cx))
            },
        );
    }

    fn toggle_drawer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.drawer_open = !self.drawer_open;
        cx.notify();
    }

    fn toggle_problems(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use gpui::{px, size, Bounds, Point, WindowBounds, WindowKind, WindowOptions};
        use ui::Root;

        // Open problems in a separate window
        let problems_drawer = self.problems_drawer.clone();

        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(100.0),
                        y: px(100.0),
                    },
                    size: size(px(900.0), px(600.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None, // No traffic lights
                }),
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(500.),
                    height: px(300.),
                }),
                ..Default::default()
            },
            |window, cx| {
                let problems_window = cx.new(|cx| ProblemsWindow::new(problems_drawer, window, cx));

                cx.new(|cx| Root::new(problems_window.into(), window, cx))
            },
        );
    }

    fn toggle_terminal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use gpui::{px, size, Bounds, Point, WindowBounds, WindowKind, WindowOptions};
        use ui::Root;

        // Open terminal in a separate window
        // Each window gets its own independent terminal instance
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(150.0),
                        y: px(150.0),
                    },
                    size: size(px(1000.0), px(700.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None, // No traffic lights
                }),
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(600.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            |window, cx| {
                // Create a NEW terminal for this window (independent terminal session)
                let terminal = cx.new(|cx| Terminal::new(window, cx).expect("Failed to create terminal"));
                let terminal_window = cx.new(|cx| TerminalWindow::new(terminal, window, cx));

                cx.new(|cx| Root::new(terminal_window.into(), window, cx))
            },
        );
    }

    fn toggle_multiplayer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use gpui::{px, size, Bounds, Point, WindowBounds, WindowKind, WindowOptions};
        use ui::Root;

        // Open multiplayer window
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(200.0),
                        y: px(200.0),
                    },
                    size: size(px(500.0), px(600.0)),
                })),
                titlebar: Some(gpui::TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                kind: WindowKind::Normal,
                is_resizable: true,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: px(400.),
                    height: px(500.),
                }),
                ..Default::default()
            },
            |window, cx| {
                let multiplayer_window = cx.new(|cx| MultiplayerWindow::new(window, cx));
                cx.new(|cx| Root::new(multiplayer_window.into(), window, cx))
            },
        );
    }

    fn on_toggle_file_manager(
        &mut self,
        _: &ToggleFileManager,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_drawer(window, cx);
    }

    fn on_toggle_problems(
        &mut self,
        _: &ToggleProblems,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_problems(window, cx);
    }

    fn on_toggle_terminal(
        &mut self,
        _: &ToggleTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_terminal(window, cx);
    }

    fn on_toggle_multiplayer(
        &mut self,
        _: &ToggleMultiplayer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_multiplayer(window, cx);
    }

    fn on_toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command_palette_open = !self.command_palette_open;

        if self.command_palette_open {
            if let Some(palette) = &self.command_palette {
                // Palette already exists, just swap in command delegate
                palette.update(cx, |palette, cx| {
                    let delegate = AnyPaletteDelegate::command(self.project_path.clone());
                    palette.swap_delegate(delegate, window, cx);
                });

                // Focus the input for typing (actions bubble up to palette)
                let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
                input_handle.focus(window);
            } else {
                // Create the palette for the first time
                let delegate = AnyPaletteDelegate::command(self.project_path.clone());
                let palette = cx.new(|cx| {
                    GenericPalette::new(delegate, window, cx)
                });

                // Subscribe to dismiss events
                cx.subscribe_in(&palette, window, |this: &mut PulsarApp, palette, _event: &DismissEvent, window, cx| {
                    // Extract the selected item first
                    let selected_item = palette.update(cx, |palette, _cx| {
                        palette.delegate_mut().take_selected_command()
                    });

                    // Handle it outside the update call
                    if let Some(item) = selected_item {
                        this.handle_command_or_file_selected(item, window, cx);
                    }

                    this.command_palette_open = false;
                    // Restore focus to the app so shortcuts work immediately
                    this.focus_handle.focus(window);
                    cx.notify();
                }).detach();

                // Focus the input for typing (actions bubble up to palette)
                let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
                input_handle.focus(window);

                self.command_palette = Some(palette);
            }
        } else {
            // Don't destroy the palette, just mark as closed for reuse
            // Restore focus to the app so shortcuts work immediately
            self.focus_handle.focus(window);
        }

        cx.notify();
    }

    fn handle_command_or_file_selected(
        &mut self,
        item: CommandOrFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use ui_common::command_palette::CommandOrFile;

        match item {
            CommandOrFile::Command(cmd) => {
                // Handle the command
                match cmd.command_type {
                    CommandType::Files => {
                        // This is now handled by the command palette switching to file mode
                        // Don't close the palette or show notifications
                        self.command_palette_open = true;
                        return;
                    }
                    CommandType::ToggleFileManager => {
                        self.toggle_drawer(window, cx);
                    }
                    CommandType::ToggleTerminal => {
                        self.toggle_terminal(window, cx);
                    }
                    CommandType::ToggleMultiplayer => {
                        self.toggle_multiplayer(window, cx);
                    }
                    CommandType::ToggleProblems => {
                        self.toggle_problems(window, cx);
                    }
                    CommandType::OpenSettings => {
                        cx.dispatch_action(&ui::OpenSettings);
                    }
                    CommandType::BuildProject => {
                        window.push_notification(
                            Notification::info("Build")
                                .message("Building project..."),
                            cx
                        );
                        // TODO: Implement build
                    }
                    CommandType::RunProject => {
                        window.push_notification(
                            Notification::info("Run")
                                .message("Running project..."),
                            cx
                        );
                        // TODO: Implement run
                    }
                    CommandType::RestartAnalyzer => {
                        self.rust_analyzer.update(cx, |analyzer, cx| {
                            analyzer.restart(window, cx);
                        });
                    }
                    CommandType::StopAnalyzer => {
                        self.rust_analyzer.update(cx, |analyzer, cx| {
                            analyzer.stop(window, cx);
                        });
                    }
                }
            }
            CommandOrFile::File(file) => {
                // Open the selected file
                self.open_path(file.path, window, cx);
            }
        }
    }

    fn on_navigate_to_diagnostic(
        &mut self,
        _drawer: &Entity<ProblemsDrawer>,
        event: &ui_problems::NavigateToDiagnostic,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        println!(
            "📂 Navigating to diagnostic: {:?} at line {}, column {}",
            event.file_path, event.line, event.column
        );

        // Open the file in the script editor
        self.open_script_tab(event.file_path.clone(), window, cx);

        // Navigate to the specific line and column
        if let Some(script_editor) = &self.script_editor {
            script_editor.update(cx, |editor, cx| {
                editor.go_to_line(event.line, event.column, window, cx);
            });
        }
    }

    /// Public handler for file selected events from external windows (like file manager window)
    /// This is called directly from the file manager window
    pub fn handle_file_selected_from_external_window(
        &mut self,
        event: &FileSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!(
            "File selected from external window - path: {:?}, type: {:?}",
            event.path, event.file_type
        );

        match event.file_type {
            FileType::Class => {
                tracing::error!("Blueprint editor tried to load from external window, but it's now plugin-only!");
                // Blueprint editor has been fully migrated to plugin system
            }
            FileType::Script => {
                tracing::debug!("Opening script tab from external window");
                self.open_script_tab(event.path.clone(), window, cx);
            }
            FileType::DawProject => {
                tracing::debug!("Opening DAW tab from external window: {:?}", event.path);
                self.open_daw_tab(event.path.clone(), window, cx);
            }
            FileType::Database => {
                tracing::debug!("Opening database tab from external window: {:?}", event.path);
                self.open_database_tab(event.path.clone(), window, cx);
            }
            FileType::StructType => {
                tracing::debug!("Opening struct editor from external window: {:?}", event.path);
                self.open_struct_tab(event.path.clone(), window, cx);
            }
            FileType::EnumType => {
                tracing::debug!("Opening enum editor from external window: {:?}", event.path);
                self.open_enum_tab(event.path.clone(), window, cx);
            }
            FileType::TraitType => {
                tracing::debug!("Opening trait editor from external window: {:?}", event.path);
                self.open_trait_tab(event.path.clone(), window, cx);
            }
            FileType::AliasType => {
                tracing::debug!("Opening alias editor from external window: {:?}", event.path);
                self.open_alias_tab(event.path.clone(), window, cx);
            }
            _ => {
                tracing::debug!("Unknown file type from external window, ignoring");
            }
        }
    }

    // Blueprint editor methods removed - now handled by plugin system

    fn on_show_type_picker_request(
        &mut self,
        editor: &Entity<ui_alias_editor::AliasEditor>,
        event: &ShowTypePickerRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Store which editor requested the picker so we can send the type back
        self.active_type_picker_editor = Some(editor.clone());

        // Show the palette with type library delegate
        if let Some(palette) = &self.command_palette {
            // Palette already exists, swap in type library delegate
            palette.update(cx, |palette, cx| {
                let delegate = AnyPaletteDelegate::type_library(event.target_slot.clone());
                palette.swap_delegate(delegate, window, cx);
            });

            // Focus the input for typing (actions bubble up to palette)
            let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
            input_handle.focus(window);
        } else {
            // Create the palette for the first time
            let delegate = AnyPaletteDelegate::type_library(event.target_slot.clone());
            let palette = cx.new(|cx| {
                GenericPalette::new(delegate, window, cx)
            });

            // Subscribe to dismiss events
            cx.subscribe_in(&palette, window, |this: &mut PulsarApp, palette, _event: &DismissEvent, window, cx| {
                // Extract the selected item
                let selected_item = palette.update(cx, |palette, _cx| {
                    palette.delegate_mut().take_selected_command()
                });

                // Check if we're in type picker mode
                let selected_type = palette.update(cx, |palette, _cx| {
                    palette.delegate_mut().take_selected_type()
                });

                // Handle command/file selection
                if let Some(item) = selected_item {
                    this.handle_command_or_file_selected(item, window, cx);
                }

                // Node selection removed - blueprint editor migrated to plugin system

                // Handle type selection
                if let Some((type_item, target_slot)) = selected_type {
                    if let Some(editor) = &this.active_type_picker_editor {
                        editor.update(cx, |ed, cx| {
                            ed.add_type_from_picker(&type_item, target_slot, cx);
                        });
                    }
                    this.active_type_picker_editor = None;
                }

                this.command_palette_open = false;
                // Restore focus to the app so shortcuts work immediately
                this.focus_handle.focus(window);
                cx.notify();
            }).detach();

            // Focus the input for typing (actions bubble up to palette)
            let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
            input_handle.focus(window);

            self.command_palette = Some(palette);
        }

        self.command_palette_open = true;
        cx.notify();
    }

    // open_engine_library_tab removed - blueprint system migrated to plugins

    /// Open or focus the script editor tab
    fn open_script_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Check if script editor already exists
        if let Some(script_editor) = &self.script_editor {
            // Script editor already exists, open the file in it
            script_editor.update(cx, |editor, cx| {
                editor.open_file(file_path, window, cx);
            });
            return;
        }

        // Create new script editor tab
        let script_editor = cx.new(|cx| ScriptEditorPanel::new(window, cx));

        // Wire up rust-analyzer to the script editor
        let analyzer = self.rust_analyzer.clone();
        script_editor.update(cx, |editor, cx| {
            editor.set_rust_analyzer(analyzer, cx);
        });

        // Load project in file explorer if we have a project path
        if let Some(ref project_path) = self.project_path {
            script_editor.update(cx, |editor, cx| {
                editor.set_project_path(project_path.clone(), window, cx);
            });
        }

        // Note: ScriptEditor now handles LSP notifications internally via set_rust_analyzer
        // We only subscribe here for non-LSP events (RunScriptRequested, etc.)
        cx.subscribe_in(&script_editor, window, Self::on_text_editor_event)
            .detach();

        // Open the specific file
        script_editor.update(cx, |editor, cx| {
            editor.open_file(file_path, window, cx);
        });

        // Add the tab to the tab panel
        self.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(script_editor.clone()), window, cx);
        });

        // Store the script editor reference
        self.script_editor = Some(script_editor);
    }

    /// Open a DAW editor tab for the given project path
    fn open_daw_tab(&mut self, project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("DEBUG: open_daw_tab called with path: {:?}", project_path);

        // For now, just check if we have any DAW editors open
        // TODO: Improve tracking to match by project path
        if !self.daw_editors.is_empty() {
            eprintln!("DEBUG: DAW editor already exists, focusing first one");
            // Just focus the first DAW tab for now
            // In a more complete implementation, we'd track which tab corresponds to which editor
            return;
        }

        eprintln!("DEBUG: Creating new DAW editor panel");
        self.next_tab_id += 1;

        // Create new DAW editor tab
        let daw_editor =
            cx.new(|cx| DawEditorPanel::new_with_project(project_path.clone(), window, cx));

        // Extract project name for tab label
        let _project_name = project_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("DAW")
            .to_string();

        eprintln!("DEBUG: Adding DAW editor to tab panel");
        // Add the tab to the tab panel
        self.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(daw_editor.clone()), window, cx);
        });

        eprintln!("DEBUG: Storing DAW editor reference");
        // Store the DAW editor reference
        self.daw_editors.push(daw_editor);

        eprintln!("DEBUG: DAW tab opened successfully");
    }

    /// Open a database editor tab for the given database path
    fn open_database_tab(&mut self, db_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("DEBUG: open_database_tab called with path: {:?}", db_path);

        // Check if a database editor for this path is already open
        let already_open = self
            .database_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .database_path
                    .as_ref()
                    .map(|p| p == &db_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            eprintln!("DEBUG: Database editor already exists, focusing it");
            // Focus the correct tab
            if let Some(editor_entity) = self.database_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        eprintln!("DEBUG: Creating new database editor");
        self.next_tab_id += 1;

        // Extract database name for tab label
        let db_name = db_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Database")
            .to_string();

        // Create new database editor tab
        let database_editor = cx.new(|cx| {
            ui_editor_table::DataTableEditor::open_database(db_path.clone(), window, cx)
                .unwrap_or_else(|e| {
                    eprintln!("Failed to open database: {}", e);
                    ui_editor_table::DataTableEditor::new(window, cx)
                })
        });

        eprintln!("DEBUG: Adding database editor to tab panel");
        // Add the tab to the tab panel
        self.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(database_editor.clone()), window, cx);
        });

        eprintln!("DEBUG: Storing database editor reference");
        // Store the database editor reference
        self.database_editors.push(database_editor);

        eprintln!("DEBUG: Database tab opened successfully");
    }

    /// Open a struct editor tab for the given struct file
    fn open_struct_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("DEBUG: open_struct_tab called with path: {:?}", file_path);

        // If it's a folder, look for struct.json inside
        let actual_file_path = if file_path.is_dir() {
            file_path.join("struct.json")
        } else {
            file_path.clone()
        };

        // Check if a struct editor for this path is already open
        let already_open = self
            .struct_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .file_path()
                    .map(|p| p == actual_file_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            eprintln!("DEBUG: Struct editor already exists, focusing it");
            if let Some(editor_entity) = self.struct_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        eprintln!("DEBUG: Creating new struct editor");
        self.next_tab_id += 1;

        // Create new struct editor tab
        let struct_editor = cx.new(|cx| {
            ui_struct_editor::StructEditor::new_with_file(actual_file_path.clone(), window, cx)
        });

        eprintln!("DEBUG: Adding struct editor to tab panel");
        self.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(struct_editor.clone()), window, cx);
        });

        eprintln!("DEBUG: Storing struct editor reference");
        self.struct_editors.push(struct_editor);

        eprintln!("DEBUG: Struct tab opened successfully");
    }

    /// Open an enum editor tab for the given enum file
    fn open_enum_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("DEBUG: open_enum_tab called with path: {:?}", file_path);

        // If it's a folder, look for enum.json inside
        let actual_file_path = if file_path.is_dir() {
            file_path.join("enum.json")
        } else {
            file_path.clone()
        };

        // Check if an enum editor for this path is already open
        let already_open = self
            .enum_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .file_path()
                    .map(|p| p == actual_file_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            eprintln!("DEBUG: Enum editor already exists, focusing it");
            if let Some(editor_entity) = self.enum_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        eprintln!("DEBUG: Creating new enum editor");
        self.next_tab_id += 1;

        // Create new enum editor tab
        let enum_editor = cx.new(|cx| {
            ui_enum_editor::EnumEditor::new_with_file(actual_file_path.clone(), window, cx)
        });

        eprintln!("DEBUG: Adding enum editor to tab panel");
        self.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(enum_editor.clone()), window, cx);
        });

        eprintln!("DEBUG: Storing enum editor reference");
        self.enum_editors.push(enum_editor);

        eprintln!("DEBUG: Enum tab opened successfully");
    }

    /// Open a trait editor tab for the given trait file
    fn open_trait_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("DEBUG: open_trait_tab called with path: {:?}", file_path);

        // If it's a folder, look for trait.json inside
        let actual_file_path = if file_path.is_dir() {
            file_path.join("trait.json")
        } else {
            file_path.clone()
        };

        // Check if a trait editor for this path is already open
        let already_open = self
            .trait_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .file_path()
                    .map(|p| p == actual_file_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            eprintln!("DEBUG: Trait editor already exists, focusing it");
            if let Some(editor_entity) = self.trait_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        eprintln!("DEBUG: Creating new trait editor");
        self.next_tab_id += 1;

        // Create new trait editor tab
        let trait_editor = cx.new(|cx| {
            ui_trait_editor::TraitEditor::new_with_file(actual_file_path.clone(), window, cx)
        });

        eprintln!("DEBUG: Adding trait editor to tab panel");
        self.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(trait_editor.clone()), window, cx);
        });

        eprintln!("DEBUG: Storing trait editor reference");
        self.trait_editors.push(trait_editor);

        eprintln!("DEBUG: Trait tab opened successfully");
    }

    /// Open an alias editor tab for the given alias file
    fn open_alias_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("DEBUG: open_alias_tab called with path: {:?}", file_path);

        // If it's a folder, look for alias.json inside
        let actual_file_path = if file_path.is_dir() {
            file_path.join("alias.json")
        } else {
            file_path.clone()
        };

        // Check if an alias editor for this path is already open
        let already_open = self
            .alias_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .file_path()
                    .map(|p| p == actual_file_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            eprintln!("DEBUG: Alias editor already exists, focusing it");
            if let Some(editor_entity) = self.alias_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        eprintln!("DEBUG: Creating new alias editor");
        self.next_tab_id += 1;

        // Create new alias editor tab
        let alias_editor = cx.new(|cx| {
            ui_alias_editor::AliasEditor::new_with_file(actual_file_path.clone(), window, cx)
        });

        // Subscribe to type picker requests
        cx.subscribe_in(&alias_editor, window, Self::on_show_type_picker_request)
            .detach();

        eprintln!("DEBUG: Adding alias editor to tab panel");
        self.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(alias_editor.clone()), window, cx);
        });

        eprintln!("DEBUG: Storing alias editor reference");
        self.alias_editors.push(alias_editor);

        eprintln!("DEBUG: Alias tab opened successfully");
    }

    fn on_text_editor_event(
        &mut self,
        _editor: &Entity<ScriptEditorPanel>,
        event: &ui_editor::TextEditorEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        use ui_editor::TextEditorEvent;

        match event {
            // LSP notifications are now handled by ScriptEditor internally
            // We only handle app-level events here
            TextEditorEvent::FileOpened { .. } => {
                // No-op: ScriptEditor handles didOpen
            }
            TextEditorEvent::FileSaved { .. } => {
                // No-op: ScriptEditor handles didSave
            }
            TextEditorEvent::FileClosed { .. } => {
                // No-op: ScriptEditor handles didClose
            }
            _ => {
                // Handle other events (RunScriptRequested, etc.) if needed
            }
        }
    }

    /// Open a path in the appropriate editor
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Try plugin system first for all file types
        match self.plugin_manager.create_editor_for_file(&path, window, cx) {
            Ok((panel, _editor_instance)) => {
                tracing::info!("Plugin system opened file: {:?}", path);
                self.center_tabs.update(cx, |tabs, cx| {
                    tabs.add_panel(panel, window, cx);
                });
                return;
            }
            Err(e) => {
                tracing::debug!("Plugin system couldn't open {:?}: {} - trying legacy handlers", path, e);
            }
        }

        // Legacy fallback for file types not yet migrated to plugins
        if let Some(extension) = path.extension() {
            match extension.to_str() {
                Some("pdaw") => {
                    self.open_daw_tab(path, window, cx);
                }
                Some("db") | Some("sqlite") | Some("sqlite3") => {
                    self.open_database_tab(path, window, cx);
                }
                Some("rs") | Some("js") | Some("ts") | Some("py") | Some("lua") => {
                    self.open_script_tab(path, window, cx);
                }
                _ => {}
            }
        }
    }
}

impl Focusable for PulsarApp {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PulsarApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Show welcome notification on first render if project was loaded
        if !self.shown_welcome_notification && self.project_path.is_some() {
            self.shown_welcome_notification = true;
            if let Some(ref path) = self.project_path {
                let project_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Project");
                window.push_notification(
                    Notification::info("Project Loaded")
                        .message(format!("Welcome to {}", project_name)),
                    cx
                );
            }
        }
        
        // Update rust-analyzer progress if indexing
        self.rust_analyzer.update(cx, |analyzer, cx| {
            analyzer.update_progress_from_thread(cx);
        });

        // Show entry screen if no project is loaded
        if let Some(screen) = &self.entry_screen {
            return screen.clone().into_any_element();
        }

        // Use the stored command palette entity - only show when open
        let command_palette = if self.command_palette_open {
            self.command_palette.clone()
        } else {
            None
        };

        let drawer_open = self.drawer_open;

        v_flex()
            .size_full()
            // NO BACKGROUND - allow viewport transparency
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_toggle_file_manager))
            .on_action(cx.listener(Self::on_toggle_problems))
            .on_action(cx.listener(Self::on_toggle_terminal))
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .child(
                // Main dock area
                div()
                    .flex_1()
                    .relative()
                    .child(self.dock_area.clone())
                    .when(drawer_open, |this| {
                        this.child(
                            // Overlay background for file manager
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .bg(Hsla::black().opacity(0.3))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|app, _, window, cx| {
                                        app.drawer_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            // File manager drawer at bottom
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .h(px(300.))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(self.file_manager_drawer.clone())
                                .with_animation(
                                    "slide-up",
                                    Animation::new(Duration::from_secs_f64(0.2)),
                                    |this, delta| this.bottom(px(-300.) + delta * px(300.)),
                                ),
                        )
                    }),
            )
            .child(
                // Footer with rust analyzer status and controls
                self.render_footer(drawer_open, cx),
            )
            // GenericPalette handles its own positioning, background, and overlay
            .children(command_palette)
            .into_any_element()
    }
}

impl PulsarApp {
    fn render_footer(&self, drawer_open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let analyzer = self.rust_analyzer.read(cx);
        let status = analyzer.status();
        let is_running = analyzer.is_running();

        let error_count = self
            .problems_drawer
            .read(cx)
            .count_by_severity(ui_problems::DiagnosticSeverity::Error);
        let warning_count = self
            .problems_drawer
            .read(cx)
            .count_by_severity(ui_problems::DiagnosticSeverity::Warning);

        // Get status colors
        let (status_color, status_icon) = match status {
            AnalyzerStatus::Ready => (cx.theme().success, IconName::CheckCircle),
            AnalyzerStatus::Indexing { .. } | AnalyzerStatus::Starting => {
                (cx.theme().warning, IconName::Loader)
            }
            AnalyzerStatus::Error(_) => (cx.theme().danger, IconName::TriangleAlert),
            AnalyzerStatus::Stopped => (cx.theme().muted_foreground, IconName::Circle),
            AnalyzerStatus::Idle => (cx.theme().muted_foreground, IconName::Circle),
        };

        // ✨ COMPLETELY REDESIGNED MODERN STATUS BAR ✨
        div()
            .w_full()
            .relative()
            // Progress bar overlay at top (only when indexing)
            .when(self.analyzer_progress > 0.0 && self.analyzer_progress < 1.0, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h(px(2.))
                        .w(relative(self.analyzer_progress))
                        .bg(cx.theme().primary)
                        .shadow_md(),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .h(px(28.))
                    .items_center()
                    .px_3()
                    .gap_2()
                    .bg(cx.theme().background)
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        // LEFT: Quick Actions with Icon-Only Compact Design
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Button::new("toggle-files")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::Folder)
                                            .size(px(16.))
                                            .text_color(if drawer_open {
                                                cx.theme().primary
                                            } else {
                                                cx.theme().muted_foreground
                                            })
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .when(drawer_open, |s| {
                                        s.bg(cx.theme().primary.opacity(0.15))
                                    })
                                    .tooltip("Toggle Files (Ctrl+B)")
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_drawer(window, cx);
                                    })),
                            )
                            .child(
                                // Problems indicator with badge
                                Button::new("toggle-problems")
                                    .ghost()
                                    .icon(
                                        Icon::new(if error_count > 0 {
                                            IconName::Close
                                        } else if warning_count > 0 {
                                            IconName::TriangleAlert
                                        } else {
                                            IconName::CheckCircle
                                        })
                                        .size(px(16.))
                                        .text_color(if error_count > 0 {
                                            cx.theme().danger
                                        } else if warning_count > 0 {
                                            cx.theme().warning
                                        } else {
                                            cx.theme().success
                                        })
                                    )
                                    .relative()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .when(error_count + warning_count > 0, |this| {
                                        this.child(
                                            // Badge with count
                                            div()
                                                .absolute()
                                                .top(px(-4.))
                                                .right(px(-4.))
                                                .min_w(px(16.))
                                                .h(px(16.))
                                                .px_1()
                                                .rounded(px(8.))
                                                .bg(if error_count > 0 {
                                                    cx.theme().danger
                                                } else {
                                                    cx.theme().warning
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_bold()
                                                        .text_color(rgb(0xFFFFFF))
                                                        .child(format!("{}", error_count + warning_count)),
                                                ),
                                        )
                                    })
                                    .tooltip(format!(
                                        "{} Errors, {} Warnings",
                                        error_count, warning_count
                                    ))
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_problems(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("toggle-terminal")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::Terminal)
                                            .size(px(16.))
                                            .text_color(cx.theme().muted_foreground)
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .tooltip("Terminal")
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_terminal(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("toggle-multiplayer")
                                    .ghost()
                                    .icon(
                                        Icon::new(IconName::User)
                                            .size(px(16.))
                                            .text_color(cx.theme().muted_foreground)
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .tooltip("Multiplayer Collaboration")
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.toggle_multiplayer(window, cx);
                                    })),
                            )
                            .child(
                                // Separator
                                div()
                                    .w(px(1.))
                                    .h(px(18.))
                                    .bg(cx.theme().border),
                            ),
                    )
                    .child(
                        // CENTER: Rust Analyzer Status - Sleek & Compact
                        h_flex()
                            .flex_1()
                            .items_center()
                            .gap_2()
                            .child(
                                // Status icon with subtle glow
                                Icon::new(status_icon)
                                    .size(px(14.))
                                    .text_color(status_color),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child("rust-analyzer"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(status_color)
                                    .child(self.analyzer_status_text.clone()),
                            )
                            .when(!self.analyzer_detail_message.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                                        .child("—"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.analyzer_detail_message.clone()),
                                )
                            })
                            .child(
                                // Analyzer controls - minimal
                                h_flex()
                                    .gap_0p5()
                                    .ml_2()
                                    .when(is_running, |this| {
                                        this.child(
                                            Button::new("analyzer-stop")
                                                .ghost()
                                                .icon(
                                                    Icon::new(IconName::X)
                                                        .size(px(12.))
                                                        .text_color(cx.theme().muted_foreground)
                                                )
                                                .p_1()
                                                .rounded(px(3.))
                                                .hover(|s| s.bg(cx.theme().danger.opacity(0.2)))
                                                .tooltip("Stop")
                                                .on_click(cx.listener(|app, _, window, cx| {
                                                    app.rust_analyzer.update(cx, |analyzer, cx| {
                                                        analyzer.stop(window, cx);
                                                    });
                                                })),
                                        )
                                    })
                                    .child(
                                        Button::new("analyzer-restart")
                                            .ghost()
                                            .icon(
                                                Icon::new(IconName::Undo)
                                                    .size(px(12.))
                                                    .text_color(cx.theme().muted_foreground)
                                            )
                                            .p_1()
                                            .rounded(px(3.))
                                            .tooltip(if is_running { "Restart" } else { "Start" })
                                            .on_click(cx.listener(move |app, _, window, cx| {
                                                if let Some(project) = app.project_path.clone() {
                                                    app.rust_analyzer.update(cx, |analyzer, cx| {
                                                        if is_running {
                                                            analyzer.restart(window, cx);
                                                        } else {
                                                            analyzer.start(project, window, cx);
                                                        }
                                                    });
                                                }
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        // Separator
                        div()
                            .w(px(1.))
                            .h(px(18.))
                            .bg(cx.theme().border),
                    )
                    .child(
                        // RIGHT: Project Name - Clean Display
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(14.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .children(
                                        self.project_path
                                            .as_ref()
                                            .and_then(|path| path.file_name())
                                            .map(|name| name.to_string_lossy().to_string())
                                            .or(Some("No Project".to_string())),
                                    ),
                            ),
                    ),
            )
    }
}

pub struct EditorPanel {
    editor_type: EditorType,
    focus_handle: FocusHandle,
}

impl EditorPanel {
    pub fn new(editor_type: EditorType, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            editor_type,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn view(editor_type: EditorType, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(editor_type, window, cx))
    }
}

impl Panel for EditorPanel {
    fn panel_name(&self) -> &'static str {
        self.editor_type.display_name()
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div()
            .child(self.editor_type.display_name())
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for EditorPanel {}

impl Render for EditorPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_editor_content(cx)
    }
}

impl EditorPanel {
    fn render_editor_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Header
                h_flex()
                    .w_full()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .justify_between()
                    .items_center()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(self.editor_type.display_name()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.editor_type.description()),
                            ),
                    ),
            )
            .child(
                // Content
                div()
                    .flex_1()
                    .p_4()
                    .overflow_hidden()
                    .child(self.render_specific_editor(cx)),
            )
    }

    fn render_specific_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.editor_type {
            EditorType::Level => self.render_level_editor(cx).into_any_element(),
            EditorType::Script => self.render_script_editor(cx).into_any_element(),
            EditorType::Daw => self.render_placeholder_editor(cx).into_any_element(),
            // Blueprint editor migrated to plugin system
        }
    }

    fn render_level_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .gap_4()
            .child(
                // Left panel - Scene Hierarchy
                div()
                    .w_64()
                    .h_full()
                    .bg(cx.theme().sidebar)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .p_3()
                    .child("Scene Hierarchy"),
            )
            .child(
                // Center - 3D Viewport
                div()
                    .flex_1()
                    .h_full()
                    .bg(cx.theme().muted.opacity(0.2))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .size_16()
                                    .bg(cx.theme().primary.opacity(0.2))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child("🎮"),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("3D Viewport"),
                            ),
                    ),
            )
            .child(
                // Right panel - Properties
                div()
                    .w_64()
                    .h_full()
                    .bg(cx.theme().sidebar)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .p_3()
                    .child("Properties"),
            )
    }

    fn render_script_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .gap_4()
            .child(
                // Left panel - File Explorer
                div()
                    .w_64()
                    .h_full()
                    .bg(cx.theme().sidebar)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .p_3()
                    .child("File Explorer"),
            )
            .child(
                // Center - Code Editor
                div()
                    .flex_1()
                    .h_full()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .p_4()
                    .child("Code Editor Area"),
            )
            .child(
                // Right panel - Output/Terminal
                div()
                    .w_64()
                    .h_full()
                    .bg(cx.theme().sidebar)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .p_3()
                    .child("Terminal"),
            )
    }

    // render_blueprint_editor removed - blueprint system migrated to plugins

    fn render_placeholder_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex_1().flex().items_center().justify_center().child(
            v_flex()
                .items_center()
                .gap_4()
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(format!("{} Editor", self.editor_type.display_name())),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Coming soon..."),
                ),
        )
    }
}



