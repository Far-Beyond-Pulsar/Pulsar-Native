use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

/// Render grid lines for a segment
pub fn render_grid_lines_segment(
    state: &DawUiState,
    start_x: f32,
    segment_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let beat_width = state.beats_to_pixels(1.0);
    let start_beat = (start_x / beat_width).floor() as i32;
    let end_beat = ((start_x + segment_width) / beat_width).ceil() as i32;
    
    div()
        .absolute()
        .inset_0()
        .children((start_beat..=end_beat).map(|beat| {
            let x = state.beats_to_pixels(beat as f64) - start_x;
            let is_bar = beat % 4 == 0;
            
            div()
                .absolute()
                .left(px(x))
                .top_0()
                .bottom_0()
                .w_px()
                .bg(if is_bar {
                    cx.theme().border
                } else {
                    cx.theme().border.opacity(0.3)
                })
        }))
}