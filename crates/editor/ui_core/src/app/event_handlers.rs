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

use engine_backend::services::{AnalyzerEvent, AnalyzerStatus, RustAnalyzerManager};
use futures::FutureExt;
use plugin_manager;
use rust_i18n::t;
use smol::Timer;
use std::path::PathBuf;
use std::time::Duration;

use super::PulsarApp;

mod analyzer_diagnostics;

/// Status-bar text for an analyzer status: `(status text, detail message,
/// progress 0..=1)`. Kept separate from the GPUI handler so it is testable.
fn status_summary(status: &AnalyzerStatus) -> (&'static str, String, f32) {
    match status {
        AnalyzerStatus::Idle => ("Idle", String::new(), 0.0),
        AnalyzerStatus::Starting => (
            "Starting...",
            "Initializing language server".to_string(),
            0.0,
        ),
        AnalyzerStatus::Indexing { progress, message } => ("Indexing", message.clone(), *progress),
        AnalyzerStatus::Ready => ("Ready", "Code intelligence active".to_string(), 1.0),
        AnalyzerStatus::Error(e) => ("Error", e.to_string(), 0.0),
        AnalyzerStatus::Stopped => ("Stopped", String::new(), 0.0),
    }
}

/// Store the analyzer's status text, detail and progress for the footer.
fn set_analyzer_state(app: &mut PulsarApp, text: &str, detail: String, progress: f32) {
    app.state.analyzer_status_text = text.to_string();
    app.state.analyzer_detail_message = detail;
    app.state.analyzer_progress = progress;
}

/// Show an error notification titled "Analyzer error".
fn notify_analyzer_error(message: String, window: &mut Window, cx: &mut Context<PulsarApp>) {
    window.push_notification(
        ui::notification::Notification::error(t!("Notification.Title.AnalyzerError").to_string())
            .message(message),
        cx,
    );
}

/// Dispatch a rust-analyzer event to its handler; each kind has one function.
pub fn on_analyzer_event(
    app: &mut PulsarApp,
    _manager: &Entity<RustAnalyzerManager>,
    event: &AnalyzerEvent,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    tracing::debug!(
        "🎯 on_analyzer_event CALLED with event type: {}",
        match event {
            AnalyzerEvent::StatusChanged(_) => "StatusChanged",
            AnalyzerEvent::Ready => "Ready",
            AnalyzerEvent::Error(_) => "Error",
            AnalyzerEvent::Diagnostics(_) => "Diagnostics",
            AnalyzerEvent::FileDiagnostics { .. } => "FileDiagnostics",
            AnalyzerEvent::IndexingProgress { .. } => "IndexingProgress",
        }
    );

    match event {
        AnalyzerEvent::StatusChanged(status) => on_status_changed(app, status, window, cx),
        AnalyzerEvent::IndexingProgress { progress, message } => {
            on_indexing_progress(app, *progress, message, cx)
        }
        AnalyzerEvent::Ready => on_ready(app, window, cx),
        AnalyzerEvent::Error(e) => on_error(app, &e.to_string(), window, cx),
        // Per-file sets drive the open code editors directly; the problems
        // drawer keeps working from the aggregate `Diagnostics` event.
        AnalyzerEvent::FileDiagnostics { .. } => {}
        AnalyzerEvent::Diagnostics(diagnostics) => {
            analyzer_diagnostics::on_diagnostics(app, diagnostics, cx)
        }
    }
}

/// The analyzer moved to a new lifecycle status: update the footer, and
/// notify when it became ready or failed.
fn on_status_changed(
    app: &mut PulsarApp,
    status: &AnalyzerStatus,
    window: &mut Window,
    cx: &mut Context<PulsarApp>,
) {
    let (text, detail, progress) = status_summary(status);
    set_analyzer_state(app, text, detail, progress);
    match status {
        AnalyzerStatus::Ready => {
            window.push_notification(
                ui::notification::Notification::success(
                    t!("Notification.Title.RustAnalyzerReady").to_string(),
                )
                .message(t!("Notification.Message.CodeIntelligenceAvailable").to_string()),
                cx,
            );
        }
        AnalyzerStatus::Error(e) => notify_analyzer_error(e.to_string(), window, cx),
        _ => {}
    }
    // Only the footer shows the analyzer status.
    app.notify_status_bar(cx);
}

/// Indexing progress changed: update the footer only.
fn on_indexing_progress(
    app: &mut PulsarApp,
    progress: f32,
    message: &str,
    cx: &mut Context<PulsarApp>,
) {
    set_analyzer_state(app, "Indexing", message.to_string(), progress);
    // Only the footer shows the analyzer status.
    app.notify_status_bar(cx);
}

/// Indexing finished: mark ready and tell the user.
fn on_ready(app: &mut PulsarApp, window: &mut Window, cx: &mut Context<PulsarApp>) {
    let (text, detail, progress) = status_summary(&AnalyzerStatus::Ready);
    set_analyzer_state(app, text, detail, progress);
    window.push_notification(
        ui::notification::Notification::success(
            t!("Notification.Title.CodeIntelligenceReady").to_string(),
        )
        .message(t!("Notification.Message.RustAnalyzerIndexingComplete").to_string()),
        cx,
    );
    // Only the footer shows the analyzer status.
    app.notify_status_bar(cx);
}

/// The analyzer reported an error: show it in the footer and as a notification.
fn on_error(app: &mut PulsarApp, message: &str, window: &mut Window, cx: &mut Context<PulsarApp>) {
    set_analyzer_state(app, "Error", message.to_string(), 0.0);
    notify_analyzer_error(message.to_string(), window, cx);
    // Only the footer shows the analyzer status.
    app.notify_status_bar(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_summary_describes_each_lifecycle_state() {
        assert_eq!(
            status_summary(&AnalyzerStatus::Idle),
            ("Idle", String::new(), 0.0)
        );
        assert_eq!(
            status_summary(&AnalyzerStatus::Ready),
            ("Ready", "Code intelligence active".to_string(), 1.0)
        );
        assert_eq!(
            status_summary(&AnalyzerStatus::Indexing {
                progress: 0.5,
                message: "crate x".into()
            }),
            ("Indexing", "crate x".to_string(), 0.5)
        );
        assert_eq!(
            status_summary(&AnalyzerStatus::Error("boom".into())),
            ("Error", "boom".to_string(), 0.0)
        );
    }
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
