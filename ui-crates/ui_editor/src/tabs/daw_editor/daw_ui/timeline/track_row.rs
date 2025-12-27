use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

/// Render a single track row - fixed header on left, scrollable content on right (Table pattern)
pub fn render_track_row(
    track: &crate::tabs::daw_editor::audio_types::Track,
    state: &DawUiState,
    total_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let track_id = track.id;
    let track_height = *state.track_heights.get(&track_id)
        .unwrap_or(&state.viewport.track_height);
    let horizontal_scroll_handle = state.timeline_scroll_handle.clone();
    let view = cx.entity().clone();
    
    // For horizontal virtual scrolling, we need to split the timeline into segments
    // Let's use 100px wide segments
    let segment_width = 100.0;
    let num_segments = (total_width / segment_width).ceil() as usize;
    let segment_sizes: Rc<Vec<Size<Pixels>>> = Rc::new(
        (0..num_segments).map(|_| Size {
            width: px(segment_width),
            height: px(track_height),
        }).collect()
    );
    
    h_flex()
        .w_full()
        .h(px(track_height))
        .border_b_1()
        .border_color(cx.theme().border)
        // Fixed left: track header (like Table's fixed left columns)
        .child(
            div()
                .w(px(TRACK_HEADER_WIDTH))
                .h_full()
                .border_r_1()
                .border_color(cx.theme().border)
                .child(super::super::track_header::render_track_header(track, state, cx))
        )
        // Scrollable right: track content using horizontal virtual_list (like Table's scrollable columns)
        .child(
            div()
                .flex_1()
                .h_full()
                .overflow_hidden()
                .relative()
                .child(
                    h_virtual_list(
                        view.clone(),
                        track_id,
                        segment_sizes,
                        {
                            let track = track.clone();
                            move |panel, visible_range, _window, cx| {
                                let total_width = panel.state.beats_to_pixels(500.0);
                                let segment_width = 100.0;
                                
                                // Render the visible segments of the timeline
                                visible_range.into_iter().map(|segment_idx| {
                                    let start_x = segment_idx as f32 * segment_width;
                                    let end_x = ((segment_idx + 1) as f32 * segment_width).min(total_width);
                                    let segment_w = end_x - start_x;
                                    
                                    super::track_content_segment::render_track_content_segment(&track, &panel.state, start_x, segment_w, total_width, cx)
                                }).collect()
                            }
                        },
                    )
                    .track_scroll(&horizontal_scroll_handle)
                )
                // Add ScrollableMask as a sibling to enable scroll wheel handling
                .child(
                    ui::scroll::ScrollableMask::new(
                        view.entity_id(),
                        Axis::Horizontal,
                        horizontal_scroll_handle.as_ref(),
                    )
                )
        )
}