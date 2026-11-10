//! The named steps `PulsarApp::new_internal` runs to build the app.
//!
//! `new_internal` (in `constructors.rs`) calls these in order; each doc says
//! what the step sets up and which earlier step it depends on. The command
//! palette has its own file (`command_palette_setup.rs`).

use engine_backend::services::RustAnalyzerManager;
use gpui::{AppContext, Context, Entity, Task, WeakEntity, Window};
use plugin_manager::PluginManager;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::dock::{DockArea, DockItem, TabPanel};
use ui_file_manager::FileManagerDrawer;
use ui_level_editor::LevelEditorPanel;
use ui_problems::ProblemsDrawer;
use ui_type_debugger::TypeDebuggerDrawer;

use super::{
    agent_chat_panel::AgentChatPanel, event_handlers, manual_tool_panel::ManualToolPanel, PulsarApp,
};

/// The entities `build_drawers` creates; they become fields of `AppState`.
pub(super) struct Drawers {
    pub file_manager_drawer: Entity<FileManagerDrawer>,
    pub nav_sidebar: Entity<super::nav_sidebar::NavSidebar>,
    pub nav_overlay: Entity<super::nav_sidebar::NavSidebarOverlay>,
    pub status_bar: Entity<super::status_bar::StatusBar>,
    pub problems_drawer: Entity<ProblemsDrawer>,
    pub type_debugger_drawer: Entity<TypeDebuggerDrawer>,
}

/// Publishes the component-event catalog as a global. Depends on nothing.
///
/// Component events are registered in the host executable. Publish
/// their declarations through the shared editor API so Blueprint and
/// other dynamically loaded editors do not need their own inventory
/// copy (which cannot see host registrations across a DLL boundary).
pub(super) fn publish_component_event_catalog(cx: &mut Context<PulsarApp>) {
    let mut component_events: Vec<_> = pulsar_world_registry::component_event_registrations()
        .map(|registration| plugin_editor_api::ComponentEventMetadata {
            component_class: registration.class_name.to_owned(),
            event: (registration.declaration)(),
        })
        .collect();
    component_events.sort_by(|a, b| {
        (&a.component_class, &a.event.name).cmp(&(&b.component_class, &b.event.name))
    });
    component_events
        .dedup_by(|a, b| a.component_class == b.component_class && a.event.name == b.event.name);
    cx.set_global(plugin_editor_api::ComponentEventCatalog {
        events: component_events,
    });
}

/// Creates the empty main `DockArea`. Depends on nothing; the center and left
/// docks are filled by the next steps.
pub(super) fn create_dock_area(
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) -> Entity<DockArea> {
    let t = std::time::Instant::now();
    let dock_area = cx.new(|cx| DockArea::new("main-dock", Some(1), window, cx));
    tracing::info!("[PulsarApp] dock area: {}ms", t.elapsed().as_millis());
    dock_area
}

/// Tells `engine_state` (and the profiler) which project this window edits.
///
/// Must run BEFORE [`build_center_dock`] so that `ensure_default_level_file()`
/// can resolve the project root while the level editor is constructed.
pub(super) fn publish_project_path(project_path: Option<&Path>) {
    if let Some(path) = project_path {
        engine_state::set_project_path(path.to_string_lossy().into_owned());
        tracing::info!("Set engine project path to {:?}", path);
        // Let other editors' profilers list and record this one (cheap
        // until a viewer starts recording; once per process).
        profiling::remote::publish_process(profiling::remote::TargetDescription {
            kind: "editor".into(),
            name: "Pulsar Editor".into(),
            project: path.display().to_string(),
        });
    }
}

/// Builds the center dock item: a tab strip holding the level editor, or an
/// empty one for secondary windows. Depends on [`create_dock_area`] (for the
/// weak handle) and [`publish_project_path`].
pub(super) fn build_center_dock(
    create_level_editor: bool,
    window_id: Option<u64>,
    weak_dock: &WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) -> DockItem {
    let t = std::time::Instant::now();
    let center_dock_item = if create_level_editor {
        let level_editor = if let Some(wid) = window_id {
            cx.new(|cx| LevelEditorPanel::new_with_window_id(wid, window, cx))
        } else {
            cx.new(|cx| LevelEditorPanel::new(window, cx))
        };
        DockItem::tabs(
            vec![Arc::new(level_editor.clone())],
            Some(0),
            weak_dock,
            window,
            cx,
        )
    } else {
        DockItem::tabs(vec![], None, weak_dock, window, cx)
    };
    tracing::info!(
        "[PulsarApp] level editor panel: {}ms",
        t.elapsed().as_millis()
    );
    center_dock_item
}

