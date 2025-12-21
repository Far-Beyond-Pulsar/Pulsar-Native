use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::super::DawPanel;
use super::{Track, DawUiState};

pub fn render_channel_strip(
    track: &Track,
    idx: usize,
    state: &DawUiState,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let is_selected = state.selection.selected_track_ids.contains(&track.id);
    let is_muted = track.muted || state.is_track_effectively_muted(track.id);
    let track_id = track.id;

    // Beautiful color per track with golden ratio
    let track_hue = (idx as f32 * 137.5) % 360.0;
    let track_color = hsla(track_hue / 360.0, 0.7, 0.5, 1.0);

    v_flex()
        .w(px(90.0))
        .h_full()
        .gap_1()
        .p_2()
        .bg(if is_selected {
            cx.theme().accent.opacity(0.25)
        } else {
            cx.theme().muted.opacity(0.15)
        })
        .rounded_lg()
        .border_1()
        .border_color(if is_selected {
            track_color.opacity(0.9)
        } else {
            cx.theme().border.opacity(0.6)
        })
        .shadow_md()
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(if is_selected {
                    cx.theme().accent.opacity(0.3)
                } else {
                    cx.theme().muted.opacity(0.2)
                })
                .shadow_lg()
        })
        .on_mouse_down(MouseButton::Left, cx.listener(move |panel, _event: &MouseDownEvent, _window, cx| {
            panel.state.select_track(track_id, false);
            cx.notify();
        }))
        // Track color indicator at top with gradient
        .child(
            div()
                .w_full()
                .h(px(3.0))
                .bg(track_color)
                .rounded_sm()
                .shadow_sm()
        )
        // Track name with tooltip
        .child(
            div()
                .w_full()
                .h(px(28.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_center()
                        .text_color(if is_muted {
                            cx.theme().muted_foreground.opacity(0.5)
                        } else {
                            cx.theme().foreground
                        })
                        .line_clamp(2)
                        .child(track.name.clone())
                )
        )
        // Output routing dropdown
        .child(super::output_routing::render_output_routing(track, track_id, cx))
        // Insert slots (3 effect slots)
        .child(super::insert_slots::render_insert_slots(track, cx))
        // Send levels (A and B with pre/post toggle)
        .child(super::send_controls::render_send_controls(track, track_id, cx))
        // Peak meter LEDs with smooth animation
        .child(super::peak_meters::render_peak_meters(track, state, cx))
        // Vertical output fader slider
        .child(super::fader_slider::render_fader_slider(track, track_id, cx))
        // Volume readout with dB display
        .child(
            div()
                .w_full()
                .h(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_medium()
                .text_color(if is_muted {
                    cx.theme().muted_foreground.opacity(0.5)
                } else {
                    cx.theme().foreground
                })
                .child(format!("{:+.1} dB", track.volume_db()))
        )
}