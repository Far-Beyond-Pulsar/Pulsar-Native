//! CPU panel — per-core utilization charts and temperature sensors.

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use crate::performance_metrics::SharedPerformanceMetrics;

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
}

impl EventEmitter<PanelEvent> for AdvancedMetricsPanel {}

impl Render for AdvancedMetricsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, scroll::ScrollbarAxis};
        let theme = cx.theme().clone();

        let metrics = self.metrics.read();
        let core_histories: Vec<Vec<f64>> = metrics.cpu_core_histories
            .iter().map(|dq| dq.iter().cloned().collect()).collect();
        let temp_histories: Vec<(String, Vec<f64>)> = metrics.temp_histories
            .iter().map(|(l, dq)| (l.clone(), dq.iter().cloned().collect())).collect();
        drop(metrics);

        cx.notify();

        let temps_empty = temp_histories.is_empty();
        let cores_empty = core_histories.is_empty();

        v_flex()
            .size_full().bg(theme.sidebar).p_4().gap_4()
            .child(
                h_flex().items_center()
                    .child(div().text_size(px(14.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground).child("Advanced Metrics"))
            )
            // Temperatures
            .child(
                v_flex().w_full().gap_2()
                    .child(
                        div().text_size(px(12.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.accent)
                            .child(if temps_empty {
                                "Temperatures".to_string()
                            } else {
                                format!("Temperatures ({} sensors)", temp_histories.len())
                            })
                    )
                    .when(temps_empty, |this: Div| {
                        this.child(
                            div().text_size(px(11.0)).text_color(theme.muted_foreground)
                                .child(if cfg!(windows) {
                                    "Temperature sensors are not available on Windows at this time."
                                } else {
                                    "No temperature sensors detected."
                                })
                        )
                    })
                    .when(!temps_empty, |this: Div| {
                        this.child(
                            div().w_full().grid().grid_cols(4).gap_2()
                                .children(temp_histories.into_iter().map(|(label, hist)| {
                                    let current = hist.last().copied().unwrap_or(0.0);
                                    let color   = Self::temp_color(current, cx);
                                    Self::mini_chart_card(label, format!("{:.0}°C", current), hist, color, cx)
                                }))
                        )
                    })
            )
            // CPU Cores
            .child(
                v_flex().w_full().gap_2()
                    .when(!cores_empty, |this: Div| {
                        this
                            .child(div().text_size(px(12.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.accent)
                                .child(format!("CPU Cores ({})", core_histories.len())))
                            .child(
                                div().w_full().grid().grid_cols(4).gap_2()
                                    .children(core_histories.into_iter().enumerate().map(|(i, hist)| {
                                        let current = hist.last().copied().unwrap_or(0.0);
                                        let color   = Self::core_color(current, cx);
                                        Self::mini_chart_card(format!("Core {}", i), format!("{:.1}%", current), hist, color, cx)
                                    }))
                            )
                    })
            )
            .scrollable(ScrollbarAxis::Vertical)
    }
}

impl Focusable for AdvancedMetricsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Panel for AdvancedMetricsPanel {
    fn panel_name(&self) -> &'static str { "advanced_metrics" }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement { "CPU".into_any_element() }
}
