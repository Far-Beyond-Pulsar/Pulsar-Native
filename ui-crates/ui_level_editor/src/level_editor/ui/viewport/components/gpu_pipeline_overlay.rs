//! GPU pipeline statistics overlay component.
//!
//! This component displays detailed GPU timing information for each render pass,
//! similar to Unreal Engine's "stat gpu" command.

use std::sync::{Arc, Mutex};

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{button::{Button, ButtonVariants as _}, h_flex, v_flex, ActiveTheme, Icon, IconName, StyledExt};

use crate::level_editor::ui::state::LevelEditorState;

/// Render a single render pass stat row.
fn render_pass_stat<V: 'static>(
    name: String,
    time_ms: f32,
    percent: f32,
    color: Hsla,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    div()
        .w_full()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(cx.theme().sidebar.opacity(0.1))
        .hover(|style| style.bg(cx.theme().sidebar.opacity(0.2)))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(
                    // Color indicator
                    div()
                        .w(px(12.0))
                        .h(px(12.0))
                        .rounded_sm()
                        .bg(color)
                        .mr_2()
                        .flex_none(),
                )
                .child(
                    // Pass name
                    div()
                        .w(px(220.0))
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .child(name),
                )
                .child(
                    // Time
                    div()
                        .w(px(70.0))
                        .flex_none()
                        .text_right()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .child(format!("{:.2}ms", time_ms)),
                )
                .child(
                    // Percentage
                    div()
                        .w(px(60.0))
                        .flex_none()
                        .text_right()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{:.1}%", percent)),
                ),
        )
}

/// Render the complete GPU pipeline overlay.
pub fn render_gpu_pipeline_overlay<V: 'static>(
    state: &LevelEditorState,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    cx: &mut Context<V>,
) -> impl IntoElement
where
    V: EventEmitter<ui::dock::PanelEvent> + Render,
{
    if state.gpu_pipeline_overlay_collapsed {
        return Button::new("expand_gpu_pipeline")
            .icon(IconName::Activity)
            .tooltip("GPU Pipeline Stats")
            .on_click(move |_, _, _| {
                state_arc.write().set_gpu_pipeline_overlay_collapsed(false);
            })
            .into_any_element();
    }

    // Get GPU profiler data
    let gpu_data = if let Ok(engine) = gpu_engine.lock() {
        engine.get_gpu_profiler_data()
    } else {
        None
    };

    // Get theme colors
    let (background, border, foreground, chart_colors, warning, success, danger) = {
        let theme = cx.theme();
        (
            theme.background,
            theme.border,
            theme.foreground,
            vec![
                theme.chart_1,
                theme.chart_2,
                theme.chart_3,
                theme.chart_4,
                theme.chart_5,
            ],
            theme.warning,
            theme.success,
            theme.danger,
        )
    };

    // Collect and sort pass data
    let pass_data = if let Some(ref data) = gpu_data {
        let mut render_passes: Vec<_> = data
            .render_metrics
            .iter()
            .filter(|metric| metric.path.starts_with("render/") && metric.value_ms > 0.0)
            .collect();

        render_passes.sort_by(|a, b| {
            b.value_ms
                .partial_cmp(&a.value_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        render_passes
            .into_iter()
            .take(15)
            .enumerate()
            .map(|(i, metric)| {
                let color = chart_colors[i % chart_colors.len()];
                (
                    metric.name.clone(),
                    metric.value_ms,
                    metric.percentage,
                    color,
                )
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };

    let mut overlay = v_flex()
        .gap_1p5()
        .p_2()
        .w_full()
        .bg(background.opacity(0.7))
        .rounded_xl()
        .border_1()
        .border_color(border.opacity(0.5))
        .shadow_lg()
        // Header
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
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded_lg()
                                .bg(cx.theme().accent.opacity(0.15))
                                .border_1()
                                .border_color(cx.theme().accent.opacity(0.3))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Activity)
                                        .size_3()
                                        .text_color(cx.theme().accent),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(foreground)
                                .child("GPU Pipeline Stats"),
                        ),
                )
                .child({
                    let state_clone = state_arc.clone();
                    Button::new("collapse_gpu_pipeline")
                        .icon(IconName::Close)
                        .ghost()
                        .tooltip("Close")
                        .on_click(move |_, _, _| {
                            state_clone
                                .write()
                                .set_gpu_pipeline_overlay_collapsed(true);
                        })
                }),
        )
        // Table header
        .child(
            div()
                .w_full()
                .px_2()
                .py_1()
                .rounded_lg()
                .bg(cx.theme().sidebar.opacity(0.15))
                .border_1()
                .border_color(border.opacity(0.3))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .child(div().w(px(18.0)).flex_none())
                        .child(
                            div()
                                .w(px(220.0))
                                .flex_none()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().muted_foreground)
                                .child("Pass Name"),
                        )
                        .child(
                            div()
                                .w(px(70.0))
                                .flex_none()
                                .text_right()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().muted_foreground)
                                .child("Time"),
                        )
                        .child(
                            div()
                                .w(px(60.0))
                                .flex_none()
                                .text_right()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().muted_foreground)
                                .child("%"),
                        ),
                ),
        );

    // Add pass rows
    for (name, time_ms, percent, color) in pass_data {
        overlay = overlay.child(render_pass_stat(name, time_ms, percent, color, cx));
    }

    // Add totals if data exists
    if let Some(ref data) = gpu_data {
        overlay = overlay
            .child(div().w_full().h_px().bg(border.opacity(0.5)).my_2())
            .child(
                div()
                    .w_full()
                    .p_2()
                    .rounded_lg()
                    .bg(cx.theme().sidebar.opacity(0.2))
                    .border_1()
                    .border_color(if data.total_gpu_ms < 8.0 {
                        success.opacity(0.3)
                    } else if data.total_gpu_ms < 16.0 {
                        warning.opacity(0.3)
                    } else {
                        danger.opacity(0.3)
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .child(div().w(px(18.0)).flex_none())
                            .child(
                                div()
                                    .w(px(220.0))
                                    .flex_none()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(foreground)
                                    .child("Total GPU Time"),
                            )
                            .child(
                                div()
                                    .w(px(70.0))
                                    .flex_none()
                                    .text_right()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if data.total_gpu_ms < 8.0 {
                                        success
                                    } else if data.total_gpu_ms < 16.0 {
                                        warning
                                    } else {
                                        danger
                                    })
                                    .child(format!("{:.2}ms", data.total_gpu_ms)),
                            )
                            .child(
                                div()
                                    .w(px(60.0))
                                    .flex_none()
                                    .text_right()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("100%"),
                            ),
                    ),
            );
    }

    overlay.into_any_element()
}
