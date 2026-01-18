//! GPU pipeline statistics overlay component.

use std::sync::{Arc, Mutex};

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{h_flex, v_flex, ActiveTheme, StyledExt};

use crate::level_editor::ui::state::LevelEditorState;

/// Render the GPU pipeline overlay with compact, efficient design.
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

    let (background, border, foreground, muted) = {
        let theme = cx.theme();
        (
            theme.background,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
        )
    };

    // Stat row helper
    let stat_row = |label: String, value: String, color: Option<Hsla>| {
        h_flex()
            .w_full()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(label.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color.unwrap_or(foreground))
                    .child(value),
            )
    };

    let mut content = v_flex()
        .gap_1()
        .p_2()
        .w_full()
        .bg(background.opacity(0.85))
        .rounded_lg()
        .border_1()
        .border_color(border.opacity(0.3))
        .shadow_md()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .pb_1()
                .border_b_1()
                .border_color(border.opacity(0.2))
                .child("GPU Pipeline"),
        );

    if let Some(ref data) = gpu_data {
        let theme = cx.theme();
        let total_color = if data.total_gpu_ms < 8.0 {
            theme.success
        } else if data.total_gpu_ms < 16.0 {
            theme.warning
        } else {
            theme.danger
        };

        content = content
            .child(stat_row("Total Frame".to_string(), format!("{:.2}ms", data.total_gpu_ms), Some(total_color)))
            .child(stat_row("FPS Target".to_string(), format!("{:.0} fps", 1000.0 / data.total_gpu_ms.max(0.1)), None));

        // Show top 5 passes
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

        if !render_passes.is_empty() {
            content = content.child(
                div()
                    .w_full()
                    .h_px()
                    .bg(border.opacity(0.2))
                    .my_1(),
            );

            for metric in render_passes.iter().take(5) {
                let name = metric.name.replace("render/", "");
                content = content.child(stat_row(
                    name,
                    format!("{:.2}ms", metric.value_ms),
                    None,
                ));
            }
        }
    } else {
        content = content.child(
            div()
                .text_xs()
                .text_color(muted)
                .child("No GPU data available"),
        );
    }

    content.into_any_element()
}