/// Extracts the center tab panel from `center_dock_item` and makes its tabs
/// persistent. Depends on [`build_center_dock`].
pub(super) fn persistent_center_tabs(
    center_dock_item: &DockItem,
    cx: &mut Context<PulsarApp>,
) -> Entity<TabPanel> {
    let center_tabs = match center_dock_item {
        DockItem::Tabs { view, .. } => view.clone(),
        _ => {
            tracing::error!("Expected Tabs dock item for center, got different type");
            panic!("Invalid dock configuration: center must be Tabs type");
        }
    };
    // The editor keeps its tab strip with a single tab, and the last tab stays open.
    center_tabs.update(cx, |tabs, cx| tabs.set_persistent_tabs(true, cx));
    center_tabs
}

/// Creates the agent-chat and manual-tool side panels and installs both the
/// center and left docks into `dock_area`. Depends on [`create_dock_area`],
/// [`build_center_dock`] and [`persistent_center_tabs`].
pub(super) fn install_docks(
    dock_area: &Entity<DockArea>,
    center_dock_item: DockItem,
    center_tabs: &Entity<TabPanel>,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    let weak_dock = dock_area.downgrade();
    let t = std::time::Instant::now();
    let agent_chat_panel =
        cx.new(|cx| AgentChatPanel::new(dock_area.clone(), center_tabs.clone(), window, cx));
    let manual_tool_panel =
        cx.new(|cx| ManualToolPanel::new(dock_area.clone(), center_tabs.clone(), window, cx));
    let left_dock = DockItem::tabs(
        vec![
            Arc::new(agent_chat_panel) as Arc<dyn ui::dock::PanelView>,
            Arc::new(manual_tool_panel) as Arc<dyn ui::dock::PanelView>,
        ],
        Some(0),
        &weak_dock,
        window,
        cx,
    );

    dock_area.update(cx, |dock, cx| {
        dock.set_center(center_dock_item, window, cx);
        dock.set_left_dock(left_dock, Some(gpui::px(420.0)), false, window, cx);
    });
    tracing::info!(
        "[PulsarApp] left dock + layout: {}ms",
        t.elapsed().as_millis()
    );
}

/// Creates the file manager, sidebar, footer, problems and type-debugger
/// entities. Depends on nothing but the app entity itself
/// (the sidebar and footer hold a weak handle back to it).
pub(super) fn build_drawers(
    project_path: Option<&PathBuf>,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) -> Drawers {
    let t = std::time::Instant::now();
    let file_manager_drawer =
        cx.new(|cx| FileManagerDrawer::new(project_path.cloned(), window, cx));
    let nav_sidebar = {
        let app = cx.weak_entity();
        let drawer = file_manager_drawer.clone();
        cx.new(|cx| {
            let mut sidebar =
                super::nav_sidebar::NavSidebar::new(app, drawer, project_path.cloned());
            sidebar.observe_drawer(cx);
            sidebar
        })
    };
    let nav_overlay = {
        let sidebar = nav_sidebar.clone();
        cx.new(|_| super::nav_sidebar::NavSidebarOverlay::new(sidebar))
    };
    let status_bar = {
        let app = cx.weak_entity();
        cx.new(|_| super::status_bar::StatusBar::new(app))
    };
    let problems_drawer = cx.new(|cx| ProblemsDrawer::new(window, cx));
    let type_debugger_drawer = cx.new(|cx| TypeDebuggerDrawer::new(window, cx));
    tracing::info!("[PulsarApp] drawers: {}ms", t.elapsed().as_millis());
    Drawers {
        file_manager_drawer,
        nav_sidebar,
        nav_overlay,
        status_bar,
        problems_drawer,
        type_debugger_drawer,
    }
}

