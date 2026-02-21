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

/// Allocation types table delegate
struct AllocationTypesTable {
    sites: Vec<crate::AllocationSite>,
    columns: Vec<ui::table::Column>,
}

impl AllocationTypesTable {
    fn new() -> Self {
        let columns = vec![
            ui::table::Column::new("type").name("Type Signature").resizable(true),
            ui::table::Column::new("size").name("Size").resizable(true),
            ui::table::Column::new("align").name("Align").resizable(true),
            ui::table::Column::new("count").name("Count").resizable(true),
            ui::table::Column::new("total").name("Total MB").resizable(true),
        ];

        Self {
            sites: Vec::new(),
            columns,
        }
    }

    fn update_sites(&mut self) {
        self.sites = crate::TYPE_TRACKER.get_sites();
    }
}

impl ui::table::TableDelegate for AllocationTypesTable {
    fn columns_count(&self, _cx: &gpui::App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &gpui::App) -> usize {
        self.sites.len()
    }

    fn column(&self, col_ix: usize, _cx: &gpui::App) -> &ui::table::Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<ui::table::Table<Self>>,
    ) -> impl IntoElement {
        use ui::h_flex;
        let theme = cx.theme();

        if let Some(site) = self.sites.get(row_ix) {
            let text = match col_ix {
                0 => site.type_signature.clone(),
                1 => format!("{}B", site.size),
                2 => format!("{}", site.align),
                3 => format!("{}", site.count),
                4 => format!("{:.2}", site.total_bytes as f64 / 1024.0 / 1024.0),
                _ => String::new(),
            };

            h_flex()
                .px_2()
                .py_1()
                .text_size(px(11.0))
                .text_color(theme.foreground)
                .child(text)
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }
}

/// Memory Breakdown Panel - Real-time memory allocation tracking
pub struct MemoryBreakdownPanel {
    focus_handle: FocusHandle,
    last_update: std::time::Instant,
    scroll_handle: ui::VirtualListScrollHandle,
    entries: Vec<crate::AllocationEntry>,
    types_table: AllocationTypesTable,
}

impl MemoryBreakdownPanel {
    pub fn new(_memory_tracker: SharedMemoryTracker, cx: &mut Context<Self>) -> Self {
        // We don't use the memory_tracker anymore - using atomic counters directly
        Self {
            focus_handle: cx.focus_handle(),
            last_update: std::time::Instant::now(),
            scroll_handle: ui::VirtualListScrollHandle::new(),
            entries: Vec::new(),
            types_table: AllocationTypesTable::new(),
        }
    }
}

impl EventEmitter<PanelEvent> for MemoryBreakdownPanel {}

impl Render for MemoryBreakdownPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, v_virtual_list};
        use crate::atomic_memory_tracking::ATOMIC_MEMORY_COUNTERS;

        let theme = cx.theme().clone();

        // Update entries at a reasonable rate (max 10 FPS)
        let now = std::time::Instant::now();
        if now.duration_since(self.last_update).as_millis() >= 100 {
            self.last_update = now;
            self.entries = ATOMIC_MEMORY_COUNTERS.get_all_entries();
            self.types_table.update_sites();
            cx.notify();
        }

        let total_current = ATOMIC_MEMORY_COUNTERS.total();
        let entry_count = self.entries.len();

        // Fixed row height for uniform list
        let row_height = px(50.0);
        let item_sizes = std::rc::Rc::new(vec![size(px(0.0), row_height); entry_count]);

        let view = cx.entity().clone();

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
                    move |this, range, window, cx| {
                        let theme = cx.theme().clone();
                        let total = ATOMIC_MEMORY_COUNTERS.total();
                        let entries = ATOMIC_MEMORY_COUNTERS.get_all_entries();

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
