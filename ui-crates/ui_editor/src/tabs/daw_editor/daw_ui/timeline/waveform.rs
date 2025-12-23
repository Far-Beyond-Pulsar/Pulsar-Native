use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

//TODO: Implement proper waveform data display
pub fn render_waveform_placeholder(tint_color: Hsla, cx: &mut Context<DawPanel>) -> impl IntoElement {
    // Darken the tint color manually
    let darkened_color = hsla(
        tint_color.h,
        tint_color.s,
        (tint_color.l * 0.7).max(0.0),
        tint_color.a
    );

    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(IconName::Activity)
                .size_4()
                .text_color(darkened_color)
        )
}