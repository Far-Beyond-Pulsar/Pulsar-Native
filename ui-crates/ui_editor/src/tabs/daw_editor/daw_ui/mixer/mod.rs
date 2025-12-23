/// Mixer View Component
/// Studio-quality channel strips with faders, pan, sends, meters, and insert effects
/// Designed for professional music production with smooth animations and precise control

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use std::sync::{Arc, RwLock};
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

pub fn render_mixer(state: &mut DawUiState, state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<super::panel::DawPanel>) -> impl IntoElement {
    let tracks = state.project.as_ref()
        .map(|p| p.tracks.clone())
        .unwrap_or_default();

    div()
        .w_full()
        .h_full()
        .relative()
        .child(
            h_flex()
                .px(px(MIXER_PADDING))
                .py_2()
                .bg(cx.theme().muted.opacity(0.15))
                .gap_2()
                .items_start()
                // Render all track channels
                .children(tracks.iter().enumerate().map(|(idx, track)| {
                    channel_strip::render_channel_strip(track, idx, state, state_arc.clone(), cx).into_any_element()
                }))
                // Add channel button
                .child(add_channel_button::render_add_channel_button(state_arc.clone(), cx))
                // Master channel
                .child(master_channel::render_master_channel(state, state_arc.clone(), cx))
        )
}
