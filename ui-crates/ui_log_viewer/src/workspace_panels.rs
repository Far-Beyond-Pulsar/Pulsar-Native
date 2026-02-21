//! Workspace panels for Mission Control

use gpui::*;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use crate::log_drawer_v2::LogDrawer;
use crate::performance_metrics::SharedPerformanceMetrics;
use crate::system_info::SharedSystemInfo;
use crate::memory_tracking::SharedMemoryTracker;

/// Logs Panel - Main log viewer in the center
pub struct LogsPanel {
    log_drawer: Entity<LogDrawer>,
    focus_handle: FocusHandle,
}

impl LogsPanel {
    pub fn new(log_drawer: Entity<LogDrawer>, cx: &mut Context<Self>) -> Self {
        Self {
            log_drawer,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for LogsPanel {}

impl Render for LogsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.log_drawer.clone())
    }
}

impl Focusable for LogsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for LogsPanel {
    fn panel_name(&self) -> &'static str {
        "logs"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Logs".into_any_element()
    }
}

/// Resource Monitor Panel - Performance and resource metrics
pub struct ResourceMonitorPanel {
    focus_handle: FocusHandle,
    metrics: SharedPerformanceMetrics,
}

impl ResourceMonitorPanel {
    pub fn new(metrics: SharedPerformanceMetrics, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            metrics,
        }
    }
}

impl EventEmitter<PanelEvent> for ResourceMonitorPanel {}

impl Render for ResourceMonitorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::prelude::FluentBuilder;
        use ui::{h_flex, chart::AreaChart, scroll::ScrollbarAxis};
        let theme = cx.theme().clone();

        // Read current metrics
        let metrics = self.metrics.read();
        let current_cpu = metrics.current_cpu;
        let current_memory_mb = metrics.current_memory_mb;
        let current_gpu = metrics.current_gpu;
        let current_fps = metrics.current_fps;

        // Clone data for charts
        let cpu_data: Vec<_> = metrics.cpu_history.iter().cloned().collect();
        let memory_data: Vec<_> = metrics.memory_history.iter().cloned().collect();
        let gpu_data: Vec<_> = metrics.gpu_history.iter().cloned().collect();
        let fps_data: Vec<_> = metrics.fps_history.iter().cloned().collect();

        drop(metrics);

        // Request continuous updates
        cx.notify();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_4()
            .gap_4()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                // Header
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("System Resources")
                    )
            )
            .child(
                // CPU Usage
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(6.0))
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.muted_foreground)
                                    .child("CPU Usage")
                            )
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.info)
                                    .child(format!("{:.1}%", current_cpu))
                            )
                    )
                    .when(!cpu_data.is_empty(), |this| {
                        this.child(
                            div()
                                .h(px(60.0))
                                .w_full()
                                .child(
                                    AreaChart::<_, SharedString, f64>::new(cpu_data.clone())
                                        .x(|d| format!("{}", d.index).into())
                                        .y(|d| d.usage)
                                        .stroke(theme.info)
                                        .fill(theme.info.opacity(0.15))
                                        .linear()
                                        .tick_margin(0)
                                        .max_y_range(100.0)
                                )
                        )
                    })
            )
            .child(
                // Memory Usage
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(6.0))
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.muted_foreground)
                                    .child("Memory Usage")
                            )
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.warning)
                                    .child(format!("{:.0} MB", current_memory_mb))
                            )
                    )
                    .when(!memory_data.is_empty(), |this| {
                        this.child(
                            div()
                                .h(px(60.0))
                                .w_full()
                                .child(
                                    AreaChart::<_, SharedString, f64>::new(memory_data.clone())
                                        .x(|d| format!("{}", d.index).into())
                                        .y(|d| d.memory_mb)
                                        .stroke(theme.warning)
                                        .fill(theme.warning.opacity(0.15))
                                        .linear()
                                        .tick_margin(0)
                                )
                        )
                    })
            )
            .child(
                // GPU Usage
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(6.0))
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.muted_foreground)
                                    .child("GPU Memory")
                            )
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.success)
                                    .child(format!("{:.1}%", current_gpu))
                            )
                    )
                    .when(!gpu_data.is_empty(), |this| {
                        this.child(
                            div()
                                .h(px(60.0))
                                .w_full()
                                .child(
                                    AreaChart::<_, SharedString, f64>::new(gpu_data.clone())
                                        .x(|d| format!("{}", d.index).into())
                                        .y(|d| d.usage)
                                        .stroke(theme.success)
                                        .fill(theme.success.opacity(0.15))
                                        .linear()
                                        .tick_margin(0)
                                        .max_y_range(100.0)
                                )
                        )
                    })
            )
            .child(
                // FPS
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(6.0))
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.muted_foreground)
                                    .child("Frame Rate")
                            )
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.accent)
                                    .child(if current_fps > 0.0 {
                                        format!("{:.0} FPS", current_fps)
                                    } else {
                                        "N/A".to_string()
                                    })
                            )
                    )
                    .when(!fps_data.is_empty(), |this| {
                        this.child(
                            div()
                                .h(px(60.0))
                                .w_full()
                                .child(
                                    AreaChart::<_, SharedString, f64>::new(fps_data.clone())
                                        .x(|d| format!("{}", d.index).into())
                                        .y(|d| d.fps)
                                        .stroke(theme.accent)
                                        .fill(theme.accent.opacity(0.15))
                                        .linear()
                                        .tick_margin(0)
                                )
                        )
                    })
            )
    }
}

