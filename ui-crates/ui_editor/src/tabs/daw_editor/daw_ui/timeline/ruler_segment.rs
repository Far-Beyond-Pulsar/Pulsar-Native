use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

/// Render a segment of the ruler for horizontal virtual scrolling
pub fn render_ruler_segment(
    state: &DawUiState,
    start_x: f32,
    segment_width: f32,
    total_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    div()
        .w(px(segment_width))
        .h_full()
        .relative()
        .bg(cx.theme().muted)
        // Render beat markings that intersect with this segment
        .child(
            div()
                .absolute()
                .left(px(-start_x))  // Offset to account for segment position
                .top_0()
                .w(px(total_width))  // Full timeline width
                .h_full()
                .children((0..=125).filter_map(|bar| {  // 500 beats / 4 = 125 bars
                    let beat = (bar * 4) as f64;
                    let x = state.beats_to_pixels(beat);
                    
                    // Only render if bar marking intersects with this segment
                    if x >= start_x && x < start_x + segment_width {
                        Some(div()
                            .absolute()
                            .left(px(x))
                            .h_full()
                            .child(
                                v_flex()
                                    .h_full()
                                    .child(
                                        div()
                                            .px_2()
                                            .text_xs()
                                            .font_family("monospace")
                                            .text_color(cx.theme().foreground)
                                            .child(format!("{}", bar + 1))
                                    )
                                    .child(
                                        div()
                                            .w_px()
                                            .flex_1()
                                            .bg(cx.theme().border)
                                    )
                            ))
                    } else {
                        None
                    }
                }))
        )
        // Playhead (full width, offset to account for segment position)
        .child(
            div()
                .absolute()
                .left(px(-start_x))  // Offset to account for segment position
                .top_0()
                .w(px(total_width))  // Full timeline width
                .h_full()
                .child(super::playhead::render_playhead(state, cx))
        )
}


