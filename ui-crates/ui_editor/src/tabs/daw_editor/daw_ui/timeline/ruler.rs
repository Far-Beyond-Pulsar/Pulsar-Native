use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

pub fn render_ruler(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let _tempo = state.project.as_ref().map(|p| p.transport.tempo).unwrap_or(120.0);
    let _zoom = state.viewport.zoom;
    let horizontal_scroll_handle = state.timeline_scroll_handle.clone();
    let view = cx.entity().clone();

    h_flex()
        .w_full()
        .h(px(TIMELINE_HEADER_HEIGHT))
        .bg(cx.theme().muted)
        .border_b_1()
        .border_color(cx.theme().border)
        // Track header spacer
        .child(
            div()
                .w(px(TRACK_HEADER_WIDTH))
                .h_full()
                .border_r_1()
                .border_color(cx.theme().border)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().muted_foreground)
                        .child("TRACKS")
                )
        )
        // Timeline ruler with beat markings - now scrollable!
        .child(
            div()
                .flex_1()
                .h_full()
                .relative()
                .overflow_hidden()
                .child(
                    h_virtual_list(
                        view,
                        "ruler",
                        {
                            let total_width = state.beats_to_pixels(500.0);
                            let segment_width = 100.0;
                            let num_segments = (total_width / segment_width).ceil() as usize;
                            Rc::new(
                                (0..num_segments).map(|_| Size {
                                    width: px(segment_width),
                                    height: px(TIMELINE_HEADER_HEIGHT),
                                }).collect()
                            )
                        },
                        move |panel, visible_range, _window, cx| {
                            let total_width = panel.state.beats_to_pixels(500.0);
                            let segment_width = 100.0;
                            
                            visible_range.into_iter().map(|segment_idx| {
                                let start_x = segment_idx as f32 * segment_width;
                                super::ruler_segment::render_ruler_segment(&panel.state, start_x, segment_width, total_width, cx)
                            }).collect()
                        },
                    )
                    .track_scroll(&horizontal_scroll_handle)
                )
        )
}