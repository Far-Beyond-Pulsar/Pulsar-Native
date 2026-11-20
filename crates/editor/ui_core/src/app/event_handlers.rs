//! Event handler implementations

use gpui::{AppContext, Context, Entity, UpdateGlobal, Window};
use ui::{dock::PanelEvent, ContextModal};
use ui_file_manager::{
    DockFileManagerEvent, FileManagerDrawer, FileSelected, PopoutFileManagerEvent,
};
use ui_problems::ProblemsDrawer;

// Note: These event handlers need plugin-based architecture to decouple from core UI.
// Current limitation: Hard-coded dependencies should be replaced with event-driven
// plugin system when the plugin architecture is finalized.

use plugin_manager;
use pulsar_lsp::{
    DiagnosticsEvent, DiagnosticsStore, LanguageServers, LanguageServersEvent, ServerEvent,
    ServerId,
};
use rust_i18n::t;

use super::PulsarApp;

mod diagnostics_feed;

/// What the footer and the toasts call a server: its display name.
fn server_name(servers: &Entity<LanguageServers>, id: &ServerId, cx: &gpui::App) -> String {
    servers
        .read(cx)
        .server(id, cx)
        .map(|server| server.read(cx).spec().display_name.to_string())
        .unwrap_or_else(|| id.server.to_string())
}

/// A language server's event: a toast for the moments a person needs to
/// know about (ready, failed); the footer reads every server's state itself,
/// so it is only repainted.
pub fn on_language_servers_event(
    app: &mut PulsarApp,
    servers: &Entity<LanguageServers>,
    event: &LanguageServersEvent,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    if let LanguageServersEvent::Server { id, event } = event {
        let name = server_name(servers, id, cx);
        match event {
            ServerEvent::Ready => {
                window.push_notification(
                    ui::notification::Notification::success(
                        t!("Notification.Title.LanguageServerReady", name = name).to_string(),
                    )
                    .message(t!("Notification.Message.CodeIntelligenceAvailable").to_string()),
                    cx,
                );
            }
            ServerEvent::Error(message) => {
                window.push_notification(
                    ui::notification::Notification::error(
                        t!("Notification.Title.LanguageServerError").to_string(),
                    )
                    .message(format!(
                        "{name}: {message}
{}",
                        t!("Notification.Message.LanguageServerSeeFooter")
                    )),
                    cx,
                );
            }
            ServerEvent::StatusChanged(_)
            | ServerEvent::Progress { .. }
            | ServerEvent::Stopped(_) => {}
        }
    }
    // Only the footer shows a server's status.
    app.notify_status_bar(cx);
}

/// The diagnostics store changed: show the source's complete set in the
/// Problems drawer (a fixed file's entries go with it).
pub fn on_diagnostics_event(
    app: &mut PulsarApp,
    store: &Entity<DiagnosticsStore>,
    event: &DiagnosticsEvent,
    _window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    let DiagnosticsEvent::Changed { source, .. } = event;
    diagnostics_feed::refresh_source(app, store, &source.label(), cx);
    app.notify_status_bar(cx);
}

pub fn on_tab_panel_event(
    app: &mut PulsarApp,
    _tabs: &Entity<ui::dock::TabPanel>,
    event: &PanelEvent,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    tracing::trace!(
        "[PANEL_EVENT] Received event: {:?}",
        match event {
            PanelEvent::MoveToNewWindow {
                position,
                source_index,
                ..
            } => {
                format!(
                    "MoveToNewWindow at {:?}, from index {}",
                    position, source_index
                )
            }
            PanelEvent::TabClosed(id) => format!("TabClosed({:?})", id),
            PanelEvent::TabChanged { active_index, .. } => format!("TabChanged({})", active_index),
            PanelEvent::ZoomIn => "ZoomIn".to_string(),
            PanelEvent::ZoomOut => "ZoomOut".to_string(),
            PanelEvent::LayoutChanged => "LayoutChanged".to_string(),
        }
    );

    match event {
        PanelEvent::MoveToNewWindow { .. } => {
            // This event is now handled by DockArea's subscription, which has proper source info
            // No need to handle it here anymore
            tracing::trace!("[PANEL_EVENT] MoveToNewWindow is now handled by DockArea");
        }
        PanelEvent::TabClosed(_entity_id) => {
            // Keep engine-level open-editor snapshot in sync.
            app.refresh_open_editor_snapshot(cx);
            // Update Discord presence when tab is closed
            app.update_discord_presence(cx);
        }
        PanelEvent::TabChanged {
            active_index: _,
            previous_index,
        } => {
            if let Some(previous_index) = previous_index {
                if let Some(panel) = _tabs.read(cx).all_panels().get(*previous_index).cloned() {
                    app.capture_panel_preview(panel, window, cx);
                }
            }
            // Keep engine-level open-editor snapshot in sync.
            app.refresh_open_editor_snapshot(cx);
            // Update Discord presence when active tab changes
            app.update_discord_presence(cx);
        }
        PanelEvent::ZoomIn | PanelEvent::ZoomOut | PanelEvent::LayoutChanged => {
            // These events are handled by the dock system, do nothing here
        }
    }
}

