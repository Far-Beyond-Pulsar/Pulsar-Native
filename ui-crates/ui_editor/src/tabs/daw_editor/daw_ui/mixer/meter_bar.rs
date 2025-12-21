use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::super::DawPanel;
use super::{Track, DawUiState, TrackId, DragState};

pub fn render_meter_bar(level: f32, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let level_clamped = level.clamp(0.0, 1.0);
    let segments = 12;

    v_flex()
        .flex_1()
        .gap_0p5()
        .flex_col_reverse() // Bottom to top
        .children((0..segments).map(move |seg| {
            let threshold = seg as f32 / segments as f32;
            let is_lit = level_clamped >= threshold;

            // Professional color gradient: green -> yellow -> orange -> red
            let color = if seg > 10 {
                hsla(0.0, 0.95, 0.5, 1.0) // Bright Red
            } else if seg > 8 {
                hsla(30.0 / 360.0, 0.95, 0.5, 1.0) // Orange
            } else if seg > 6 {
                hsla(60.0 / 360.0, 0.95, 0.5, 1.0) // Yellow
            } else {
                hsla(120.0 / 360.0, 0.8, 0.5, 1.0) // Green
            };

            div()
                .w_full()
                .h(px(3.0))
                .rounded_sm()
                .bg(if is_lit {
                    color
                } else {
                    cx.theme().secondary.opacity(0.25)
                })
                .when(is_lit, |d| d.shadow_sm())
        }))
}