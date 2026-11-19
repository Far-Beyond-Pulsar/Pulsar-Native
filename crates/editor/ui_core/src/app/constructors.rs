//! Constructor methods for PulsarApp
//!
//! The public `new*` constructors all funnel into [`PulsarApp::new_internal`],
//! which is a short list of named steps defined in `construction.rs` (and
//! `command_palette_setup.rs` for the palette).

use gpui::{Context, Window};
use std::path::PathBuf;

use super::command_palette_setup::register_command_palette;
use super::construction::{
    build_center_dock, build_drawers, connect_language_servers, create_dock_area,
    create_plugin_manager, finish_construction, install_docks, persistent_center_tabs,
    publish_component_event_catalog, publish_project_path, register_drawer_windows,
    spawn_multiuser_refresh, spawn_task_queue_refresh, subscribe_drawer_events,
    sync_file_types_into_drawer, sync_project_into_drawers,
};
use super::PulsarApp;

impl PulsarApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_internal(None, true, window, cx)
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
        Self::new_internal(Some(project_path), true, window, cx)
    }

    /// Builds the app by running the steps in `construction.rs` in order; the
    /// order matters (each step's doc names what it depends on).
    pub(super) fn new_internal(
        project_path: Option<PathBuf>,
        create_level_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let t_total = std::time::Instant::now();
        tracing::info!("[PulsarApp] new_internal start");

        publish_component_event_catalog(cx);

        // Dock area, then the project path BEFORE the level editor is built.
        let dock_area = create_dock_area(window, cx);
        let weak_dock = dock_area.downgrade();
        publish_project_path(project_path.as_deref());

        let center_dock_item = build_center_dock(create_level_editor, &weak_dock, window, cx);
        let center_tabs = persistent_center_tabs(&center_dock_item, cx);
        install_docks(&dock_area, center_dock_item, &center_tabs, window, cx);

        // Store project_path before moving it
        let has_project = project_path.is_some();

        let drawers = build_drawers(project_path.as_ref(), window, cx);
        register_drawer_windows(&drawers, cx);

        // Script problems from Play-in-Editor (#854, #868) join the
        // language server's in the problems panel.
        super::script_problems::watch(drawers.problems_drawer.clone(), cx);
        // Classes that are two languages' (the host's one-language rule).
        super::class_problems::watch(drawers.problems_drawer.clone(), project_path.clone(), cx);

        subscribe_drawer_events(
            &drawers.file_manager_drawer,
            &drawers.problems_drawer,
            window,
            cx,
        );
        let language_servers = connect_language_servers(project_path.as_ref(), window, cx);

        // Initialize palette manager global
        ui_common::command_palette::PaletteManager::init(cx);

        let t_plugins = std::time::Instant::now();
        let (plugin_manager, plugins_staged) = create_plugin_manager(&language_servers, t_plugins);

        let multiuser_refresh_task = spawn_multiuser_refresh(cx);

        let mut app = Self {
            state: crate::app::state::AppState {
                dock_area,
                project_path,
                file_manager_drawer: drawers.file_manager_drawer,
                chrome: Default::default(),
                problems_drawer: drawers.problems_drawer,
                type_debugger_drawer: drawers.type_debugger_drawer,
                task_queue_refresh_task: None,
                center_tabs,
                preview_tab_panels: Vec::new(),
                language_servers,
                shown_welcome_notification: false,
                command_palette_open: false,
                command_palette_id: None,
                command_palette: None,
                command_palette_view: None,
                project_switcher_open: false,
                project_switcher_view: None,
                focus_handle: cx.focus_handle(),
                navigation: Default::default(),
                nav_sidebar: drawers.nav_sidebar,
                nav_overlay: drawers.nav_overlay,
                status_bar: drawers.status_bar,
                _multiuser_refresh_task: Some(multiuser_refresh_task),
                layout_persist: false,
                layout_slot: None,
                window_bounds: None,
                window_restore_bounds: None,
                layout_ready: false,
                layout_save_task: None,
                layout_dormant: Vec::new(),
                layout_window: None,
                layout_closing: false,
                layout_popouts: Default::default(),
                layout_ghosts: Vec::new(),
                radial: super::radial_menu::RadialHost::new(cx),
            },
        };

        app.state.task_queue_refresh_task = Some(spawn_task_queue_refresh(cx));
        sync_file_types_into_drawer(&mut app, cx);
        if has_project {
            sync_project_into_drawers(&mut app, cx);
        }

        // Update Discord presence with initial tab if project is loaded
        if has_project && create_level_editor {
            app.update_discord_presence(cx);
        }

        register_command_palette(&mut app, window, cx);

        tracing::info!(
            "[PulsarApp] new_internal total: {}ms",
            t_total.elapsed().as_millis()
        );

        finish_construction(
            &mut app,
            plugin_manager,
            plugins_staged,
            has_project,
            window,
            cx,
        );

        app
    }

    /// Get the current workspace root
    pub fn workspace_root(&self) -> Option<&PathBuf> {
        self.state.project_path.as_ref()
    }
}
