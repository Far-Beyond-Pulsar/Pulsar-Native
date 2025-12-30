use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::super::DawPanel;
use super::{Track, DawUiState, TrackId, DragState};

pub fn render_peak_meters(track: &Track, state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    // Get actual meter data from audio service
    let (left_peak, right_peak) = if let Some(meter) = state.track_meters.get(&track.id) {
        (meter.peak_left, meter.peak_right)
    } else {
        (0.0, 0.0)
    };

    h_flex()
        .w_full()
        .h(px(48.0))
        .gap_1()
        .p_0p5()
        .bg(cx.theme().secondary.opacity(0.2))
        .rounded_sm()
        .child(super::meter_bar::render_meter_bar(left_peak, cx))
        .child(super::meter_bar::render_meter_bar(right_peak, cx))
}