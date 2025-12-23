use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::{Track, DawUiState, TrackId, DragState};
use std::sync::Arc;
use parking_lot::RwLock;

pub fn render_send_controls(
    track: &Track,
    track_id: TrackId,
    state_arc: Arc<RwLock<DawUiState>>,
    cx: &mut Context<super::super::panel::DawPanel>,
) -> impl IntoElement {
    // Get send values from track if available
    let send_a_amount = track.sends.get(0).map(|s| s.amount).unwrap_or(0.0);
    let send_a_pre = track.sends.get(0).map(|s| s.pre_fader).unwrap_or(false);
    let send_b_amount = track.sends.get(1).map(|s| s.amount).unwrap_or(0.0);
    let send_b_pre = track.sends.get(1).map(|s| s.pre_fader).unwrap_or(false);

    v_flex()
        .w_full()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("SENDS")
        )
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .child(super::send_row::render_send_row("A", send_a_amount, send_a_pre, track_id, 0, state_arc.clone(), cx))
                .child(super::send_row::render_send_row("B", send_b_amount, send_b_pre, track_id, 1, state_arc.clone(), cx))
        )
}
