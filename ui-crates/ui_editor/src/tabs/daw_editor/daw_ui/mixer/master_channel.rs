use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::{Track, DawUiState, TrackId, DragState};
use std::sync::Arc;
use parking_lot::RwLock;

pub fn render_master_channel(state: &DawUiState, state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<super::super::panel::DawPanel>) -> impl IntoElement {
    let master_volume = state.project.as_ref()
        .map(|p| p.master_track.volume)
        .unwrap_or(1.0);
    let volume_db = 20.0 * master_volume.log10();
    let volume_percent = ((master_volume / 1.5) * 100.0).clamp(0.0, 100.0);

    v_flex()
        .w(px(90.0))
        .h_full()
        .gap_1()
        .p_2()
        .bg(cx.theme().accent.opacity(0.2))
        .rounded_lg()
        .border_2()
        .border_color(cx.theme().accent)
        .shadow_xl()
        // Master label with gradient bar
        .child(
            div()
                .w_full()
                .h(px(3.0))
                .bg(cx.theme().accent)
                .rounded_sm()
                .shadow_md()
        )
        .child(
            div()
                .w_full()
                .h(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .font_bold()
                        .text_color(cx.theme().accent)
                        .child("MASTER")
                )
        )
        // Spacer for insert slots
        .child(div().h(px(44.0)))
        // Master peak meters
        .child(super::master_meters::render_master_meters(state, cx))
        // Master fader
        .child(super::master_fader::render_master_fader(master_volume, state_arc.clone(), cx))
        // Master volume readout with warning color
        .child(
            div()
                .w_full()
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .font_bold()
                .text_color(if volume_db > 0.0 {
                    hsla(0.0, 0.95, 0.5, 1.0) // Red warning
                } else {
                    cx.theme().accent
                })
                .child(format!("{:+.1} dB", volume_db))
        )
}
