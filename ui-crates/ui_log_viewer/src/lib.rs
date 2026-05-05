//! Mission Control - Engine monitoring and logging interface

mod atomic_memory_tracking;
mod caller_tracking;
mod gpu_engines;
mod gpu_info;
mod live_logs;
mod log_drawer_v2;
mod log_reader;
mod mem_details;
mod memory_database;
mod memory_tracking;
mod panels;
mod performance_metrics;
mod system_info;
pub mod tracking_allocator;
mod type_tracking;

pub use atomic_memory_tracking::{AllocationEntry, SizeBucket, ATOMIC_MEMORY_COUNTERS};
pub use live_logs::{publish_live_log, subscribe_live_logs};
pub use log_drawer_v2::LogDrawer;
pub use memory_tracking::{
    create_memory_tracker, MemoryCategory, MemoryStatsSnapshot, MemoryTracker, SharedMemoryTracker,
};
pub use panels::{
    AdvancedMetricsPanel, CallerSitesPanel, GpuMetricsPanel, LogsPanel, MemoryBreakdownPanel,
    ResourceMonitorPanel, SystemInfoPanel,
};
pub use performance_metrics::{
    create_shared_metrics, PerformanceMetrics, SharedPerformanceMetrics,
};
pub use system_info::{create_shared_info, SharedSystemInfo, SystemInfo};
pub use tracking_allocator::{
    disable_tracking, enable_tracking, is_tracking_active, MemoryCategoryGuard, TrackingAllocator,
};
pub use type_tracking::{AllocationSite, TYPE_TRACKER};

use gpui::*;
use ui::{dock::DockItem, h_flex, v_flex, workspace::Workspace, ActiveTheme, TitleBar};

/// Mission Control - Main panel with workspace layout
pub struct MissionControlPanel {
    focus_handle: FocusHandle,
    log_drawer: Entity<LogDrawer>,
    workspace: Option<Entity<Workspace>>,
    metrics: SharedPerformanceMetrics,
    system_info: SharedSystemInfo,
    memory_tracker: SharedMemoryTracker,
    _metrics_task: Option<Task<()>>,
}

impl MissionControlPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let log_drawer = cx.new(LogDrawer::new);
        let metrics = create_shared_metrics();
        let system_info = create_shared_info();
        let memory_tracker = create_memory_tracker();

        // Using atomic counters now - no need to connect tracker

        Self {
            focus_handle: cx.focus_handle(),
            log_drawer,
            workspace: None,
            metrics,
            system_info,
            memory_tracker,
            _metrics_task: None,
        }
    }

    /// Start monitoring the log file and metrics
    pub fn start_monitoring(&mut self, cx: &mut Context<Self>) {
        if self._metrics_task.is_some() {
            return;
        }

        self.log_drawer.update(cx, |drawer, cx| {
            drawer.start_monitoring(cx);
        });

        // Standard UI reactivity pattern used elsewhere: an entity-owned task
        // updates state through `cx.update` and then notifies that entity.
        let task = cx.spawn(async move |this, cx| loop {
            smol::Timer::after(std::time::Duration::from_secs(1)).await;

            let _ = cx.update(|cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |panel, cx| {
                        panel.metrics.write().update_system_metrics();
                        cx.notify();
                    });
                }
            });
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
                cx,
            )
        });

        let log_drawer = self.log_drawer.clone();
        let metrics = self.metrics.clone();
        let system_info = self.system_info.clone();
        let memory_tracker = self.memory_tracker.clone();

        workspace.update(cx, |workspace, cx| {
            let dock_area = workspace.dock_area().downgrade();

            // Create logs panel for center
            let logs_panel = cx.new(|cx| {
                LogsPanel::new(log_drawer.clone(), cx)
            });

            // Create memory breakdown panel for center
            let memory_panel = cx.new(|cx| {
                MemoryBreakdownPanel::new(memory_tracker.clone(), metrics.clone(), cx)
            });

            // Create advanced metrics panel for center
            let advanced_panel = cx.new(|cx| {
                AdvancedMetricsPanel::new(metrics.clone(), cx)
            });

            // Create GPU metrics panel for center
            let gpu_panel = cx.new(|cx| {
                GpuMetricsPanel::new(metrics.clone(), system_info.clone(), cx)
            });

            // Create caller sites panel for center
            let callers_panel = cx.new(|cx| {
                CallerSitesPanel::new(window, cx)
            });
            let resource_panel = cx.new(|cx| {
                ResourceMonitorPanel::new(metrics.clone(), cx)
            });

            // Create system info panel for right bottom
            let system_info_panel = cx.new(|cx| {
                SystemInfoPanel::new(system_info.clone(), cx)
            });

            // Center: Logs | Memory | CPU | GPU | Callers tabs
            let center_tabs = DockItem::tabs(
                vec![
                    std::sync::Arc::new(logs_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                    std::sync::Arc::new(memory_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                    std::sync::Arc::new(advanced_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                    std::sync::Arc::new(gpu_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                    std::sync::Arc::new(callers_panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                ],
                Some(0), // Default to logs tab
                &dock_area,
                window,
                cx,
            );

            // Right: Resource monitor (top) + System info (bottom) split vertically
            let resource_tabs = DockItem::tabs(
                vec![std::sync::Arc::new(resource_panel) as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            let system_info_tabs = DockItem::tabs(
                vec![std::sync::Arc::new(system_info_panel) as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            let right_split = ui::dock::DockItem::split_with_sizes(
                gpui::Axis::Vertical,
                vec![resource_tabs, system_info_tabs],
                vec![None, Some(px(350.0))], // Charts flexible, system info 350px
                &dock_area,
                window,
                cx,
            );

            // Initialize workspace with center and right (split) panels
            workspace.initialize(
                center_tabs,
                None, // No left dock
                Some(right_split), // Right dock split between charts and system info
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

        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                // Title bar
                TitleBar::new().child(
                    h_flex().flex_1().items_center().px_4().child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Mission Control"),
                    ),
                ),
            )
            .child(if let Some(ref workspace) = self.workspace {
                workspace.clone().into_any_element()
            } else {
                div().child("Loading Mission Control...").into_any_element()
            })
    }
}

impl window_manager::PulsarWindow for MissionControlPanel {
    type Params = ();

    fn window_name() -> &'static str {
        "MissionControlPanel"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1920.0, 1080.0)
    }

    fn build(_: (), _window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        let panel = cx.new(MissionControlPanel::new);
        panel.update(cx, |p, cx| p.start_monitoring(cx));
        panel
    }
}

/// Type alias for use in the PulsarWindow system.
pub type LogViewerWindow = MissionControlPanel;
