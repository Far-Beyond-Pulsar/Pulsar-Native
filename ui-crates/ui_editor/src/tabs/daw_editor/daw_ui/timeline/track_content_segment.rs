use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

/// Render a segment of the track content for horizontal virtual scrolling
pub fn render_track_content_segment(
    track: &crate::tabs::daw_editor::audio_types::Track,
    state: &DawUiState,
    start_x: f32,
    segment_width: f32,
    _total_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let track_id = track.id;

    div()
        .w(px(segment_width))
        .h_full()
        .relative()
        .bg(cx.theme().background)
        // Grid lines for this segment
        .child(super::grid_lines_segment::render_grid_lines_segment(state, start_x, segment_width, cx))
        // Drop zone for the entire timeline (full width, positioned to cover this segment)
        .child(
            div()
                .absolute()
                .left(px(-start_x))  // Offset to account for segment position
                .top_0()
                .w(px(state.beats_to_pixels(500.0)))  // Full timeline width
                .h_full()
                .child(super::drop_zone::render_drop_zone(track_id, state, cx))
        )
        // Render clips that intersect with this segment
        // Clips are absolutely positioned, so we render the full content but clipped to segment
        .child(
            div()
                .absolute()
                .left(px(-start_x))  // Offset to account for segment position
                .top_0()
                .w(px(state.beats_to_pixels(500.0)))  // Full timeline width
                .h_full()
                .children(track.clips.iter().filter_map(|clip| {
                    let tempo = state.get_tempo();
                    let clip_start = state.beats_to_pixels(clip.start_beat(tempo));
                    let clip_end = clip_start + state.beats_to_pixels(clip.duration_beats(tempo));
                    
                    // Only render if clip intersects with this segment
                    if clip_end >= start_x && clip_start < start_x + segment_width {
                        Some(super::clip::render_clip(clip, track_id, state, cx))
                    } else {
                        None
                    }
                }))
        )
}