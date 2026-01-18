//! Performance overlay component with stats and graphs.
//!
//! This component provides a comprehensive performance monitoring overlay
//! with FPS stats, metric toggles, and interactive graphs.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::{Button, ButtonVariants as _}, chart::{AreaChart, BarChart}, h_flex, v_flex, ActiveTheme, Icon, IconName, StyledExt,
};

use super::super::performance::*;
use crate::level_editor::ui::state::LevelEditorState;
use super::toggle_button::create_state_toggle;

/// Metric card displaying a single performance value with color coding.
fn metric_card<V: 'static>(
    title: impl Into<SharedString>,
    value: String,
    threshold_high: Option<f64>,
    threshold_low: Option<f64>,
    actual_value: f64,
    use_bold: bool,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    let title_str: SharedString = title.into();

    let (border_color, text_color) = if let (Some(high), Some(low)) = (threshold_high, threshold_low) {
        if actual_value > high {
            (cx.theme().success.opacity(0.3), cx.theme().success)
        } else if actual_value > low {
            (cx.theme().warning.opacity(0.3), cx.theme().warning)
        } else {
            (cx.theme().danger.opacity(0.3), cx.theme().danger)
        }
    } else {
        (cx.theme().border.opacity(0.5), cx.theme().foreground)
    };

    let mut value_div = div();
    
    if use_bold {
        value_div = value_div
            .text_2xl()
            .font_weight(FontWeight::BOLD);
    } else {
        value_div = value_div
            .text_xl()
            .font_weight(FontWeight::SEMIBOLD);
    }
    
    value_div = value_div
        .text_color(text_color)
        .child(value);

    v_flex()
        .flex_1()
        .gap_1()
        .p_3()
        .rounded_lg()
        .bg(cx.theme().sidebar.opacity(0.3))
        .border_1()
        .border_color(border_color)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(title_str),
        )
        .child(value_div)
}

/// Render the metrics selector toolbar.
fn metrics_selector_toolbar(
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    state: &LevelEditorState,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_1()
        .flex_wrap()
        .child(create_state_toggle(
            "toggle_fps",
            IconName::Activity,
            "FPS Graph",
            state.show_fps_graph,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_fps_graph(),
        ))
        .child(create_state_toggle(
            "toggle_tps",
            IconName::Play,
            "TPS Graph",
            state.show_tps_graph,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_tps_graph(),
        ))
        .child(create_state_toggle(
            "toggle_frame_time",
            IconName::Clock,
            "Frame Time",
            state.show_frame_time_graph,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_frame_time_graph(),
        ))
        .child(create_state_toggle(
            "toggle_memory",
            IconName::Database,
            "GPU Memory",
            state.show_memory_graph,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_memory_graph(),
        ))
        .child(create_state_toggle(
            "toggle_draw_calls",
            IconName::BookStack,
            "Draw Calls",
            state.show_draw_calls_graph,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_draw_calls_graph(),
        ))
        .child(create_state_toggle(
            "toggle_vertices",
            IconName::Triangle,
            "Vertices",
            state.show_vertices_graph,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_vertices_graph(),
        ))
        .child(create_state_toggle(
            "toggle_input_latency",
            IconName::CursorPointer,
            "Input Latency",
            state.show_input_latency_graph,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_input_latency_graph(),
        ))
        .child(create_state_toggle(
            "toggle_ui_consistency",
            IconName::GraphUp,
            "UI Consistency",
            state.show_ui_consistency_graph,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_ui_consistency_graph(),
        ))
}

