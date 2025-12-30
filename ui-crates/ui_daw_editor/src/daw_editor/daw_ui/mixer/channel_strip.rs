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
        // Control buttons: Mute, Solo, Record Arm
        .child(
            h_flex()
                .w_full()
                .gap_0p5()
                .child(
                    // Mute button
                    Button::new(ElementId::Name(format!("mute-{}", track_id).into()))
                        .icon(Icon::new(IconName::Square).size_3())
                        .compact()
                        .small()
                        .flex_1()
                        .when(track.muted, |b| b.primary())
                        .when(!track.muted, |b| b.ghost())
                        .tooltip("Mute (M)")
                        .on_click(cx.listener(move |panel, _, _window, cx| {
                            if let Some(ref mut project) = panel.state.project {
                                if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                                    track.muted = !track.muted;

                                    // Sync to audio service
                                    if let Some(ref service) = panel.state.audio_service {
                                        let service = service.clone();
                                        let muted = track.muted;
                                        cx.spawn(async move |_this, _cx| {
                                            let _ = service.set_track_mute(track_id, muted).await;
                                        }).detach();
                                    }
                                }
                            }
                            cx.notify();
                        }))
                )
                .child(
                    // Solo button
                    Button::new(ElementId::Name(format!("solo-{}", track_id).into()))
                        .icon(Icon::new(IconName::Heart).size_3())
                        .compact()
                        .small()
                        .flex_1()
                        .when(track.solo, |b| b.primary())
                        .when(!track.solo, |b| b.ghost())
                        .tooltip("Solo (S)")
                        .on_click(cx.listener(move |panel, _, _window, cx| {
                            if let Some(ref mut project) = panel.state.project {
                                if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                                    track.solo = !track.solo;

                                    // Sync to audio service
                                    if let Some(ref service) = panel.state.audio_service {
                                        let service = service.clone();
                                        let solo = track.solo;
                                        cx.spawn(async move |_this, _cx| {
                                            let _ = service.set_track_solo(track_id, solo).await;
                                        }).detach();
                                    }
                                }
                            }
                            cx.notify();
                        }))
                )
                .child(
                    // Record Arm button
                    Button::new(ElementId::Name(format!("record-arm-{}", track_id).into()))
                        .icon(Icon::new(IconName::Circle).size_3())
                        .compact()
                        .small()
                        .flex_1()
                        .when(track.record_armed, |b| b.primary())
                        .when(!track.record_armed, |b| b.ghost())
                        .tooltip("Record Arm (R)")
                        .on_click(cx.listener(move |panel, _, _window, cx| {
                            if let Some(ref mut project) = panel.state.project {
                                if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                                    track.record_armed = !track.record_armed;
                                }
                            }
                            cx.notify();
                        }))
                )
        )
        // Output routing dropdown
        .child(super::output_routing::render_output_routing(track, track_id, cx))
        // Pan control with visual feedback
        .child(super::pan_control::render_pan_control(track, track_id, cx))
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