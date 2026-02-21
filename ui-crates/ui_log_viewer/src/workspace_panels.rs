//! Workspace panels for Mission Control

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use crate::log_drawer_v2::LogDrawer;
use crate::performance_metrics::SharedPerformanceMetrics;
use crate::system_info::SharedSystemInfo;
use crate::memory_tracking::SharedMemoryTracker;
use smol::Timer;

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
        let current_vram_used_mb = metrics.current_vram_used_mb;
        let current_fps = metrics.current_fps;
        let current_net_rx_kbps = metrics.current_net_rx_kbps;
        let current_net_tx_kbps = metrics.current_net_tx_kbps;
        let current_disk_read_kbps = metrics.current_disk_read_kbps;
        let current_disk_write_kbps = metrics.current_disk_write_kbps;

        // Clone data for charts
        let cpu_data: Vec<_> = metrics.cpu_history.iter().cloned().collect();
        let memory_data: Vec<_> = metrics.memory_history.iter().cloned().collect();
        let gpu_data: Vec<_> = metrics.gpu_history.iter().cloned().collect();
        let fps_data: Vec<_> = metrics.fps_history.iter().cloned().collect();
        let net_rx_data: Vec<_> = metrics.net_rx_history.iter().cloned().collect();
        let net_tx_data: Vec<_> = metrics.net_tx_history.iter().cloned().collect();
        let disk_read_data: Vec<_> = metrics.disk_read_history.iter().cloned().collect();
        let disk_write_data: Vec<_> = metrics.disk_write_history.iter().cloned().collect();

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
                                    .child("GPU VRAM Used")
                            )
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.success)
                                    .child(if current_vram_used_mb > 0.0 {
                                        format!("{:.0} MB", current_vram_used_mb)
                                    } else {
                                        "N/A".to_string()
                                    })
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
                                        .y(|d| d.vram_used_mb)
                                        .stroke(theme.success)
                                        .fill(theme.success.opacity(0.15))
                                        .linear()
                                        .tick_margin(0)
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
            .child(Self::io_chart_card(
                "Network In",
                if current_net_rx_kbps >= 1024.0 {
                    format!("{:.1} MB/s", current_net_rx_kbps / 1024.0)
                } else {
                    format!("{:.0} KB/s", current_net_rx_kbps)
                },
                net_rx_data, |d: &crate::performance_metrics::NetDataPoint| d.kbps,
                theme.info, cx,
            ))
            .child(Self::io_chart_card(
                "Network Out",
                if current_net_tx_kbps >= 1024.0 {
                    format!("{:.1} MB/s", current_net_tx_kbps / 1024.0)
                } else {
                    format!("{:.0} KB/s", current_net_tx_kbps)
                },
                net_tx_data, |d: &crate::performance_metrics::NetDataPoint| d.kbps,
                theme.warning, cx,
            ))
            .child(Self::io_chart_card(
                "Disk Read",
                if current_disk_read_kbps >= 1024.0 {
                    format!("{:.1} MB/s", current_disk_read_kbps / 1024.0)
                } else {
                    format!("{:.0} KB/s", current_disk_read_kbps)
                },
                disk_read_data, |d: &crate::performance_metrics::DiskDataPoint| d.kbps,
                theme.success, cx,
            ))
            .child(Self::io_chart_card(
                "Disk Write",
                if current_disk_write_kbps >= 1024.0 {
                    format!("{:.1} MB/s", current_disk_write_kbps / 1024.0)
                } else {
                    format!("{:.0} KB/s", current_disk_write_kbps)
                },
                disk_write_data, |d: &crate::performance_metrics::DiskDataPoint| d.kbps,
                theme.danger, cx,
            ))
    }
}

