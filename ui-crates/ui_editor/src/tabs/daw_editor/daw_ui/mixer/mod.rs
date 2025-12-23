/// Mixer View Component
/// Studio-quality channel strips with faders, pan, sends, meters, and insert effects
/// Designed for professional music production with smooth animations and precise control

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use crate::tabs::daw_editor::audio_types::{Track, TrackId};
use std::rc::Rc;

pub mod add_channel_button;
pub mod channel_strip;
pub mod fader_slider;
pub mod insert_slots;
pub mod master_channel;
pub mod meter_bar;
pub mod output_routing;
pub mod peak_meters;
pub mod send_controls;
pub mod send_row;
pub mod master_meters;
pub mod master_fader;



pub const CHANNEL_STRIP_WIDTH: f32 = 90.0;
pub const MIXER_PADDING: f32 = 8.0;

pub fn render_mixer(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let num_tracks = state.project.as_ref()
        .map(|p| p.tracks.len())
        .unwrap_or(0);

    // Prepare item sizes for horizontal virtualization
    let channel_sizes: Rc<Vec<Size<Pixels>>> = {
        // num_tracks + add button + master = total items
        let total_items = num_tracks + 2;
        Rc::new(
            (0..total_items).map(|_| Size {
                width: px(CHANNEL_STRIP_WIDTH),
                height: px(400.0), // Fixed mixer height to match panel
            }).collect()
        )
    };

    let panel_entity = cx.entity().clone();

    div()
        .w_full()
        .h_full()
        .relative()
        .overflow_hidden()
        .child(
            h_virtual_list(
                panel_entity.clone(),
                "mixer-channels",
                channel_sizes,
                move |panel, visible_range, _, cx| {
                    let num_tracks = panel.state.project.as_ref()
                        .map(|p| p.tracks.len())
                        .unwrap_or(0);

                    visible_range.filter_map(|idx| {
                        if idx < num_tracks {
                            // Render track channel
                            if let Some(ref project) = panel.state.project {
                                if idx < project.tracks.len() {
                                    let track = &project.tracks[idx];
                                    return Some(channel_strip::render_channel_strip(track, idx, &panel.state, cx).into_any_element());
                                }
                            }
                            None
                        } else if idx == num_tracks {
                            // Render add channel button
                            Some(add_channel_button::render_add_channel_button(cx).into_any_element())
                        } else if idx == num_tracks + 1 {
                            // Render master channel
                            Some(master_channel::render_master_channel(&panel.state, cx).into_any_element())
                        } else {
                            None
                        }
                    }).collect::<Vec<_>>()
                },
            )
            .track_scroll(&state.mixer_scroll_handle)
            .px(px(MIXER_PADDING))
            .py_2()
            .bg(cx.theme().muted.opacity(0.15))
            .gap_2()
        )
        .child(
            // Scrollbar overlay
            div()
                .absolute()
                .inset_0()
                .child(
                    Scrollbar::both(
                        &state.mixer_scroll_state,
                        &state.mixer_scroll_handle,
                    )
                    .axis(ScrollbarAxis::Horizontal)
                )
        )
}