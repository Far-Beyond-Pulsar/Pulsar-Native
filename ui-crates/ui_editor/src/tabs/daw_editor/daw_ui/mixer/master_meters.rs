use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::super::DawPanel;
use super::{Track, DawUiState, TrackId, DragState};

pub fn render_master_meters(state: &DawUiState, cx: &mut Context<super::super::panel::DawPanel>) -> impl IntoElement {
    let (left_peak, right_peak) = (state.master_meter.peak_left, state.master_meter.peak_right);

    h_flex()
        .w_full()
        .h(px(48.0))
        .gap_1()
        .p_0p5()
        .bg(cx.theme().secondary.opacity(0.25))
        .rounded_sm()
        .child(super::meter_bar::render_meter_bar(left_peak, cx))
        .child(super::meter_bar::render_meter_bar(right_peak, cx))
}