/// Registers entity-capturing openers so the window registry can open the
/// problems and type-debugger windows without callers knowing the concrete
/// types or holding the entities. Depends on [`build_drawers`].
pub(super) fn register_drawer_windows(drawers: &Drawers, cx: &mut Context<PulsarApp>) {
    use gpui::UpdateGlobal as _;
    use ui_common::PulsarWindowExt as _;

    let pd = drawers.problems_drawer.clone();
    window_manager::WindowRegistry::update_global(cx, move |reg, _| {
        reg.register("ProblemsWindow", move |cx| {
            ui_problems::ProblemsWindow::open(pd.clone(), cx);
        });
    });

    let td = drawers.type_debugger_drawer.clone();
    window_manager::WindowRegistry::update_global(cx, move |reg, _| {
        reg.register("TypeDebuggerWindow", move |cx| {
            ui_type_debugger::TypeDebuggerWindow::open(td.clone(), cx);
        });
    });
}

/// Wires the drawers' events to the app's handlers. Depends on
/// [`build_drawers`].
pub(super) fn subscribe_drawer_events(
    file_manager_drawer: &Entity<FileManagerDrawer>,
    problems_drawer: &Entity<ProblemsDrawer>,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    cx.subscribe_in(
        file_manager_drawer,
        window,
        event_handlers::on_file_selected,
    )
    .detach();
    cx.subscribe_in(
        file_manager_drawer,
        window,
        event_handlers::on_popout_file_manager,
    )
    .detach();
    cx.subscribe_in(
        file_manager_drawer,
        window,
        event_handlers::on_dock_file_manager,
    )
    .detach();
    cx.subscribe_in(file_manager_drawer, window, event_handlers::on_drag_event)
        .detach();
    // Keep the folder the drawer shows, so the layout remembers it.
    cx.observe(file_manager_drawer, |app, drawer, cx| {
        let folder = drawer
            .read(cx)
            .selected_folder()
            .map(std::path::Path::to_path_buf);
        if app.state.chrome.file_manager_folder != folder {
            app.state.chrome.file_manager_folder = folder;
            app.chrome_changed(cx);
        }
    })
    .detach();
    cx.subscribe_in(
        problems_drawer,
        window,
        event_handlers::on_navigate_to_diagnostic,
    )
    .detach();
}

/// Uses the shared rust-analyzer when one is passed, otherwise creates one
/// (started when there is a project), and subscribes the app to its events.
/// Depends on nothing; the plugin manager step needs its result.
pub(super) fn connect_rust_analyzer(
    shared_rust_analyzer: Option<Entity<RustAnalyzerManager>>,
    project_path: Option<&PathBuf>,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) -> Entity<RustAnalyzerManager> {
    // Create rust analyzer manager or use shared one
    let rust_analyzer = if let Some(shared_analyzer) = shared_rust_analyzer {
        shared_analyzer
    } else {
        let analyzer = cx.new(|cx| RustAnalyzerManager::new(window, cx));

        // Start rust analyzer if we have a project
        if let Some(project) = project_path {
            analyzer.update(cx, |analyzer, cx| {
                analyzer.start(project.clone(), window, cx);
            });
        }

        analyzer
    };

    // Subscribe to analyzer events
    cx.subscribe_in(&rust_analyzer, window, event_handlers::on_analyzer_event)
        .detach();
    rust_analyzer
}

/// Takes the manager the splash screen staged (or makes a fresh one), gives
/// it the editor services and registers the built-in editors. Returns the
/// manager and whether it was staged (so plugins are already installed).
/// Depends on [`connect_rust_analyzer`].
pub(super) fn create_plugin_manager(
    rust_analyzer: &Entity<RustAnalyzerManager>,
    t_plugins: std::time::Instant,
) -> (PluginManager, bool) {
    tracing::info!("[PulsarApp] plugin init start");
    // The splash screen normally installed the plugins already; without it
    // (compat path) they load in the background below.
    let staged_manager = plugin_manager::take_staged_manager();
    let plugins_staged = staged_manager.is_some();
    let mut plugin_manager = staged_manager.unwrap_or_else(PluginManager::new);
    // Every editor -- built-in or a plugin library -- is created with a handle
    // to the engine's one language server and its one set of undo
    // histories (one per document, Pulsar-Native#931).
    plugin_manager.set_editor_services(plugin_editor_api::EditorServices {
        language_server: Some(rust_analyzer.clone()),
        undo: plugin_editor_api::UndoService::new(),
    });

    // Register built-in editors
    crate::register_all_builtin_editors(plugin_manager.builtin_registry_mut());
    plugin_manager.register_builtin_editors();
    tracing::info!(
        "[PulsarApp] built-in editors registered in {:?}",
        t_plugins.elapsed()
    );
    (plugin_manager, plugins_staged)
}