impl Focusable for ResourceMonitorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ResourceMonitorPanel {
    fn panel_name(&self) -> &'static str {
        "resource_monitor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Resources".into_any_element()
    }
}

/// System Info Panel - Comprehensive system specifications
pub struct SystemInfoPanel {
    focus_handle: FocusHandle,
    system_info: SharedSystemInfo,
}

impl SystemInfoPanel {
    pub fn new(system_info: SharedSystemInfo, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            system_info,
        }
    }

    fn info_row(
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
        cx: &App,
    ) -> impl IntoElement {
        use ui::h_flex;
        let theme = cx.theme();

        h_flex()
            .w_full()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(label.into())
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(value.into())
            )
    }

    fn section_header(title: impl Into<SharedString>, cx: &App) -> impl IntoElement {
        let theme = cx.theme();

        div()
            .w_full()
            .text_size(px(12.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.accent)
            .pb_2()
            .child(title.into())
    }
}

impl EventEmitter<PanelEvent> for SystemInfoPanel {}

impl Render for SystemInfoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, scroll::ScrollbarAxis};
        let theme = cx.theme().clone();

        let info = self.system_info.read();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_4()
            .gap_3()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                // Header
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("System Information")
                    )
            )
            .child(
                // Operating System Section
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(Self::section_header("Operating System", cx))
                    .child(Self::info_row("OS", &info.os_name, cx))
                    .child(Self::info_row("Version", &info.os_version, cx))
                    .child(Self::info_row("Kernel", &info.kernel_version, cx))
                    .child(Self::info_row("Hostname", &info.host_name, cx))
                    .child(Self::info_row("Uptime", info.uptime_formatted(), cx))
            )
            .child(
                // CPU Section
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(Self::section_header("Processor", cx))
                    .child(Self::info_row("Model", &info.cpu_brand, cx))
                    .child(Self::info_row("Vendor", &info.cpu_vendor, cx))
                    .child(Self::info_row("Cores", format!("{} cores", info.cpu_cores), cx))
                    .child(Self::info_row("Frequency", format!("{} MHz", info.cpu_frequency), cx))
            )
            .child(
                // Memory Section
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(Self::section_header("Memory", cx))
                    .child(Self::info_row("Total RAM", format!("{:.2} GB", info.total_memory_gb()), cx))
                    .child(Self::info_row("Total Swap", format!("{:.2} GB", info.total_swap_gb()), cx))
            )
            .child(
                // GPU Section
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(Self::section_header("Graphics", cx))
                    .child(Self::info_row("GPU", &info.gpu_name, cx))
                    .child(Self::info_row("Vendor", &info.gpu_vendor, cx))
                    .child(Self::info_row("Driver", &info.gpu_driver_version, cx))
            )
            .child(
                // Engine Information
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(Self::section_header("Engine", cx))
                    .child(Self::info_row("Renderer", "Helio (D3D12)", cx))
                    .child(Self::info_row("Backend", "Blade", cx))
            )
    }
}

