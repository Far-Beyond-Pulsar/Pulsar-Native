//! Resource Monitor panel — CPU, memory, GPU, FPS, network, disk charts.

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use crate::performance_metrics::SharedPerformanceMetrics;

pub struct ResourceMonitorPanel {
    focus_handle: FocusHandle,
    metrics: SharedPerformanceMetrics,
}

impl ResourceMonitorPanel {
    pub fn new(metrics: SharedPerformanceMetrics, cx: &mut Context<Self>) -> Self {
        Self { focus_handle: cx.focus_handle(), metrics }
    }

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
                    .child(div().text_size(px(12.0)).font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground).child(label))
                    .child(div().text_size(px(14.0)).font_weight(gpui::FontWeight::BOLD)
                        .text_color(color).child(value_str))
            )
            .when(!data.is_empty(), |this| {
                this.child(
                    div().h(px(40.0)).w_full().child(
                        AreaChart::<_, SharedString, f64>::new(data)
                            .x(move |_d| "".into())
                            .y(y_fn)
                            .stroke(color)
                            .fill(color.opacity(0.15))
                            .linear()
                            .tick_margin(0),
                    ),
                )
            })
    }
}

impl EventEmitter<PanelEvent> for ResourceMonitorPanel {}

impl Render for ResourceMonitorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, chart::AreaChart, scroll::ScrollbarAxis};

        let theme = cx.theme().clone();
        let metrics = self.metrics.read();

        let current_cpu             = metrics.current_cpu;
        let current_memory_mb       = metrics.current_memory_mb;
        let current_vram_used_mb    = metrics.current_vram_used_mb;
        let current_fps             = metrics.current_fps;
        let current_net_rx_kbps     = metrics.current_net_rx_kbps;
        let current_net_tx_kbps     = metrics.current_net_tx_kbps;
        let current_disk_read_kbps  = metrics.current_disk_read_kbps;
        let current_disk_write_kbps = metrics.current_disk_write_kbps;

        let cpu_data:        Vec<_> = metrics.cpu_history.iter().cloned().collect();
        let memory_data:     Vec<_> = metrics.memory_history.iter().cloned().collect();
        let gpu_data:        Vec<_> = metrics.gpu_history.iter().cloned().collect();
        let fps_data:        Vec<_> = metrics.fps_history.iter().cloned().collect();
        let net_rx_data:     Vec<_> = metrics.net_rx_history.iter().cloned().collect();
        let net_tx_data:     Vec<_> = metrics.net_tx_history.iter().cloned().collect();
        let disk_read_data:  Vec<_> = metrics.disk_read_history.iter().cloned().collect();
        let disk_write_data: Vec<_> = metrics.disk_write_history.iter().cloned().collect();
        drop(metrics);

        cx.notify();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .p_4()
            .gap_4()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                h_flex().items_center().gap_2()
                    .child(div().text_size(px(14.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground).child("System Resources"))
            )
            // CPU
            .child(
                v_flex().w_full().p_3().gap_2().bg(theme.background)
                    .border_1().border_color(theme.border).rounded(px(6.0))
                    .child(h_flex().w_full().justify_between()
                        .child(div().text_size(px(12.0)).font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground).child("CPU Usage"))
                        .child(div().text_size(px(18.0)).font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.info).child(format!("{:.1}%", current_cpu)))
                    )
                    .when(!cpu_data.is_empty(), |this| {
                        this.child(div().h(px(60.0)).w_full().child(
                            AreaChart::<_, SharedString, f64>::new(cpu_data)
                                .x(|d| format!("{}", d.index).into()).y(|d| d.usage)
                                .stroke(theme.info).fill(theme.info.opacity(0.15))
                                .linear().tick_margin(0).max_y_range(100.0)
                        ))
                    })
            )
            // Memory
            .child(
                v_flex().w_full().p_3().gap_2().bg(theme.background)
                    .border_1().border_color(theme.border).rounded(px(6.0))
                    .child(h_flex().w_full().justify_between()
                        .child(div().text_size(px(12.0)).font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground).child("Memory Usage"))
                        .child(div().text_size(px(18.0)).font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.warning).child(format!("{:.0} MB", current_memory_mb)))
                    )
                    .when(!memory_data.is_empty(), |this| {
                        this.child(div().h(px(60.0)).w_full().child(
                            AreaChart::<_, SharedString, f64>::new(memory_data)
                                .x(|d| format!("{}", d.index).into()).y(|d| d.memory_mb)
                                .stroke(theme.warning).fill(theme.warning.opacity(0.15))
                                .linear().tick_margin(0)
                        ))
                    })
            )
            // GPU VRAM
            .child(
                v_flex().w_full().p_3().gap_2().bg(theme.background)
                    .border_1().border_color(theme.border).rounded(px(6.0))
                    .child(h_flex().w_full().justify_between()
                        .child(div().text_size(px(12.0)).font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground).child("GPU VRAM Used"))
                        .child(div().text_size(px(18.0)).font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.success).child(
                                if current_vram_used_mb > 0.0 {
                                    format!("{:.0} MB", current_vram_used_mb)
                                } else { "N/A".to_string() }
                            ))
                    )
                    .when(!gpu_data.is_empty(), |this| {
                        this.child(div().h(px(60.0)).w_full().child(
                            AreaChart::<_, SharedString, f64>::new(gpu_data)
                                .x(|d| format!("{}", d.index).into()).y(|d| d.vram_used_mb)
                                .stroke(theme.success).fill(theme.success.opacity(0.15))
                                .linear().tick_margin(0)
                        ))
                    })
            )
            // FPS
            .child(
                v_flex().w_full().p_3().gap_2().bg(theme.background)
                    .border_1().border_color(theme.border).rounded(px(6.0))
                    .child(h_flex().w_full().justify_between()
                        .child(div().text_size(px(12.0)).font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground).child("Frame Rate"))
                        .child(div().text_size(px(18.0)).font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.accent).child(
                                if current_fps > 0.0 {
                                    format!("{:.0} FPS", current_fps)
                                } else { "N/A".to_string() }
                            ))
                    )
                    .when(!fps_data.is_empty(), |this| {
                        this.child(div().h(px(60.0)).w_full().child(
                            AreaChart::<_, SharedString, f64>::new(fps_data)
                                .x(|d| format!("{}", d.index).into()).y(|d| d.fps)
                                .stroke(theme.accent).fill(theme.accent.opacity(0.15))
                                .linear().tick_margin(0)
                        ))
                    })
            )
            // Network / Disk I/O
            .child(Self::io_chart_card(
                "Network In",
                if current_net_rx_kbps >= 1024.0 {
                    format!("{:.1} MB/s", current_net_rx_kbps / 1024.0)
                } else { format!("{:.0} KB/s", current_net_rx_kbps) },
                net_rx_data,
                |d: &crate::performance_metrics::NetDataPoint| d.kbps,
                theme.info, cx,
            ))
            .child(Self::io_chart_card(
                "Network Out",
                if current_net_tx_kbps >= 1024.0 {
                    format!("{:.1} MB/s", current_net_tx_kbps / 1024.0)
                } else { format!("{:.0} KB/s", current_net_tx_kbps) },
                net_tx_data,
                |d: &crate::performance_metrics::NetDataPoint| d.kbps,
                theme.warning, cx,
            ))
            .child(Self::io_chart_card(
                "Disk Read",
                if current_disk_read_kbps >= 1024.0 {
                    format!("{:.1} MB/s", current_disk_read_kbps / 1024.0)
                } else { format!("{:.0} KB/s", current_disk_read_kbps) },
                disk_read_data,
                |d: &crate::performance_metrics::DiskDataPoint| d.kbps,
                theme.success, cx,
            ))
            .child(Self::io_chart_card(
                "Disk Write",
                if current_disk_write_kbps >= 1024.0 {
                    format!("{:.1} MB/s", current_disk_write_kbps / 1024.0)
                } else { format!("{:.0} KB/s", current_disk_write_kbps) },
                disk_write_data,
                |d: &crate::performance_metrics::DiskDataPoint| d.kbps,
                theme.danger, cx,
            ))
    }
}

impl Focusable for ResourceMonitorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Panel for ResourceMonitorPanel {
    fn panel_name(&self) -> &'static str { "resource_monitor" }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement { "Resources".into_any_element() }
}
