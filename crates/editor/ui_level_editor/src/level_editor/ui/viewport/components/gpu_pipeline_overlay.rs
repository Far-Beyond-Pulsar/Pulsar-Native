//! Render pipeline timing overlay component.

use std::sync::{Arc, Mutex};

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme, StyledExt};

use crate::level_editor::state::LevelEditorState;
use engine_backend::subsystems::render::helio_renderer::{
    DiagnosticMetric, GpuProfilerAvailability,
};

const PASS_COLORS: &[(f32, f32, f32)] = &[
    (0.4, 0.7, 1.0),
    (1.0, 0.6, 0.4),
    (0.6, 1.0, 0.6),
    (1.0, 0.8, 0.4),
    (0.8, 0.6, 1.0),
    (1.0, 0.6, 0.8),
    (0.6, 0.9, 1.0),
    (1.0, 0.9, 0.6),
];

fn time_label(time_ms: Option<f32>) -> String {
    time_ms
        .map(|time| format!("{time:.2}ms"))
        .unwrap_or_else(|| "—".to_owned())
}

fn timing_color(time_ms: Option<f32>, success: Hsla, warning: Hsla, danger: Hsla) -> Hsla {
    match time_ms {
        Some(time) if time < 8.0 => success,
        Some(time) if time < 16.0 => warning,
        Some(_) => danger,
        None => warning,
    }
}

/// Render the latest non-blocking Helio pass timings.
pub fn render_gpu_pipeline_overlay<V>(
    _state: &LevelEditorState,
    _state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    cx: &mut Context<V>,
) -> impl IntoElement
where
    V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
{
    // Cloning is deliberately conditional on the overlay being visible. The
    // render thread itself updates a reusable cache without allocating.
    let profiler_data = gpu_engine
        .try_lock()
        .ok()
        .and_then(|engine| engine.get_gpu_profiler_data());

    let (background, border, foreground, muted, success, warning, danger) = {
        let theme = cx.theme();
        (
            theme.background,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
            theme.success,
            theme.warning,
            theme.danger,
        )
    };

    v_flex()
        .gap_2()
        .p_3()
        .w(px(410.0))
        .bg(background.opacity(0.95))
        .rounded_lg()
        .border_1()
        .border_color(border)
        .shadow_lg()
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child("Render Pipeline"),
                )
                .when_some(profiler_data.as_ref(), |header, data| {
                    let (label, color) = match data.availability {
                        GpuProfilerAvailability::Disabled => ("GPU disabled", muted),
                        GpuProfilerAvailability::Unsupported => ("GPU unsupported", warning),
                        GpuProfilerAvailability::Pending => ("GPU pending", warning),
                        GpuProfilerAvailability::Available => ("GPU available", success),
                        GpuProfilerAvailability::Backpressured => ("GPU backpressured", danger),
                    };
                    header.child(div().text_xs().text_color(color).child(label))
                }),
        )
        .child(div().w_full().h(px(1.0)).bg(border))
        .map(|this| {
            if let Some(ref data) = profiler_data {
                let mut render_passes: Vec<&DiagnosticMetric> = data
                    .render_metrics
                    .iter()
                    .filter(|metric| metric.cpu_ms.is_some() || metric.gpu_ms.is_some())
                    .collect();
                render_passes.sort_by(|a, b| {
                    let a_time = a.gpu_ms.or(a.cpu_ms).unwrap_or_default();
                    let b_time = b.gpu_ms.or(b.cpu_ms).unwrap_or_default();
                    b_time.total_cmp(&a_time)
                });

                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .child(div().w(px(16.0)).flex_none())
                                .child(
                                    div()
                                        .w(px(210.0))
                                        .flex_none()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Pass"),
                                )
                                .child(
                                    div()
                                        .w(px(65.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("CPU"),
                                )
                                .child(
                                    div()
                                        .w(px(65.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("GPU"),
                                ),
                        )
                        .child(
                            div()
                                .id("gpu-pass-list")
                                .w_full()
                                .max_h(px(300.0))
                                .scrollable(gpui::Axis::Vertical)
                                .occlude()
                                .child(v_flex().gap_0p5().children(
                                    render_passes.iter().enumerate().map(|(index, metric)| {
                                        let (r, g, b) = PASS_COLORS[index % PASS_COLORS.len()];
                                        h_flex()
                                            .w_full()
                                            .items_center()
                                            .child(
                                                div().w(px(16.0)).flex_none().child(
                                                    div()
                                                        .w(px(8.0))
                                                        .h(px(8.0))
                                                        .rounded(px(2.0))
                                                        .bg(hsla(r, g, b, 1.0)),
                                                ),
                                            )
                                            .child(
                                                div()
                                                    .w(px(210.0))
                                                    .flex_none()
                                                    .overflow_hidden()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .line_height(relative(1.0))
                                                    .whitespace_nowrap()
                                                    .child(metric.name),
                                            )
                                            .child(
                                                div()
                                                    .w(px(65.0))
                                                    .flex_none()
                                                    .text_right()
                                                    .text_xs()
                                                    .text_color(foreground)
                                                    .child(time_label(metric.cpu_ms)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(65.0))
                                                    .flex_none()
                                                    .text_right()
                                                    .text_xs()
                                                    .text_color(foreground)
                                                    .child(time_label(metric.gpu_ms)),
                                            )
                                    }),
                                )),
                        )
                        .child(div().w_full().h(px(1.0)).bg(border).mt_1())
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .child(div().w(px(16.0)).flex_none())
                                .child(
                                    div()
                                        .w(px(210.0))
                                        .flex_none()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(foreground)
                                        .child("Pass totals"),
                                )
                                .child(
                                    div()
                                        .w(px(65.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(timing_color(
                                            data.total_cpu_ms,
                                            success,
                                            warning,
                                            danger,
                                        ))
                                        .child(time_label(data.total_cpu_ms)),
                                )
                                .child(
                                    div()
                                        .w(px(65.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(timing_color(
                                            data.total_gpu_ms,
                                            success,
                                            warning,
                                            danger,
                                        ))
                                        .child(time_label(data.total_gpu_ms)),
                                ),
                        )
                        .child(div().text_xs().text_color(muted).child(format!(
                                "CPU frame {} · GPU frame {} · lag {} · drops {} · overflows {}",
                                data.frame_count,
                                data.gpu_frame_count
                                    .map(|frame| frame.to_string())
                                    .unwrap_or_else(|| "—".to_owned()),
                                data.gpu_lag_frames
                                    .map(|lag| lag.to_string())
                                    .unwrap_or_else(|| "—".to_owned()),
                                data.readback_drops,
                                data.query_overflows
                            ))),
                )
            } else {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Renderer busy; keeping the previous frame responsive"),
                )
            }
        })
        .into_any_element()
}