/// Starts the task that repaints the footer when the multiuser session
/// changes. Depends on the global `EngineContext` existing (checked when the
/// task first runs).
pub(super) fn spawn_multiuser_refresh(cx: &mut Context<PulsarApp>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let multiuser = engine_state::EngineContext::global()
            .expect("EngineContext not initialized")
            .multiuser
            .clone();
        loop {
            multiuser.changed().await;
            // Only the footer shows the session.
            _ = this.update(cx, |app, cx| app.notify_status_bar(cx));
        }
    })
}

/// Starts the task that repaints the task button when queue state changes.
/// The task window owns its own refresh task while it is open. The queue is
/// editor-only; game/runtime work is never routed through it.
pub(super) fn spawn_task_queue_refresh(cx: &mut Context<PulsarApp>) -> Task<()> {
    let task_queue = editor_task_queue::global().clone();
    cx.spawn(async move |this, cx| {
        let mut revision = task_queue.revision();
        loop {
            smol::Timer::after(std::time::Duration::from_millis(250)).await;
            let next_revision = task_queue.revision();
            if next_revision != revision {
                revision = next_revision;
                _ = this.update(cx, |app, cx| app.notify_status_bar(cx));
            }
        }
    })
}

/// Gives the file manager drawer the file types the global plugin manager
/// knows. Depends on `app.state.file_manager_drawer` and the plugin manager
/// having been published as the global.
pub(super) fn sync_file_types_into_drawer(app: &mut PulsarApp, cx: &mut Context<PulsarApp>) {
    // Update file manager drawer with registered file types from plugin manager
    let file_types: Vec<plugin_editor_api::FileTypeDefinition> =
        if let Some(pm_lock) = plugin_manager::global() {
            pm_lock
                .read()
                .file_type_registry()
                .get_all_file_types()
                .into_iter()
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

    app.state.file_manager_drawer.update(cx, |drawer, cx| {
        drawer.update_file_types(file_types);
        cx.notify();
    });
}

/// Pushes the project's user types and root path into the type-debugger and
/// problems drawers. Only meaningful when a project is open; depends on
/// `app.state` holding the drawers and the project path.
pub(super) fn sync_project_into_drawers(app: &mut PulsarApp, cx: &mut Context<PulsarApp>) {
    // Sync UserTypeRegistry to UI if we have a project
    if let Some(engine_state) = engine_state::EngineContext::global() {
        if let Some(user_types) = engine_state.user_types() {
            let types = user_types.all();
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

/// Final wiring once the app struct exists: focus, radial menu, tab-preview
/// handlers, open-editor snapshot, layout persistence and the background
/// plugin load. Depends on every step above (it reads the finished `app`).
pub(super) fn finish_construction(
    app: &mut PulsarApp,
    plugin_manager: PluginManager,
    plugins_staged: bool,
    has_project: bool,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    // Focus this app's handle so menus built before any editor interaction have
    // a valid action_context that routes back to PulsarApp's .on_action() handlers.
    // Without this, window.focused(cx) returns None when the first menu is opened,
    // action_context is never set, and menu item actions dispatch from PopupMenu's
    // own focus chain — which is a sibling of PulsarApp, not a descendant.
    app.state.focus_handle.focus(window, cx);
    app.install_radial_menu(window, cx);

    app.install_tab_preview_handlers(window, cx);
    let dock_area = app.state.dock_area.clone();
    cx.subscribe_in(
        &dock_area,
        window,
        |app, _, event: &ui::dock::DockEvent, window, cx| {
            if matches!(event, ui::dock::DockEvent::LayoutChanged) {
                app.install_tab_preview_handlers(window, cx);
            }
        },
    )
    .detach();

    // Populate the open-editor snapshot so the AI can see tabs that were created
    // during construction (e.g. the default level editor) without waiting for a
    // tab-change or file-open event.
    app.refresh_open_editor_snapshot(cx);

    // Restore the project's saved tab layout and keep saving it.
    if has_project {
        app.init_layout_persistence(window, cx);
    }

    // Plugins load in the background; the layout restores once they have.
    app.start_plugin_load(plugin_manager, plugins_staged, window, cx);
}