impl ResourceMonitorPanel {
    /// Generic mini chart card used for Network/Disk rows.
    fn io_chart_card<D: Clone + 'static>(
        label: &'static str,
        value_str: String,
        data: Vec<D>,
        y_fn: impl Fn(&D) -> f64 + 'static,
        color: gpui::Hsla,
        cx: &App,
    ) -> impl IntoElement {
        use ui::h_flex;
        use ui::chart::AreaChart;
        use gpui::prelude::FluentBuilder;
        let theme = cx.theme().clone();
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
                            .child(label)
                    )
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(color)
                            .child(value_str)
                    )
            )
            .when(!data.is_empty(), |this| {
                this.child(
                    div()
                        .h(px(40.0))
                        .w_full()
                        .child(
                            AreaChart::<_, SharedString, f64>::new(data)
                                .x(move |_d| "".into())
                                .y(y_fn)
                                .stroke(color)
                                .fill(color.opacity(0.15))
                                .linear()
                                .tick_margin(0)
                        )
                )
            })
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
                    .child(Self::info_row("VRAM", info.gpu_vram_formatted(), cx))
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
    scroll_handle: ui::VirtualListScrollHandle,
    cached_entries: Vec<crate::AllocationEntry>,
    cached_total: usize,
    last_update: std::time::Instant,
}

impl MemoryBreakdownPanel {
    pub fn new(_memory_tracker: SharedMemoryTracker, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: ui::VirtualListScrollHandle::new(),
            cached_entries: Vec::new(),
            cached_total: 0,
            last_update: std::time::Instant::now(),
        }
    }
}

impl EventEmitter<PanelEvent> for MemoryBreakdownPanel {}

impl Render for MemoryBreakdownPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, v_virtual_list};

        let theme = cx.theme().clone();

        // Update at 15 fps (~67ms) when visible
        let now = std::time::Instant::now();
        if now.duration_since(self.last_update).as_millis() >= 67 {
            self.last_update = now;
            
            use crate::atomic_memory_tracking::ATOMIC_MEMORY_COUNTERS;
            
            // Quick atomic reads (cheap)
            self.cached_total = ATOMIC_MEMORY_COUNTERS.total();
            self.cached_entries = ATOMIC_MEMORY_COUNTERS.get_all_entries();
        }

        // Use only cached values
        let total_current = self.cached_total;
        let entry_count = self.cached_entries.len();

        // Fixed row height for uniform list
        let row_height = px(50.0);
        let item_sizes = std::rc::Rc::new(vec![size(px(0.0), row_height); entry_count]);

        let view = cx.entity().clone();
        
        // Clone cached data for the closure to avoid reading from atomics
        let cached_entries = self.cached_entries.clone();
        let cached_total = total_current;

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .gap_2()
            .child(
                // Header with total
                v_flex()
                    .w_full()
                    .p_4()
                    .bg(theme.background)
                    .border_b_1()
                    .border_color(theme.border)
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Memory Breakdown")
                    )
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.accent)
                            .child(format!("{:.2} MB", total_current as f64 / 1024.0 / 1024.0))
                    )
            )
            .child(
                // Virtual list of allocations
                v_virtual_list(
                    view,
                    "memory-breakdown-list",
                    item_sizes,
                    move |_this, range, _window, cx| {
                        let theme = cx.theme().clone();
                        
                        // Use cached data - NO atomic reads!
                        let total = cached_total;
                        let entries = &cached_entries;

                        // Color palette
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

                        range.map(|ix| {
                            if let Some(entry) = entries.get(ix) {
                                let percentage = if total > 0 {
                                    (entry.size as f64 / total as f64) * 100.0
                                } else {
                                    0.0
                                };
                                let color = colors[ix % colors.len()];
                                let size_mb = entry.size as f64 / 1024.0 / 1024.0;

                                v_flex()
                                    .w_full()
                                    .p_3()
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
                                                    .child(entry.name.clone())
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
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }
                        }).collect()
                    },
                )
                .track_scroll(&self.scroll_handle)
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

// ─────────────────────────────────────────────────────────────────────────────
// Advanced Metrics Panel — per-core CPU breakdown + temperatures
// ─────────────────────────────────────────────────────────────────────────────

pub struct AdvancedMetricsPanel {
    focus_handle: FocusHandle,
    metrics: SharedPerformanceMetrics,
}

impl AdvancedMetricsPanel {
    pub fn new(metrics: SharedPerformanceMetrics, cx: &mut Context<Self>) -> Self {
        Self { focus_handle: cx.focus_handle(), metrics }
    }

    fn temp_color(temp_c: f64, cx: &App) -> gpui::Hsla {
        let t = cx.theme();
        if temp_c >= 85.0 { t.danger } else if temp_c >= 70.0 { t.warning } else { t.success }
    }

    fn core_color(usage: f64, cx: &App) -> gpui::Hsla {
        let t = cx.theme();
        if usage > 80.0 { t.danger } else if usage > 60.0 { t.warning } else { t.info }
    }

    /// A small area-chart card for one metric.
    fn mini_chart_card(
        label: impl Into<String>,
        value_str: impl Into<String>,
        data: Vec<f64>,
        color: gpui::Hsla,
        cx: &App,
    ) -> impl IntoElement {
        use ui::chart::AreaChart;
        use gpui::prelude::FluentBuilder;
        use ui::h_flex;

        #[derive(Clone)]
        struct Pt { i: usize, v: f64 }

        let pts: Vec<Pt> = data.into_iter().enumerate().map(|(i, v)| Pt { i, v }).collect();
        let label = label.into();
        let value_str = value_str.into();
        let theme = cx.theme().clone();

        v_flex()
            .w_full()
            .p_2()
            .gap_1()
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
                            .text_size(px(10.0))
                            .text_color(theme.muted_foreground)
                            .child(label)
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(color)
                            .child(value_str)
                    )
            )
            .when(!pts.is_empty(), |this| {
                this.child(
                    div()
                        .h(px(36.0))
                        .w_full()
                        .child(
                            AreaChart::<_, SharedString, f64>::new(pts)
                                .x(|p: &Pt| format!("{}", p.i).into())
                                .y(|p: &Pt| p.v)
                                .stroke(color)
                                .fill(color.opacity(0.2))
                                .linear()
                                .tick_margin(0)
                        )
                )
            })
    }
}

