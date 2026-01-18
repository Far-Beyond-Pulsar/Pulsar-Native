//! Reusable graph panel component.
//!
//! This component eliminates repetitive boilerplate for rendering performance graphs
//! with consistent styling and layout.

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    chart::{AreaChart, BarChart},
    h_flex, v_flex, ActiveTheme, Icon, IconName, StyledExt,
};

/// A reusable panel for displaying performance graphs.
///
/// Supports both bar chart and area chart modes with consistent styling.
pub struct GraphPanel<T: Clone + 'static> {
    title: SharedString,
    icon: IconName,
    subtitle: Option<SharedString>,
    data: Vec<T>,
    use_line_chart: bool,
    height: Pixels,
    chart_colors: ChartColors,
}

/// Color configuration for charts.
pub struct ChartColors {
    pub stroke: Hsla,
    pub fill: Hsla,
}

impl<T: Clone + 'static> GraphPanel<T> {
    /// Create a new graph panel.
    pub fn new(title: impl Into<SharedString>, icon: IconName, data: Vec<T>) -> Self {
        Self {
            title: title.into(),
            icon,
            subtitle: None,
            data,
            use_line_chart: false,
            height: px(120.0),
            chart_colors: ChartColors {
                stroke: Hsla::default(),
                fill: Hsla::default(),
            },
        }
    }

    /// Set a subtitle for the graph.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Set whether to use a line chart (area chart) instead of a bar chart.
    pub fn use_line_chart(mut self, use_line: bool) -> Self {
        self.use_line_chart = use_line;
        self
    }

    /// Set the height of the graph.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }

    /// Set the chart colors.
    pub fn colors(mut self, stroke: Hsla, fill: Hsla) -> Self {
        self.chart_colors = ChartColors { stroke, fill };
        self
    }

    /// Build a bar chart with custom styling function.
    pub fn build_bar<X, Y, F, V: 'static>(
        self,
        x_fn: X,
        y_fn: Y,
        fill_fn: F,
        cx: &Context<V>,
    ) -> impl IntoElement
    where
        X: Fn(&T) -> SharedString + 'static,
        Y: Fn(&T) -> f64 + 'static + Clone,
        F: Fn(&T) -> Hsla + 'static,
        V: Render,
    {
        // Extract fields before consuming self
        let title = self.title.clone();
        let icon = self.icon.clone();
        let subtitle = self.subtitle.clone();
        let height = self.height;

        let chart = BarChart::new(self.data)
            .x(x_fn)
            .y(y_fn)
            .fill(fill_fn)
            .tick_margin(10)
            .into_any_element();

        Self::build_panel_impl(title, icon, subtitle, height, chart, cx)
    }

    /// Build an area chart.
    pub fn build_area<X, Y, V: 'static>(
        self,
        x_fn: X,
        y_fn: Y,
        cx: &Context<V>,
    ) -> impl IntoElement
    where
        X: Fn(&T) -> SharedString + 'static,
        Y: Fn(&T) -> f64 + 'static,
        V: Render,
    {
        // Extract fields before consuming self
        let title = self.title.clone();
        let icon = self.icon.clone();
        let subtitle = self.subtitle.clone();
        let height = self.height;
        let stroke = self.chart_colors.stroke;
        let fill = self.chart_colors.fill;

        let chart = AreaChart::new(self.data)
            .x(x_fn)
            .y(y_fn)
            .stroke(stroke)
            .fill(fill)
            .linear()
            .tick_margin(10)
            .into_any_element();

        Self::build_panel_impl(title, icon, subtitle, height, chart, cx)
    }

    /// Build the graph panel with a custom chart element (internal implementation).
    fn build_panel_impl<V: 'static>(
        title: SharedString,
        icon: IconName,
        subtitle: Option<SharedString>,
        height: Pixels,
        chart: AnyElement,
        cx: &Context<V>,
    ) -> impl IntoElement
    where
        V: Render,
    {
        let header = Self::build_header_impl(title, icon, subtitle, cx);

        v_flex()
            .w_full()
            .p_3()
            .rounded_lg()
            .bg(cx.theme().sidebar.opacity(0.2))
            .border_1()
            .border_color(cx.theme().border.opacity(0.5))
            .gap_2()
            .child(header)
            .child(div().h(height).w_full().child(chart))
    }

    /// Build the header with title, icon, and optional subtitle (internal implementation).
    fn build_header_impl<V: 'static>(
        title: SharedString,
        icon: IconName,
        subtitle: Option<SharedString>,
        cx: &Context<V>,
    ) -> impl IntoElement
    where
        V: Render,
    {
        let mut header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(icon).size_4().text_color(cx.theme().accent))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(title),
                    ),
            );

        if let Some(subtitle) = subtitle {
            header = header.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(subtitle),
            );
        }

        header
    }
}

/// Create a simple FPS-style colored bar chart.
///
/// Colors bars based on value ranges (green for high, yellow for mid, red for low).
pub fn fps_colored_bar_chart<T: Clone + 'static, V: 'static>(
    data: Vec<T>,
    x_fn: impl Fn(&T) -> SharedString + 'static,
    y_fn: impl Fn(&T) -> f64 + 'static + Clone,
    high_threshold: f64,
    low_threshold: f64,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    let theme = cx.theme();
    let success = theme.success;
    let warning = theme.warning;
    let danger = theme.danger;

    let y_fn_clone = y_fn.clone();
    BarChart::new(data)
        .x(x_fn)
        .y(y_fn)
        .fill(move |d| {
            let value = y_fn_clone(d);
            if value >= high_threshold {
                success
            } else if value >= low_threshold {
                warning
            } else {
                danger
            }
        })
        .tick_margin(10)
}

/// Create a standard area chart with theme colors.
pub fn themed_area_chart<T: Clone + 'static>(
    data: Vec<T>,
    x_fn: impl Fn(&T) -> SharedString + 'static,
    y_fn: impl Fn(&T) -> f64 + 'static,
    color: Hsla,
) -> impl IntoElement {
    AreaChart::new(data)
        .x(x_fn)
        .y(y_fn)
        .stroke(color)
        .fill(color.opacity(0.2))
        .linear()
        .tick_margin(10)
}
