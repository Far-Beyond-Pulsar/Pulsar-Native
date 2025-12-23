use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

/// Render grid lines
pub fn render_grid_lines(state: &DawUiState, cx: &mut Context<super::super::panel::DawPanel>) -> impl IntoElement {
    let _zoom = state.viewport.zoom;
    let num_beats = 500; // Total beats to show

    div()
        .absolute()
        .inset_0()
        
        .children((0..num_beats).step_by(4).map(|beat| {
            let x = state.beats_to_pixels(beat as f64);

            div()
                .absolute()
                .left(px(x))
                .top_0()
                .bottom_0()
                .w_px()
                .bg(cx.theme().border.opacity(0.3))
        }))
}

