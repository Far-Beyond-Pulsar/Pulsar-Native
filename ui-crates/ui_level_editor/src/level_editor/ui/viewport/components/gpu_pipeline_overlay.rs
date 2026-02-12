//! GPU pipeline statistics overlay component.

use std::sync::{Arc, Mutex};

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{h_flex, v_flex, ActiveTheme, StyledExt};

use crate::level_editor::ui::state::LevelEditorState;
use engine_backend::subsystems::render::helio_renderer::DiagnosticMetric;

/// Color palette for pipeline passes
const PASS_COLORS: &[(f32, f32, f32)] = &[
    (0.4, 0.7, 1.0),   // Light blue
    (1.0, 0.6, 0.4),   // Orange
    (0.6, 1.0, 0.6),   // Light green
    (1.0, 0.8, 0.4),   // Yellow
    (0.8, 0.6, 1.0),   // Purple
    (1.0, 0.6, 0.8),   // Pink
    (0.6, 0.9, 1.0),   // Cyan
    (1.0, 0.9, 0.6),   // Light yellow
];

/// Render the GPU pipeline overlay with fixed-width columns.
pub fn render_gpu_pipeline_overlay<V: 'static>(
    state: &LevelEditorState,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    cx: &mut Context<V>,
) -> impl IntoElement
where
    V: EventEmitter<ui::dock::PanelEvent> + Render,
{
    // Get GPU profiler data
    let gpu_data = if let Ok(engine) = gpu_engine.lock() {
        engine.get_gpu_profiler_data()
    } else {
        None
    };

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
        .w(px(340.0))
        .bg(background.opacity(0.95))
        .rounded_lg()
        .border_1()
        .border_color(border)
        .shadow_lg()
        .child(
            // Header
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .child("GPU Pipeline"),
        )
        .child(div().w_full().h(px(1.0)).bg(border))
        .map(|this| {
            if let Some(ref data) = gpu_data {
                // Calculate passes and percentages
                let mut render_passes: Vec<&DiagnosticMetric> = data
                    .render_metrics
                    .iter()
                    .filter(|metric| metric.is_gpu && metric.value_ms > 0.0)
                    .collect();

                render_passes.sort_by(|a, b| {
                    b.value_ms.partial_cmp(&a.value_ms).unwrap_or(std::cmp::Ordering::Equal)
                });

                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            // Column headers
                            h_flex()
                                .w_full()
                                .items_center()
                                .child(div().w(px(16.0)).flex_none())
                                .child(
                                    div()
                                        .w(px(180.0))
                                        .flex_none()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Pass")
                                )
                                .child(
                                    div()
                                        .w(px(60.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Time")
                                )
                                .child(
                                    div()
                                        .w(px(50.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("%")
                                )
                        )
                        .child(
                            // Scrollable pass list
                            div()
                                .id("gpu-pass-list")
                                .w_full()
                                .max_h(px(300.0))
                                .scrollable(gpui::Axis::Vertical)
                                .occlude()
                                .child(
                                    v_flex()
                                        .gap_0p5()
                                        .children(render_passes.iter().enumerate().map(|(i, metric)| {
                                            let color_idx = i % PASS_COLORS.len();
                                            let (r, g, b) = PASS_COLORS[color_idx];
                                            let color = hsla(r, g, b, 1.0);
                                            let percent = metric.percentage;

                                            h_flex()
                                                .w_full()
                                                .items_center()
                                                .child(
                                                    // Color indicator
                                                    div()
                                                        .w(px(16.0))
                                                        .flex_none()
                                                        .child(
                                                            div()
                                                                .w(px(8.0))
                                                                .h(px(8.0))
                                                                .rounded(px(2.0))
                                                                .bg(color)
                                                        )
                                                )
                                                .child(
                                                    // Pass name
                                                    div()
                                                        .w(px(180.0))
                                                        .flex_none()
                                                        .overflow_hidden()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .line_height(relative(1.0))
                                                        .whitespace_nowrap()
                                                        .child(metric.name.clone())
                                                )
                                                .child(
                                                    // Time
                                                    div()
                                                        .w(px(60.0))
                                                        .flex_none()
                                                        .text_right()
                                                        .text_xs()
                                                        .text_color(foreground)
                                                        .child(format!("{:.2}ms", metric.value_ms))
                                                )
                                                .child(
                                                    // Percentage
                                                    div()
                                                        .w(px(50.0))
                                                        .flex_none()
                                                        .text_right()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(format!("{:.1}%", percent))
                                                )
                                        }))
                                )
                        )
                        .child(div().w_full().h(px(1.0)).bg(border).mt_1())
                        .child(
                            // Total GPU row
                            h_flex()
                                .w_full()
                                .items_center()
                                .child(div().w(px(16.0)).flex_none())
                                .child(
                                    div()
                                        .w(px(180.0))
                                        .flex_none()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(foreground)
                                        .child("Total GPU")
                                )
                                .child(
                                    div()
                                        .w(px(60.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if data.total_gpu_ms < 8.0 {
                                            success
                                        } else if data.total_gpu_ms < 16.0 {
                                            warning
                                        } else {
                                            danger
                                        })
                                        .child(format!("{:.2}ms", data.total_gpu_ms))
                                )
                                .child(
                                    div()
                                        .w(px(50.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("100.0%")
                                )
                        )
                        .child(
                            // Frame time / FPS row
                            h_flex()
                                .w_full()
                                .items_center()
                                .child(div().w(px(16.0)).flex_none())
                                .child(
                                    div()
                                        .w(px(180.0))
                                        .flex_none()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("Frame Time")
                                )
                                .child(
                                    div()
                                        .w(px(60.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .text_color(foreground)
                                        .child(format!("{:.2}ms", data.total_gpu_ms))
                                )
                                .child(
                                    div()
                                        .w(px(50.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color({
                                            let fps = 1000.0 / data.total_gpu_ms.max(0.1);
                                            if fps > 60.0 {
                                                success
                                            } else if fps > 30.0 {
                                                warning
                                            } else {
                                                danger
                                            }
                                        })
                                        .child(format!("{:.0} FPS", 1000.0 / data.total_gpu_ms.max(0.1)))
                                )
                        )
                )
            } else {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("No GPU data available"),
                )
            }
        })
        .into_any_element()
}
