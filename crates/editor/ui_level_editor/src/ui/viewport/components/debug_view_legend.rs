//! Reusable floating color scale for viewport debug views.

use engine_backend::subsystems::render::ViewportDebugLegend;
use gpui::*;
use ui::{h_flex, v_flex};

/// Render the optional scale advertised by a viewport debug mode.
pub fn render_debug_view_legend(
    legend: &ViewportDebugLegend,
    theme: &ui::Theme,
) -> impl IntoElement {
    let mut ramp = div()
        .flex()
        .w(px(252.0))
        .h(px(9.0))
        .rounded(px(4.0))
        .overflow_hidden();

    for index in 0..32 {
        let color = sample_scale(legend, (index as f32 + 0.5) / 32.0);
        let packed = (u32::from(color[0]) << 16) | (u32::from(color[1]) << 8) | u32::from(color[2]);
        ramp = ramp.child(div().flex_1().h_full().bg(rgb(packed)));
    }

    let title = if legend.unit.is_empty() {
        legend.title.to_owned()
    } else {
        format!("{} ({})", legend.title, legend.unit)
    };
    let mut labels = h_flex()
        .w(px(252.0))
        .justify_between()
        .text_xs()
        .text_color(theme.muted_foreground);
    for stop in legend.stops {
        labels = labels.child(stop.label);
    }

    v_flex()
        .w(px(284.0))
        .gap_1()
        .p_2()
        .rounded(theme.radius)
        .bg(theme.background.opacity(0.9))
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(title),
        )
        .child(ramp)
        .child(labels)
}

fn sample_scale(legend: &ViewportDebugLegend, position: f32) -> [u8; 3] {
    let stops = legend.stops;
    if stops.is_empty() {
        return [255, 255, 255];
    }
    if stops.len() == 1 {
        return stops[0].color;
    }

    let scaled = position.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let left = scaled.floor() as usize;
    let right = (left + 1).min(stops.len() - 1);
    let blend = scaled - left as f32;
    std::array::from_fn(|channel| {
        let a = f32::from(stops[left].color[channel]);
        let b = f32::from(stops[right].color[channel]);
        (a + (b - a) * blend).round() as u8
    })
}