impl Focusable for SystemInfoPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SystemInfoPanel {
    fn panel_name(&self) -> &'static str {
        "system_info"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "System Info".into_any_element()
    }
}

/// Memory Breakdown Panel - Real-time memory allocation tracking
pub struct MemoryBreakdownPanel {
    focus_handle: FocusHandle,
    memory_tracker: SharedMemoryTracker,
}

impl MemoryBreakdownPanel {
    pub fn new(memory_tracker: SharedMemoryTracker, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            memory_tracker,
        }
    }

    fn category_row(
        category_name: &str,
        size_bytes: usize,
        percentage: f64,
        color: Hsla,
        cx: &App,
    ) -> impl IntoElement {
        use ui::h_flex;
        let theme = cx.theme();

        let size_mb = size_bytes as f64 / 1024.0 / 1024.0;

        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(category_name.to_string())
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{:.2} MB", size_mb))
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(color)
                                    .child(format!("{:.1}%", percentage))
                            )
                    )
            )
            .child(
                // Progress bar
                div()
                    .w_full()
                    .h(px(6.0))
                    .bg(theme.border)
                    .rounded(px(3.0))
                    .child(
                        div()
                            .h_full()
                            .w(relative(percentage as f32 / 100.0))
                            .bg(color)
                            .rounded(px(3.0))
                    )
            )
    }
}

impl EventEmitter<PanelEvent> for MemoryBreakdownPanel {}

impl Render for MemoryBreakdownPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, scroll::ScrollbarAxis};
        let theme = cx.theme().clone();

        let tracker = self.memory_tracker.read();
        let stats = tracker.stats();
        let stats_guard = stats.read();

        let total_current = stats_guard.current_usage;
        let breakdown = stats_guard.category_breakdown();

        drop(stats_guard);
        drop(tracker);

        // Request continuous updates
        cx.notify();

        // Color palette for categories
        let colors = vec![
            theme.chart_1,
            theme.chart_2,
            theme.chart_3,
            theme.chart_4,
            theme.chart_5,
            theme.info,
            theme.warning,
            theme.success,
        ];

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_4()
            .gap_4()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                // Header
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Memory Breakdown")
                    )
            )
            .child(
                // Summary Stats
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(6.0))
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme.muted_foreground)
                                    .child("Current Usage")
                            )
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.accent)
                                    .child(format!("{:.2} MB", total_current as f64 / 1024.0 / 1024.0))
                            )
                    )
            )
            .child(
                // Category Breakdown
                v_flex()
                    .w_full()
                    .gap_3()
                    .children(
                        breakdown.iter().enumerate().map(|(idx, (category, size))| {
                            let percentage = if total_current > 0 {
                                (*size as f64 / total_current as f64) * 100.0
                            } else {
                                0.0
                            };
                            let color = colors[idx % colors.len()];

                            Self::category_row(
                                category.as_str(),
                                *size,
                                percentage,
                                color,
                                cx,
                            )
                        })
                    )
            )
            .child(
                // Info text
                div()
                    .mt_4()
                    .text_size(px(10.0))
                    .text_color(theme.muted_foreground.opacity(0.7))
                    .child("Real-time memory tracking via global allocator hooks")
            )
    }
}

impl Focusable for MemoryBreakdownPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for MemoryBreakdownPanel {
    fn panel_name(&self) -> &'static str {
        "memory_breakdown"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Memory".into_any_element()
    }
}
