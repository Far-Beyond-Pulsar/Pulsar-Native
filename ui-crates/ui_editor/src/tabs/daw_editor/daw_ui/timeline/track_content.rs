use super::*;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};

/// Render track timeline content (clips, automation, etc.)
pub fn render_track_content(
    track: &crate::tabs::daw_editor::audio_types::Track,
    state: &DawUiState,
    total_width: f32,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let track_id = track.id;
    let track_height = *state.track_heights.get(&track_id)
        .unwrap_or(&state.viewport.track_height);

    div()
        .w(px(total_width))
        .h_full()
        .relative()
        .bg(cx.theme().background)
        // Grid lines
        .child(super::grid_lines::render_grid_lines(state, cx))
        // Drop zone for dragging files/clips
        .child(super::drop_zone::render_drop_zone(track_id, state, cx))
        // Render clips
        .children(track.clips.iter().map(|clip| {
            super::clip::render_clip(clip, track_id, state, cx)
        }))
}