pub fn on_file_selected(
    app: &mut PulsarApp,
    _drawer: &Entity<FileManagerDrawer>,
    event: &FileSelected,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    tracing::debug!("Opening file: {:?}", event.path);

    // Open via plugin system
    app.open_path(event.path.clone(), window, cx);

    if !app.state.chrome.drawer_docked {
        app.state.chrome.drawer_open = false;
    }
    app.update_discord_presence(cx);
    cx.notify();
}

pub fn on_dock_file_manager(
    app: &mut PulsarApp,
    _drawer: &Entity<FileManagerDrawer>,
    _event: &DockFileManagerEvent,
    _window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    app.state.chrome.drawer_docked = true;
    app.state.chrome.drawer_open = false;
    app.chrome_changed(cx);
    cx.notify();
}

pub fn on_popout_file_manager(
    app: &mut PulsarApp,
    drawer: &Entity<FileManagerDrawer>,
    event: &PopoutFileManagerEvent,
    _window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    use engine_state::WindowRequest;
    use gpui::{px, size, Bounds, Point, WindowBounds, WindowKind, WindowOptions};
    use ui::Root;

    // Get project path from the drawer
    let project_path = drawer.read(cx).project_path.clone();

    app.state.chrome.drawer_docked = false;
    app.state.chrome.drawer_open = false;
    app.chrome_changed(cx);
    cx.notify();

    // Use mouse position from event for window placement
    let window_origin = Point {
        x: event.position.x - px(500.0), // Center window at mouse
        y: event.position.y - px(350.0),
    };

    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: window_origin,
            size: size(px(1000.0), px(700.0)),
        })),
        titlebar: None,
        kind: WindowKind::Normal,
        is_resizable: true,
        window_decorations: Some(gpui::WindowDecorations::Client),
        window_min_size: Some(gpui::Size {
            width: px(600.),
            height: px(400.),
        }),
        ..Default::default()
    };

    let _ = window_manager::WindowManager::update_global(cx, |wm, cx| {
        wm.create_window(
            WindowRequest::FileManager {
                project_path: project_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
            },
            opts,
            move |window, cx| {
                let new_drawer =
                    cx.new(|cx| FileManagerDrawer::new_in_window(project_path.clone(), window, cx));

                // Propagate registered file types from plugin manager
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
                new_drawer.update(cx, |d, cx| {
                    d.update_file_types(file_types);
                    cx.notify();
                });

                let file_manager_window =
                    cx.new(|cx| ui_file_manager::FileManagerWindow::new(new_drawer, window, cx));
                cx.new(|cx| Root::new(file_manager_window.into(), window, cx))
            },
            cx,
        )
    });
}

pub fn on_navigate_to_diagnostic(
    app: &mut PulsarApp,
    _drawer: &Entity<ProblemsDrawer>,
    event: &ui_problems::NavigateToDiagnostic,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    tracing::debug!(
        "📂 Navigating to diagnostic: {:?} at line {}, column {}",
        event.file_path,
        event.line,
        event.column
    );

    app.open_path(event.file_path.clone(), window, cx);
}

pub fn on_drag_event(
    app: &mut PulsarApp,
    _drawer: &Entity<FileManagerDrawer>,
    event: &ui_types_common::DragEvent,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    match event {
        ui_types_common::DragEvent::AssetDragStarted(_payload) => {
            tracing::debug!("Asset drag started, closing drawer");
            // Close the drawer and set flag to suppress reopening
            app.state.chrome.suppress_drawer_for_drag = true;
            app.close_drawer(window, cx);
            // The drawer can only report the drag ending while it renders,
            // and it does not render while closed, so a drop elsewhere (the
            // viewport) would leave it closed for good. Watch for the end
            // here and bring it back, showing the folder it was in.
            cx.spawn_in(window, async move |this, cx| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let ended = this
                    .update_in(cx, |app, window, cx| {
                        if cx.has_active_drag() {
                            return false;
                        }
                        app.state.chrome.suppress_drawer_for_drag = false;
                        if !app.state.chrome.drawer_docked {
                            app.open_drawer(window, cx);
                        }
                        true
                    })
                    .unwrap_or(true);
                if ended {
                    break;
                }
            })
            .detach();
        }
        ui_types_common::DragEvent::AssetDropped(_payload) => {
            tracing::debug!("Asset dropped, clearing suppress flag");
            // Clear the suppress flag but keep drawer closed
            app.state.chrome.suppress_drawer_for_drag = false;
        }
        ui_types_common::DragEvent::AssetDragCancelled => {
            tracing::debug!("Asset drag cancelled, reopening drawer");
            // Clear the suppress flag and reopen drawer
            app.state.chrome.suppress_drawer_for_drag = false;
            app.open_drawer(window, cx);
        }
    }
}
