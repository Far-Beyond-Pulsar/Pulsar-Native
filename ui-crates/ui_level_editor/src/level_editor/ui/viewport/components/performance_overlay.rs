//! Compact performance overlay for game development monitoring.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::{Button, ButtonVariants as _}, chart::AreaChart, h_flex, v_flex, ActiveTheme, Icon, IconName, StyledExt,
};

use super::super::performance::*;
use crate::level_editor::ui::state::LevelEditorState;

/// Compact stat display: label and value in a single line.
fn stat_line<V: 'static>(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    color: Hsla,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.into())
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(value.into())
        )
}

/// Mini inline sparkline graph for compact display.
fn mini_graph<T: Clone + 'static, V: 'static>(
    data: Vec<T>,
    x_fn: impl Fn(&T) -> SharedString + 'static,
    y_fn: impl Fn(&T) -> f64 + 'static,
    color: Hsla,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    div()
        .h(px(40.0))
        .w_full()
        .child(
            AreaChart::new(data)
                .x(x_fn)
                .y(y_fn)
                .stroke(color)
                .fill(color.opacity(0.15))
                .linear()
                .tick_margin(0)
        )
}

/// Render the complete compact performance overlay.
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
    _fps_graph_state: Rc<RefCell<bool>>,
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

    let frame_time_ms = pipeline_us as f64 / 1000.0;
    
    // Color coding for FPS
    let ui_fps_color = if ui_fps >= 240.0 {
        cx.theme().success
    } else if ui_fps >= 120.0 {
        cx.theme().accent
    } else if ui_fps >= 60.0 {
        cx.theme().warning
    } else {
        cx.theme().danger
    };

    let bevy_fps_color = if bevy_fps >= 144.0 {
        cx.theme().success
    } else if bevy_fps >= 60.0 {
        cx.theme().accent
    } else if bevy_fps >= 30.0 {
        cx.theme().warning
    } else {
        cx.theme().danger
    };

    let frame_color = if frame_time_ms <= 6.9 {
        cx.theme().success
    } else if frame_time_ms <= 16.6 {
        cx.theme().accent
    } else if frame_time_ms <= 33.3 {
        cx.theme().warning
    } else {
        cx.theme().danger
    };

    // Calculate averages
    let avg_draw_calls = if !draw_calls_data.is_empty() {
        draw_calls_data.iter().map(|d| d.draw_calls).sum::<f64>() / draw_calls_data.len() as f64
    } else {
        0.0
    };

    let avg_vertices = if !vertices_data.is_empty() {
        vertices_data.iter().map(|d| d.vertices).sum::<f64>() / vertices_data.len() as f64
    } else {
        0.0
    };

    let avg_memory = if !memory_data.is_empty() {
        memory_data.iter().map(|d| d.memory_mb).sum::<f64>() / memory_data.len() as f64
    } else {
        0.0
    };

    let avg_input_latency = if !input_latency_data.is_empty() {
        input_latency_data.iter().map(|d| d.latency_ms).sum::<f64>() / input_latency_data.len() as f64
    } else {
        0.0
    };

    v_flex()
        .gap_2()
        .p_2()
        .w(px(280.0))
        .bg(cx.theme().background.opacity(0.85))
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border.opacity(0.5))
        .shadow_lg()
        // Header
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(Icon::new(IconName::Activity).size_3().text_color(cx.theme().accent))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("Performance")
                        )
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
                })
        )
        // Core metrics section
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .p_1p5()
                .rounded(cx.theme().radius)
                .bg(cx.theme().sidebar.opacity(0.3))
                .border_1()
                .border_color(cx.theme().border.opacity(0.3))
                .child(stat_line("UI FPS", format!("{:.0}", ui_fps), ui_fps_color, cx))
                .child(stat_line("Render FPS", format!("{:.0}", bevy_fps), bevy_fps_color, cx))
                .child(stat_line("Frame Time", format!("{:.2}ms", frame_time_ms), frame_color, cx))
        )
        // Rendering stats
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .p_1p5()
                .rounded(cx.theme().radius)
                .bg(cx.theme().sidebar.opacity(0.3))
                .border_1()
                .border_color(cx.theme().border.opacity(0.3))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child("Rendering")
                )
                .child(stat_line("Draw Calls", format!("{:.0}", avg_draw_calls), cx.theme().chart_1, cx))
                .child(stat_line("Vertices", format!("{:.0}k", avg_vertices / 1000.0), cx.theme().chart_2, cx))
                .child(stat_line("GPU Memory", format!("{:.1}MB", avg_memory), cx.theme().chart_3, cx))
        )
        // Input latency
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .p_1p5()
                .rounded(cx.theme().radius)
                .bg(cx.theme().sidebar.opacity(0.3))
                .border_1()
                .border_color(cx.theme().border.opacity(0.3))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().muted_foreground)
                        .child("Input")
                )
                .child(stat_line("Latency", format!("{:.2}ms", avg_input_latency), cx.theme().warning, cx))
        )
        // Mini graphs section
        .when(!fps_data.is_empty(), |el| {
            el.child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .p_1p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().sidebar.opacity(0.3))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.3))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("FPS History")
                    )
                    .child(mini_graph(
                        fps_data,
                        |d| SharedString::from(format!("{}", d.index)),
                        |d| d.fps,
                        ui_fps_color,
                        cx
                    ))
            )
        })
        .when(!frame_time_data.is_empty(), |el| {
            el.child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .p_1p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().sidebar.opacity(0.3))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.3))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("Frame Time (ms)")
                    )
                    .child(mini_graph(
                        frame_time_data,
                        |d| SharedString::from(format!("{}", d.index)),
                        |d| d.frame_time_ms.min(50.0), // Cap spikes at 50ms
                        frame_color,
                        cx
                    ))
            )
        })
        .when(!input_latency_data.is_empty(), |el| {
            el.child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .p_1p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().sidebar.opacity(0.3))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.3))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("Input Latency (ms)")
                    )
                    .child(mini_graph(
                        input_latency_data,
                        |d| SharedString::from(format!("{}", d.index)),
                        |d| d.latency_ms,
                        cx.theme().warning,
                        cx
                    ))
            )
        })
        .into_any_element()
}
