pub use super::state::*;
pub use super::panel::DawPanel;
pub use gpui::*;
pub use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Icon, IconName, StyledExt, ActiveTheme,
    scroll::{Scrollbar, ScrollbarAxis}, PixelsExt, h_virtual_list};
pub use std::rc::Rc;
pub use std::ops::Range;
pub use crate::tabs::daw_editor::audio_types::SAMPLE_RATE;

mod clip;
mod drop_zone;
mod grid_lines;
mod grid_lines_segment;
mod playhead;
mod ruler_segment;
mod ruler;
mod track_content;
mod track_content_segment;
mod track_row;
mod virtual_track_area;
mod waveform;

pub const TIMELINE_HEADER_HEIGHT: f32 = 40.0;
pub const TRACK_HEADER_WIDTH: f32 = 200.0;
pub const MIN_TRACK_HEIGHT: f32 = 60.0;
pub const MAX_TRACK_HEIGHT: f32 = 300.0;

pub fn render_timeline(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    // Prepare virtualization item sizes for tracks
    let track_sizes: Rc<Vec<Size<Pixels>>> = {
        let tracks = state.project.as_ref()
            .map(|p| p.tracks.as_slice())
            .unwrap_or(&[]);
        
        Rc::new(
            tracks.iter()
                .map(|track| {
                    let height = *state.track_heights.get(&track.id)
                        .unwrap_or(&state.viewport.track_height);
                    Size {
                        width: px(9999.0), // Will be constrained by layout
                        height: px(height),
                    }
                })
                .collect()
        )
    };

    let panel_entity = cx.entity().clone();

    v_flex()
        .size_full()
        .bg(cx.theme().background)
        // Ruler/timeline header
        .child(ruler::render_ruler(state, cx))
        // Scrollable track area with virtualization
        .child(virtual_track_area::render_virtual_track_area(state, track_sizes, cx))
}