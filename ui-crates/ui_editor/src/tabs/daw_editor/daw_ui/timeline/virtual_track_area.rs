use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

/// Render virtualized track area - using Table pattern with uniform_list
pub fn render_virtual_track_area(
    state: &DawUiState,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let panel_entity = cx.entity().clone();
    let num_tracks = state.project.as_ref().map(|p| p.tracks.len()).unwrap_or(0);
    let total_beats = 500.0;
    let total_width = state.beats_to_pixels(total_beats);
    let vertical_scroll_handle = state.timeline_vertical_scroll_handle.clone();
    
    div()
        .flex_1()
        .w_full()
        .relative()
        .overflow_hidden()
        .child(
            uniform_list(
                "timeline-track-rows",
                num_tracks,
                cx.processor(move |panel: &mut DawPanel, visible_range: Range<usize>, _window, cx| {
                    let tracks = panel.state.project.as_ref()
                        .map(|p| &p.tracks)
                        .map(|t| t.as_slice())
                        .unwrap_or(&[]);
                    let total_width = panel.state.beats_to_pixels(500.0);
                    
                    visible_range.into_iter().filter_map(|track_idx| {
                        tracks.get(track_idx).map(|track| {
                            super::track_row::render_track_row(track, &panel.state, total_width, cx)
                        })
                    }).collect()
                })
            )
            .size_full()
            .track_scroll(vertical_scroll_handle)
            .with_sizing_behavior(ListSizingBehavior::Infer)
        )
        // Scrollbar overlay for horizontal scrolling
        .child(
            div()
                .absolute()
                .inset_0()
                .child(
                    Scrollbar::both(
                        &state.timeline_scroll_state,
                        &state.timeline_scroll_handle,
                    )
                    .axis(ScrollbarAxis::Horizontal)
                )
        )
}