impl EventEmitter<PanelEvent> for AdvancedMetricsPanel {}

impl Render for AdvancedMetricsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, scroll::ScrollbarAxis};
        let theme = cx.theme().clone();

        let metrics = self.metrics.read();
        let core_histories: Vec<Vec<f64>> = metrics.cpu_core_histories
            .iter()
            .map(|dq| dq.iter().cloned().collect())
            .collect();
        let temp_histories: Vec<(String, Vec<f64>)> = metrics.temp_histories
            .iter()
            .map(|(l, dq)| (l.clone(), dq.iter().cloned().collect()))
            .collect();
        drop(metrics);

        cx.notify();

        let temps_empty = temp_histories.is_empty();
        let cores_empty = core_histories.is_empty();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_4()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Advanced Metrics")
                    )
            )
            // ── Temperatures ─────────────────────────────────────────────────
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.accent)
                            .child(if temps_empty {
                                "Temperatures".to_string()
                            } else {
                                format!("Temperatures ({} sensors)", temp_histories.len())
                            })
                    )
                    .when(temps_empty, |this: Div| {
                        this.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted_foreground)
                                .child(
                                    if cfg!(windows) {
                                        "Temperature sensors are not available on Windows at this time."
                                    } else {
                                        "No temperature sensors detected."
                                    }
                                )
                        )
                    })
                    .when(!temps_empty, |this: Div| {
                        this.child(
                            div()
                                .w_full()
                                .grid()
                                .grid_cols(4)
                                .gap_2()
                                .children(temp_histories.into_iter().map(|(label, hist)| {
                                    let current = hist.last().copied().unwrap_or(0.0);
                                    let color = Self::temp_color(current, cx);
                                    Self::mini_chart_card(label, format!("{:.0}°C", current), hist, color, cx)
                                }))
                        )
                    })
            )
            // ── CPU Cores ────────────────────────────────────────────────────
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .when(!cores_empty, |this: Div| {
                        this
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.accent)
                                    .child(format!("CPU Cores ({})", core_histories.len()))
                            )
                            .child(
                                div()
                                    .w_full()
                                    .grid()
                                    .grid_cols(4)
                                    .gap_2()
                                    .children(core_histories.into_iter().enumerate().map(|(i, hist)| {
                                        let current = hist.last().copied().unwrap_or(0.0);
                                        let color = Self::core_color(current, cx);
                                        Self::mini_chart_card(format!("Core {}", i), format!("{:.1}%", current), hist, color, cx)
                                    }))
                            )
                    })
            )
            .scrollable(ScrollbarAxis::Vertical)
    }
}

impl Focusable for AdvancedMetricsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for AdvancedMetricsPanel {
    fn panel_name(&self) -> &'static str { "advanced_metrics" }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Advanced".into_any_element()
    }
}