/// Render a graph panel with title, icon, and chart.
fn render_graph<T: Clone + 'static, V: 'static>(
    title: impl Into<SharedString>,
    icon: IconName,
    subtitle: Option<impl Into<SharedString>>,
    data: Vec<T>,
    use_line_chart: bool,
    x_fn: impl Fn(&T) -> SharedString + 'static + Clone,
    y_fn: impl Fn(&T) -> f64 + 'static + Clone,
    chart_color: Hsla,
    fill_fn: Option<impl Fn(&T) -> Hsla + 'static>,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    let title_str: SharedString = title.into();
    let subtitle_str: Option<SharedString> = subtitle.map(|s| s.into());

    let mut panel = v_flex()
        .w_full()
        .p_3()
        .rounded_lg()
        .bg(cx.theme().sidebar.opacity(0.2))
        .border_1()
        .border_color(cx.theme().border.opacity(0.5))
        .gap_2();

    // Header
    let mut header = h_flex()
        .w_full()
        .items_center()
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
                        .child(title_str),
                ),
        );

    if let Some(sub) = subtitle_str {
        header = header.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(sub),
        );
    }

    panel = panel.child(header);

    // Chart
    let chart = if use_line_chart {
        AreaChart::new(data)
            .x(x_fn)
            .y(y_fn)
            .stroke(chart_color)
            .fill(chart_color.opacity(0.2))
            .linear()
            .tick_margin(10)
            .into_any_element()
    } else if let Some(fill_fn) = fill_fn {
        BarChart::new(data)
            .x(x_fn)
            .y(y_fn)
            .fill(fill_fn)
            .tick_margin(10)
            .into_any_element()
    } else {
        BarChart::new(data)
            .x(x_fn)
            .y(y_fn)
            .fill(move |_| chart_color)
            .tick_margin(10)
            .into_any_element()
    };

    panel.child(div().h(px(120.0)).w_full().child(chart))
}

