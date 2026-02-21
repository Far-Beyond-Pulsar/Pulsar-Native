//! GPU panel — per-engine utilization, VRAM breakdown, and static hardware info.

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use crate::performance_metrics::SharedPerformanceMetrics;
use crate::system_info::SharedSystemInfo;

pub struct GpuMetricsPanel {
    focus_handle: FocusHandle,
    metrics: SharedPerformanceMetrics,
    system_info: SharedSystemInfo,
}

impl GpuMetricsPanel {
    pub fn new(metrics: SharedPerformanceMetrics, system_info: SharedSystemInfo, cx: &mut Context<Self>) -> Self {
        cx.notify();
        Self { focus_handle: cx.focus_handle(), metrics, system_info }
    }

    pub fn mini_chart_card(
        label: impl Into<String>,
        value_str: impl Into<String>,
        data: Vec<f64>,
        color: gpui::Hsla,
        cx: &App,
    ) -> impl IntoElement {
        use ui::chart::AreaChart;
        use ui::h_flex;

        #[derive(Clone)]
        struct Pt { i: usize, v: f64 }

        let pts: Vec<Pt> = data.into_iter().enumerate().map(|(i, v)| Pt { i, v }).collect();
        let label     = label.into();
        let value_str = value_str.into();
        let theme     = cx.theme().clone();

        v_flex()
            .w_full().p_2().gap_1()
            .bg(theme.background).border_1().border_color(theme.border).rounded(px(6.0))
            .child(
                h_flex().w_full().justify_between()
                    .child(div().text_size(px(10.0)).text_color(theme.muted_foreground).child(label))
                    .child(div().text_size(px(11.0)).font_weight(gpui::FontWeight::BOLD).text_color(color).child(value_str))
            )
            .when(!pts.is_empty(), |this| {
                this.child(
                    div().h(px(36.0)).w_full().child(
                        AreaChart::<_, SharedString, f64>::new(pts)
                            .x(|p: &Pt| format!("{}", p.i).into())
                            .y(|p: &Pt| p.v)
                            .stroke(color)
                            .fill(color.opacity(0.2))
                            .linear()
                            .tick_margin(0),
                    ),
                )
            })
    }

    fn section_header(title: &str, cx: &App) -> impl IntoElement {
        let theme = cx.theme().clone();
        div().w_full().mb_1().pb_1().border_b_1().border_color(theme.border)
            .child(div().text_size(px(11.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground).child(title.to_string()))
    }
}

impl Render for GpuMetricsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, scroll::ScrollbarAxis};

        let metrics  = self.metrics.read();
        let sysinfo  = self.system_info.read();
        let theme    = cx.theme().clone();

        let gpu_name     = sysinfo.gpu_name.clone();
        let gpu_vendor   = sysinfo.gpu_vendor.clone();
        let gpu_driver   = sysinfo.gpu_driver_version.clone();
        let vram_total   = sysinfo.gpu_vram_formatted();
        let vram_used_mb   = metrics.current_vram_used_mb;
        let vram_shared_mb = metrics.current_vram_shared_mb;
        let vram_total_mb  = sysinfo.gpu_vram_total_mb.unwrap_or(0) as f64;

        let mut engines: Vec<(String, Vec<f64>)> = metrics.gpu_engine_histories
            .iter()
            .map(|(k, dq)| (k.clone(), dq.iter().copied().collect()))
            .collect();
        engines.sort_by_key(|(k, _)| {
            crate::gpu_engines::KNOWN_ENGINES.iter().position(|e| *e == k).unwrap_or(999)
        });

        drop(metrics);
        drop(sysinfo);

        let vram_pct  = if vram_total_mb > 0.0 { (vram_used_mb / vram_total_mb * 100.0).min(100.0) } else { 0.0 };
        let color_vram = if vram_pct > 85.0 { theme.danger } else if vram_pct > 65.0 { theme.warning } else { theme.success };

        v_flex()
            .w_full().p_3().gap_3()
            // GPU Information
            .child(Self::section_header("GPU Information", cx))
            .child(
                div().w_full().p_3().bg(theme.background)
                    .border_1().border_color(theme.border).rounded(px(6.0))
                    .child(v_flex().gap_1()
                        .child(info_row("Name",       gpu_name,   &theme))
                        .child(info_row("Vendor",     gpu_vendor, &theme))
                        .child(info_row("Driver",     gpu_driver, &theme))
                        .child(info_row("VRAM Total", vram_total, &theme))
                    )
            )
            // Video Memory
            .child(Self::section_header("Video Memory", cx))
            .child(
                div().w_full().grid().grid_cols(2).gap_2()
                    .child(
                        v_flex().p_2().gap_1().bg(theme.background)
                            .border_1().border_color(theme.border).rounded(px(6.0))
                            .child(h_flex().w_full().justify_between()
                                .child(div().text_size(px(10.0)).text_color(theme.muted_foreground).child("Dedicated Used"))
                                .child(div().text_size(px(11.0)).font_weight(gpui::FontWeight::BOLD).text_color(color_vram)
                                    .child(format!("{:.0} MiB ({:.0}%)", vram_used_mb, vram_pct)))
                            )
                            .child(div().text_size(px(10.0)).text_color(theme.muted_foreground)
                                .child(format!("/ {:.0} MiB total", vram_total_mb)))
                    )
                    .child(
                        v_flex().p_2().gap_1().bg(theme.background)
                            .border_1().border_color(theme.border).rounded(px(6.0))
                            .child(h_flex().w_full().justify_between()
                                .child(div().text_size(px(10.0)).text_color(theme.muted_foreground).child("Shared Memory"))
                                .child(div().text_size(px(11.0)).font_weight(gpui::FontWeight::BOLD).text_color(theme.info)
                                    .child(format!("{:.0} MiB", vram_shared_mb)))
                            )
                            .child(div().text_size(px(10.0)).text_color(theme.muted_foreground).child("System RAM used by GPU"))
                    )
            )
            // GPU Engines
            .child(Self::section_header(&format!("GPU Engines ({} active)", engines.len()), cx))
            .when(engines.is_empty(), |this| {
                this.child(div().text_size(px(11.0)).text_color(theme.muted_foreground)
                    .child("No engine data — PDH counters unavailable on this system."))
            })
            .when(!engines.is_empty(), |this: Div| {
                this.child(
                    div().w_full().grid().grid_cols(4).gap_2()
                        .children(engines.into_iter().map(|(name, hist)| {
                            let current = hist.last().copied().unwrap_or(0.0);
                            let color = if current > 80.0 { theme.danger }
                                else if current > 50.0 { theme.warning }
                                else if current > 5.0  { theme.info }
                                else { theme.muted_foreground };
                            Self::mini_chart_card(name, format!("{:.1}%", current), hist, color, cx)
                        }))
                )
            })
            .scrollable(ScrollbarAxis::Vertical)
    }
}

fn info_row(label: &str, value: String, theme: &ui::ThemeColor) -> impl IntoElement {
    use ui::h_flex;
    h_flex().gap_2()
        .child(div().text_size(px(11.0)).text_color(theme.muted_foreground).w(px(80.0)).child(label.to_string()))
        .child(div().text_size(px(11.0)).text_color(theme.foreground).child(value))
}

impl Focusable for GpuMetricsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Panel for GpuMetricsPanel {
    fn panel_name(&self) -> &'static str { "gpu_metrics" }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement { "GPU".into_any_element() }
    fn closable(&self, _cx: &App) -> bool { false }
    fn zoomable(&self, _cx: &App) -> Option<ui::dock::PanelControl> { None }
}

impl EventEmitter<PanelEvent> for GpuMetricsPanel {}
