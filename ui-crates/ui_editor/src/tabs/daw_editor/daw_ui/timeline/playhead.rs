use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

pub fn render_playhead(state: &mut DawUiState, cx: &mut Context<super::super::panel::DawPanel>) -> impl IntoElement {
    let x = state.beats_to_pixels(state.selection.playhead_position);

    div()
        .absolute()
        .left(px(x))
        .top_0()
        .bottom_0()
        .w(px(2.0))
        .bg(cx.theme().accent)
        
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(-6.0))
                .w(px(14.0))
                .h(px(14.0))
                .bg(cx.theme().accent)
                .child(
                    Icon::new(IconName::Play)
                        .size_3()
                        .text_color(cx.theme().accent_foreground)
                )
        )
}