/// Render the complete performance overlay.
pub fn render_performance_overlay<V: 'static>(
    state: &LevelEditorState,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    ui_fps: f64,
    bevy_fps: f64,
    pipeline_us: u64,
    fps_data: Vec<FpsDataPoint>,
    tps_data: Vec<TpsDataPoint>,
    frame_time_data: Vec<FrameTimeDataPoint>,
    memory_data: Vec<MemoryDataPoint>,
    draw_calls_data: Vec<DrawCallsDataPoint>,
    vertices_data: Vec<VerticesDataPoint>,
    input_latency_data: Vec<InputLatencyDataPoint>,
    ui_consistency_data: Vec<UiConsistencyDataPoint>,
    fps_graph_state: Rc<RefCell<bool>>,
    cx: &mut Context<V>,
) -> impl IntoElement
where
    V: EventEmitter<ui::dock::PanelEvent> + Render,
{
    if state.performance_overlay_collapsed {
        return Button::new("expand_performance")
            .icon(IconName::Activity)
            .ghost()
            .tooltip("Show Performance Stats")
            .on_click(move |_, _, _| {
                state_arc.write().set_performance_overlay_collapsed(false);
            })
            .into_any_element();
    }

    let mut overlay = v_flex()
        .gap_2()
        .p_3()
        .w_full()
        .bg(cx.theme().background.opacity(0.7))
        .rounded_xl()
        .border_1()
        .border_color(cx.theme().border.opacity(0.5))
        .shadow_lg()
        // Header
        .child(
            v_flex()
                .w_full()
                .gap_3()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .w(px(32.0))
                                        .h(px(32.0))
                                        .rounded_lg()
                                        .bg(cx.theme().accent.opacity(0.15))
                                        .border_1()
                                        .border_color(cx.theme().accent.opacity(0.3))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            Icon::new(IconName::Activity)
                                                .size_4()
                                                .text_color(cx.theme().accent),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().foreground)
                                        .child("Performance Stats"),
                                ),
                        )
                        .child({
                            let state_clone = state_arc.clone();
                            Button::new("collapse_performance")
                                .icon(IconName::Close)
                                .ghost()
                                .tooltip("Close")
                                .on_click(move |_, _, _| {
                                    state_clone.write().set_performance_overlay_collapsed(true);
                                })
                        }),
                )
                // Metric cards
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(metric_card("UI FPS", format!("{:.0}", ui_fps), Some(300.0), Some(144.0), ui_fps, true, cx))
                        .child(metric_card("Render FPS", format!("{:.0}", bevy_fps), Some(200.0), Some(60.0), bevy_fps, true, cx))
                        .child(metric_card("Frame Time", format!("{:.2}ms", pipeline_us as f64 / 1000.0), None, None, 0.0, false, cx)),
                ),
        )
        // Metrics selector
        .child(
            div()
                .w_full()
                .p_2()
                .rounded_lg()
                .bg(cx.theme().sidebar.opacity(0.2))
                .border_1()
                .border_color(cx.theme().border.opacity(0.5))
                .child(metrics_selector_toolbar(state_arc.clone(), state)),
        );

    // Graphs
    if state.show_fps_graph && !fps_data.is_empty() {
        let use_line = *fps_graph_state.borrow();
        let theme = cx.theme();
        overlay = if use_line {
            let color = theme.chart_1;
            overlay.child(render_graph(
                "FPS Graph",
                IconName::Activity,
                None::<&str>,
                fps_data,
                true,
                |d| SharedString::from(format!("{}", d.index)),
                |d| d.fps,
                color,
                None::<fn(&FpsDataPoint) -> Hsla>,
                cx,
            ))
        } else {
            let success = theme.success;
            let warning = theme.warning;
            let danger = theme.danger;
            overlay.child(render_graph(
                "FPS Graph",
                IconName::Activity,
                None::<&str>,
                fps_data,
                false,
                |d| SharedString::from(format!("{}", d.index)),
                |d| d.fps,
                success,
                Some(move |d: &FpsDataPoint| {
                    if d.fps >= 120.0 {
                        success
                    } else if d.fps >= 60.0 {
                        warning
                    } else {
                        danger
                    }
                }),
                cx,
            ))
        };
    }

    if state.show_tps_graph && !tps_data.is_empty() {
        let use_line = *fps_graph_state.borrow();
        let color = cx.theme().chart_2;
        overlay = overlay.child(render_graph(
            "TPS Graph",
            IconName::Play,
            None::<&str>,
            tps_data,
            use_line,
            |d| SharedString::from(format!("{}", d.index)),
            |d| d.tps,
            color,
            None::<fn(&TpsDataPoint) -> Hsla>,
            cx,
        ));
    }

    if state.show_frame_time_graph && !frame_time_data.is_empty() {
        let use_line = *fps_graph_state.borrow();
        let color = cx.theme().chart_3;
        overlay = overlay.child(render_graph(
            "Frame Time",
            IconName::Clock,
            Some("(Lower = Better)"),
            frame_time_data,
            use_line,
            |d| SharedString::from(format!("{}", d.index)),
            |d| d.frame_time_ms,
            color,
            None::<fn(&FrameTimeDataPoint) -> Hsla>,
            cx,
        ));
    }

    if state.show_memory_graph && !memory_data.is_empty() {
        let use_line = *fps_graph_state.borrow();
        let color = cx.theme().chart_4;
        overlay = overlay.child(render_graph(
            "GPU Memory",
            IconName::Database,
            None::<&str>,
            memory_data,
            use_line,
            |d| SharedString::from(format!("{}", d.index)),
            |d| d.memory_mb,
            color,
            None::<fn(&MemoryDataPoint) -> Hsla>,
            cx,
        ));
    }

    if state.show_draw_calls_graph && !draw_calls_data.is_empty() {
        let use_line = *fps_graph_state.borrow();
        let color = cx.theme().chart_5;
        overlay = overlay.child(render_graph(
            "Draw Calls per Frame",
            IconName::BookStack,
            None::<&str>,
            draw_calls_data,
            use_line,
            |d| SharedString::from(format!("{}", d.index)),
            |d| d.draw_calls,
            color,
            None::<fn(&DrawCallsDataPoint) -> Hsla>,
            cx,
        ));
    }

    if state.show_vertices_graph && !vertices_data.is_empty() {
        let use_line = *fps_graph_state.borrow();
        let color = cx.theme().chart_1;
        overlay = overlay.child(render_graph(
            "Vertices Rendered",
            IconName::Triangle,
            None::<&str>,
            vertices_data,
            use_line,
            |d| SharedString::from(format!("{}", d.index)),
            |d| d.vertices,
            color,
            None::<fn(&VerticesDataPoint) -> Hsla>,
            cx,
        ));
    }

    if state.show_input_latency_graph && !input_latency_data.is_empty() {
        let use_line = *fps_graph_state.borrow();
        let color = cx.theme().warning;
        overlay = overlay.child(render_graph(
            "Input Thread Latency",
            IconName::CursorPointer,
            Some("Time to send input to GPU"),
            input_latency_data,
            use_line,
            |d| SharedString::from(format!("{}", d.index)),
            |d| d.latency_ms,
            color,
            None::<fn(&InputLatencyDataPoint) -> Hsla>,
            cx,
        ));
    }

    if state.show_ui_consistency_graph && !ui_consistency_data.is_empty() {
        let use_line = *fps_graph_state.borrow();
        let color = cx.theme().accent;
        overlay = overlay.child(render_graph(
            "UI Consistency",
            IconName::GraphUp,
            Some("(Lower = Smoother)"),
            ui_consistency_data,
            use_line,
            |d| SharedString::from(format!("{}", d.index)),
            |d| d.consistency_score,
            color,
            None::<fn(&UiConsistencyDataPoint) -> Hsla>,
            cx,
        ));
    }

    overlay.into_any_element()
}
