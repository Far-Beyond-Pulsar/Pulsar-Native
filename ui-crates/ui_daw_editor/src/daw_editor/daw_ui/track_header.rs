/// Track Header Component
/// Left sidebar showing track controls (mute, solo, volume, etc.)

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme,
};

pub fn render_track_header(
    track: &crate::daw_editor::audio_types::Track,
    state: &DawUiState,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let track_height = *state.track_heights.get(&track.id)
        .unwrap_or(&state.viewport.track_height);
    
    let is_selected = state.selection.selected_track_ids.contains(&track.id);
    let is_muted = state.is_track_effectively_muted(track.id);
    let is_soloed = state.solo_tracks.contains(&track.id);
    let track_id = track.id;
    
    // Convert linear volume (0.0-2.0) to dB slider value (-60 to +12 dB)
    let current_db = track.volume_db();
    let slider_value = ((current_db + 60.0) / 72.0).clamp(0.0, 1.0); // Map -60..+12 to 0..1
    let pan_slider_value = ((track.pan + 1.0) / 2.0).clamp(0.0, 1.0); // Map -1..1 to 0..1
    
    v_flex()
        .w_full()
        .h(px(track_height))
        .px_2()
        .py_2()
        .gap_2()
        .bg(if is_selected {
            cx.theme().accent.opacity(0.1)
        } else {
            cx.theme().muted.opacity(0.05)
        })
        .border_b_1()
        .border_color(cx.theme().border)
        // Track name and color with controls
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                // Color indicator
                .child(
                    div()
                        .w(px(4.0))
                        .h(px(32.0))
                        .rounded_sm()
                        .bg(rgb(0x3b82f6)) // Track color
                )
                // Name and button group
                .child(
                    v_flex()
                        .flex_1()
                        .gap_1()
                        // Track name
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(track.name.clone())
                        )
                        // Control buttons
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new(ElementId::Name(format!("track-{}-mute", track_id).into()))
                                        .label("M")
                                        .compact()
                                        .small()
                                        .when(is_muted, |b| b.warning())
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            if let Some(t) = this.state.get_track_mut(track_id) {
                                                t.muted = !t.muted;
                                                let new_muted_val = t.muted;

                                                // Sync to audio service
                                                if let Some(ref service) = this.state.audio_service {
                                                    let service = service.clone();
                                                    cx.spawn(async move |_this, _cx| {
                                                        let _ = service.set_track_mute(track_id, new_muted_val).await;
                                                    }).detach();
                                                }

                                                cx.notify();
                                            }
                                        }))
                                )
                                .child(
                                    Button::new(ElementId::Name(format!("track-{}-solo", track_id).into()))
                                        .label("S")
                                        .compact()
                                        .small()
                                        .when(is_soloed, |b| b.primary())
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.state.toggle_solo(track_id);
                                            let is_solo_val = this.state.solo_tracks.contains(&track_id);

                                            // Sync to audio service
                                            if let Some(ref service) = this.state.audio_service {
                                                let service = service.clone();
                                                cx.spawn(async move |_this, _cx| {
                                                    let _ = service.set_track_solo(track_id, is_solo_val).await;
                                                }).detach();
                                            }

                                            cx.notify();
                                        }))
                                )
                                .child(
                                    Button::new(ElementId::Name(format!("track-{}-record", track_id).into()))
                                        .icon(Icon::new(IconName::Circle))
                                        .compact()
                                        .small()
                                        .ghost()
                                )
                        )
                )
        )
        // Horizontal Volume Slider (manual drag handler)
        .child(
            v_flex()
                .w_full()
                .gap_1()
                // Volume label and value
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .text_color(cx.theme().muted_foreground)
                                .child("Volume")
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(format!("{:+.1} dB", current_db))
                        )
                )
                // Horizontal slider track
                .child(
                    div()
                        .w_full()
                        .h(px(20.0))
                        .relative()
                        .child(
                            // Track background
                            div()
                                .id(ElementId::Name(format!("track-{}-vol-track", track_id).into()))
                                .w_full()
                                .h(px(6.0))
                                .mt(px(7.0))
                                .bg(cx.theme().secondary.opacity(0.5))
                                .rounded_sm()
                                .cursor_ew_resize()
                                .child(
                                    // Volume fill
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .w(relative(slider_value as f32))
                                        .h_full()
                                        .bg(hsla(0.55, 0.7, 0.55, 1.0))
                                        .rounded_sm()
                                )
                                .child(
                                    // Draggable thumb
                                    div()
                                        .id(ElementId::Name(format!("track-{}-vol-thumb", track_id).into()))
                                        .absolute()
                                        .left(relative(slider_value as f32))
                                        .top(px(-2.0))
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .ml(px(-5.0))
                                        .bg(cx.theme().accent)
                                        .rounded_sm()
                                        .border_2()
                                        .border_color(cx.theme().foreground.opacity(0.3))
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                                            panel.state.drag_state = DragState::DraggingTrackHeaderVolume {
                                                track_id,
                                                start_mouse_x: event.position.x,
                                                start_value: slider_value as f32,
                                            };
                                            cx.notify();
                                        }))
                                )
                        )
                )
        )
        // Pan Control (manual drag handler)
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .text_color(cx.theme().muted_foreground)
                                .child("Pan")
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(if track.pan.abs() < 0.01 {
                                    "C".to_string()
                                } else if track.pan < 0.0 {
                                    format!("L{:.0}", -track.pan * 100.0)
                                } else {
                                    format!("R{:.0}", track.pan * 100.0)
                                })
                        )
                )
                .child(
                    div()
                        .w_full()
                        .h(px(20.0))
                        .relative()
                        .child(
                            // Track background
                            div()
                                .id(ElementId::Name(format!("track-{}-pan-track", track_id).into()))
                                .w_full()
                                .h(px(6.0))
                                .mt(px(7.0))
                                .bg(cx.theme().secondary.opacity(0.5))
                                .rounded_sm()
                                .cursor_ew_resize()
                                .child(
                                    // Center indicator
                                    div()
                                        .absolute()
                                        .left(relative(0.5))
                                        .top_0()
                                        .w(px(2.0))
                                        .h_full()
                                        .bg(cx.theme().border)
                                )
                                .child(
                                    // Draggable thumb
                                    div()
                                        .id(ElementId::Name(format!("track-{}-pan-thumb", track_id).into()))
                                        .absolute()
                                        .left(relative(pan_slider_value as f32))
                                        .top(px(-2.0))
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .ml(px(-5.0))
                                        .bg(cx.theme().accent)
                                        .rounded_sm()
                                        .border_2()
                                        .border_color(cx.theme().foreground.opacity(0.3))
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                                            panel.state.drag_state = DragState::DraggingTrackHeaderPan {
                                                track_id,
                                                start_mouse_x: event.position.x,
                                                start_value: pan_slider_value as f32,
                                            };
                                            cx.notify();
                                        }))
                                )
                        )
                )
        )
}
