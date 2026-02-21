//! Mission Control - Engine monitoring and logging interface

mod log_drawer_v2;
mod log_reader;
mod workspace_panels;
mod performance_metrics;

pub use log_drawer_v2::LogDrawer;
pub use workspace_panels::{LogsPanel, ResourceMonitorPanel};
pub use performance_metrics::{PerformanceMetrics, SharedPerformanceMetrics, create_shared_metrics};

use gpui::*;
use ui::{
    dock::DockItem,
    workspace::Workspace,
    v_flex, ActiveTheme,
};

/// Mission Control - Main panel with workspace layout
pub struct MissionControlPanel {
    focus_handle: FocusHandle,
    log_drawer: Entity<LogDrawer>,
    workspace: Option<Entity<Workspace>>,
    metrics: SharedPerformanceMetrics,
    _metrics_task: Option<Task<()>>,
}

impl MissionControlPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let log_drawer = cx.new(|cx| LogDrawer::new(cx));
        let metrics = create_shared_metrics();

        Self {
            focus_handle: cx.focus_handle(),
            log_drawer,
            workspace: None,
            metrics,
            _metrics_task: None,
        }
    }

    /// Start monitoring the log file and metrics
    pub fn start_monitoring(&mut self, cx: &mut Context<Self>) {
        self.log_drawer.update(cx, |drawer, cx| {
            drawer.start_monitoring(cx);
        });

        // Start metrics collection task
        let metrics = self.metrics.clone();
        let task = cx.background_executor().spawn(async move {
            loop {
                // Update system metrics every second
                smol::Timer::after(std::time::Duration::from_secs(1)).await;

                let mut m = metrics.write();
                m.update_system_metrics();

                // Try to get render metrics from engine
                if let Some(_engine_context) = engine_state::EngineContext::global() {
                    // Try to get renderer metrics
                    // For now we'll use placeholder values, but this can be connected to actual renderer
                    // when we have access to the GPU renderer stats
                }
            }
        });

        self._metrics_task = Some(task);
    }

    fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }

        let workspace = cx.new(|cx| {
            Workspace::new_with_channel(
                "mission-control-workspace",
                ui::dock::DockChannel(4), // Different channel from level editor (3)
                window,
                cx
            )
        });

        let log_drawer = self.log_drawer.clone();
        let metrics = self.metrics.clone();

        workspace.update(cx, |workspace, cx| {
            let dock_area = workspace.dock_area().downgrade();

            // Create logs panel for center
            let logs_panel = cx.new(|cx| {
                LogsPanel::new(log_drawer.clone(), cx)
            });

            // Create resource monitor panel for right
            let resource_panel = cx.new(|cx| {
                ResourceMonitorPanel::new(metrics.clone(), cx)
            });

            // Center: Logs panel
            let center_tabs = DockItem::tabs(
                vec![std::sync::Arc::new(logs_panel) as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Right: Resource monitor panel
            let right_tabs = DockItem::tabs(
                vec![std::sync::Arc::new(resource_panel) as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Initialize workspace with center and right panels
            workspace.initialize(
                center_tabs,
                None, // No left dock
                Some(right_tabs), // Right dock for resources
                None, // No bottom dock
                window,
                cx,
            );
        });

        self.workspace = Some(workspace);
    }
}

impl Focusable for MissionControlPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MissionControlPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.initialize_workspace(window, cx);

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                if let Some(ref workspace) = self.workspace {
                    workspace.clone().into_any_element()
                } else {
                    div().child("Loading Mission Control...").into_any_element()
                }
            )
    }
}
