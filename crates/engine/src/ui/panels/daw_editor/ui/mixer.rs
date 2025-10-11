/// Mixer View Component
/// Professional channel strips with faders, pan, sends, meters, and insert effects

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, VirtualListScrollHandle, scroll::{Scrollbar, ScrollbarAxis},
};
use crate::ui::panels::daw_editor::audio_types::{Track, TrackId};
use std::rc::Rc;

const CHANNEL_STRIP_WIDTH: f32 = 90.0;
const MIXER_PADDING: f32 = 8.0;

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
                                    return Some(render_channel_strip(track, idx, &panel.state, cx).into_any_element());
                                }
                            }
                            None
                        } else if idx == num_tracks {
                            // Render add channel button
                            Some(render_add_channel_button(cx).into_any_element())
                        } else if idx == num_tracks + 1 {
                            // Render master channel
                            Some(render_master_channel(&panel.state, cx).into_any_element())
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

fn render_add_channel_button(cx: &mut Context<DawPanel>) -> impl IntoElement {
    v_flex()
        .w(px(90.0))
        .h_full()
        .gap_1()
        .p_2()
        .bg(cx.theme().accent.opacity(0.1))
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().accent.opacity(0.3))
        .cursor_pointer()
        .hover(|style| style.bg(cx.theme().accent.opacity(0.2)))
        .on_mouse_down(MouseButton::Left, cx.listener(|panel, _event: &MouseDownEvent, _window, cx| {
            // Add a new track
            if let Some(ref mut project) = panel.state.project {
                let new_track_id = uuid::Uuid::new_v4();
                let new_track = Track {
                    id: new_track_id,
                    name: format!("Track {}", project.tracks.len() + 1),
                    track_type: super::super::audio_types::TrackType::Audio,
                    volume: 1.0,
                    pan: 0.0,
                    muted: false,
                    solo: false,
                    record_armed: false,
                    clips: Vec::new(),
                    sends: Vec::new(),
                    automation: Vec::new(),
                    color: [0.5, 0.5, 0.8],
                };
                project.tracks.push(new_track);
                cx.notify();
            }
        }))
        .child(
            div()
                .flex_1()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .w(px(48.0))
                        .h(px(48.0))
                        .rounded_full()
                        .bg(cx.theme().accent.opacity(0.3))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Plus)
                                .size_6()
                                .text_color(cx.theme().accent)
                        )
                )
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().accent)
                        .child("Add Track")
                )
        )
}

fn render_channel_strip(
    track: &Track,
    idx: usize,
    state: &DawUiState,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let is_selected = state.selection.selected_track_ids.contains(&track.id);
    let is_muted = track.muted || state.is_track_effectively_muted(track.id);
    let track_id = track.id;

    // Beautiful color per track
    let track_hue = (idx as f32 * 137.5) % 360.0; // Golden angle distribution
    let track_color = hsla(track_hue / 360.0, 0.6, 0.5, 1.0);

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
        .on_mouse_down(MouseButton::Left, cx.listener(move |panel, _event: &MouseDownEvent, _window, cx| {
            panel.state.select_track(track_id, false);
            cx.notify();
        }))
        // Track color indicator at top
        .child(
            div()
                .w_full()
                .h(px(3.0))
                .bg(track_color)
                .rounded_sm()
        )
        // Track name
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
                        .text_color(cx.theme().foreground)
                        .line_clamp(2)
                        .child(track.name.clone())
                )
        )
        // Output routing dropdown
        .child(render_output_routing(track, track_id, cx))
        // Insert slots (3 effect slots)
        .child(render_insert_slots(track, cx))
        // Send levels (A and B with pre/post toggle)
        .child(render_send_controls(track, track_id, cx))
        // Peak meter LEDs
        .child(render_peak_meters(track, state, cx))
        // Vertical output fader slider (routes to selected output)
        .child(render_fader_slider(track, track_id, cx))
        // Volume readout
        .child(
            div()
                .w_full()
                .h(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{:+.1} dB", track.volume_db()))
        )
}

fn render_insert_slots(track: &Track, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let track_id = track.id;

    v_flex()
        .w_full()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("INSERTS")
        )
        .child(
            h_flex()
                .w_full()
                .gap_0p5()
                .children((0..3).map(move |slot_idx| {
                    // Check if this slot has an effect loaded
                    let has_effect = false; // TODO: Check track.effects[slot_idx]

                    div()
                        .id(ElementId::Name(format!("insert-{}-{}", track_id, slot_idx).into()))
                        .w(px(24.0))
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if has_effect {
                            cx.theme().accent.opacity(0.6)
                        } else {
                            cx.theme().secondary.opacity(0.4)
                        })
                        .rounded_sm()
                        .border_1()
                        .border_color(if has_effect {
                            cx.theme().accent
                        } else {
                            cx.theme().border.opacity(0.5)
                        })
                        .text_xs()
                        .text_color(if has_effect {
                            cx.theme().accent_foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .cursor_pointer()
                        .hover(|style| style.bg(cx.theme().accent.opacity(0.3)))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_panel, _event: &MouseDownEvent, _window, cx| {
                            // TODO: Show effect browser/menu
                            eprintln!("📦 Insert slot {} clicked for track {}", slot_idx, track_id);
                            cx.notify();
                        }))
                        .tooltip(if has_effect {
                            "Click to change effect"
                        } else {
                            "Click to add effect"
                        })
                        .child(if has_effect {
                            "FX".to_string()
                        } else {
                            format!("{}", slot_idx + 1)
                        })
                }))
        )
}

fn render_peak_meters(track: &Track, state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
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
        .child(render_meter_bar(left_peak, cx))
        .child(render_meter_bar(right_peak, cx))
}

fn render_meter_bar(level: f32, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let level_clamped = level.clamp(0.0, 1.0);
    let segments = 12;

    v_flex()
        .flex_1()
        .gap_0p5()
        .flex_col_reverse() // Bottom to top
        .children((0..segments).map(move |seg| {
            let threshold = seg as f32 / segments as f32;
            let is_lit = level_clamped >= threshold;

            // Color gradient: green -> yellow -> orange -> red
            let color = if seg > 10 {
                hsla(0.0, 0.9, 0.5, 1.0) // Red
            } else if seg > 8 {
                hsla(30.0 / 360.0, 0.9, 0.5, 1.0) // Orange
            } else if seg > 6 {
                hsla(60.0 / 360.0, 0.9, 0.5, 1.0) // Yellow
            } else {
                hsla(120.0 / 360.0, 0.7, 0.5, 1.0) // Green
            };

            div()
                .w_full()
                .h(px(3.0))
                .rounded_sm()
                .bg(if is_lit {
                    color
                } else {
                    cx.theme().secondary.opacity(0.3)  // Better contrast for unlit segments
                })
        }))
}

fn render_fader_slider(
    track: &Track,
    track_id: TrackId,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let volume = track.volume;
    let volume_percent = ((volume / 1.5) * 100.0).clamp(0.0, 100.0);
    
    v_flex()
        .w_full()
        .flex_1()
        .min_h(px(100.0))
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .text_center()
                .child("VOLUME")
        )
        .child(
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    // Vertical fader track with proper mouse handling
                    div()
                        .id(ElementId::Name(format!("fader-track-{}", track_id).into()))
                        .relative()
                        .w(px(10.0))
                        .h_full()
                        .min_h(px(80.0))
                        .bg(cx.theme().secondary.opacity(0.5))  // Better contrast
                        .rounded_sm()
                        .cursor_ns_resize()
                        .child(
                            // Volume fill - brighter color for better contrast
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .w_full()
                                .h(relative(volume_percent / 100.0))
                                .bg(hsla(0.55, 0.7, 0.55, 1.0)) // Bright teal-green for visibility
                                .rounded_sm()
                        )
                        .child(
                            // Fader thumb - draggable
                            div()
                                .id(ElementId::Name(format!("fader-thumb-{}", track_id).into()))
                                .absolute()
                                .w(px(24.0))
                                .h(px(14.0))
                                .left(px(-7.0))
                                .bottom(relative(volume_percent / 100.0))
                                .bg(cx.theme().accent)
                                .rounded_sm()
                                .border_2()
                                .border_color(cx.theme().foreground.opacity(0.3))
                                .shadow_md()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                                    // Start dragging fader
                                    panel.state.drag_state = DragState::DraggingFader {
                                        track_id,
                                        start_mouse_y: event.position.y.as_f32(),
                                        start_volume: volume,
                                    };
                                    cx.notify();
                                }))
                        )
                )
        )
}

fn render_pan_slider(
    track: &Track,
    track_id: TrackId,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let pan = track.pan;
    let pan_percent = (pan * 100.0) as i32;
    let pan_position = ((pan + 1.0) / 2.0 * 100.0).clamp(0.0, 100.0);
    
    v_flex()
        .w_full()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("PAN")
        )
        .child(
            div()
                .w_full()
                .h(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    // Horizontal pan slider
                    div()
                        .id(ElementId::Name(format!("pan-track-{}", track_id).into()))
                        .relative()
                        .w_full()
                        .h(px(8.0))
                        .bg(cx.theme().secondary.opacity(0.5))  // Better contrast
                        .rounded_sm()
                        .cursor_ew_resize()
                        .child(
                            // Center indicator
                            div()
                                .absolute()
                                .w(px(2.0))
                                .h_full()
                                .left(relative(0.5))
                                .bg(cx.theme().border)
                        )
                        .child(
                            // Pan thumb - draggable
                            div()
                                .id(ElementId::Name(format!("pan-thumb-{}", track_id).into()))
                                .absolute()
                                .w(px(14.0))
                                .h(px(20.0))
                                .left(relative(pan_position / 100.0))
                                .top(px(-6.0))
                                .bg(cx.theme().accent)
                                .rounded_sm()
                                .border_2()
                                .border_color(cx.theme().foreground.opacity(0.3))
                                .shadow_md()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                                    // Start dragging pan
                                    panel.state.drag_state = DragState::DraggingPan {
                                        track_id,
                                        start_mouse_x: event.position.x.as_f32(),
                                        start_pan: pan,
                                    };
                                    cx.notify();
                                }))
                        )
                )
        )
        .child(
            div()
                .w_full()
                .text_xs()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child(if pan_percent == 0 {
                    "C".to_string()
                } else if pan_percent < 0 {
                    format!("{}L", -pan_percent)
                } else {
                    format!("{}R", pan_percent)
                })
        )
}

/// Output routing dropdown - selects which bus/output this track routes to
fn render_output_routing(
    track: &Track,
    track_id: TrackId,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    // For now, all tracks route to Master (could expand to Aux/Group buses later)
    let output_name = "Master";

    v_flex()
        .w_full()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("OUTPUT")
        )
        .child(
            div()
                .id(ElementId::Name(format!("output-routing-{}", track_id).into()))
                .w_full()
                .h(px(24.0))
                .px_2()
                .flex()
                .items_center()
                .justify_center()
                .bg(cx.theme().accent.opacity(0.3))
                .rounded_sm()
                .border_1()
                .border_color(cx.theme().accent.opacity(0.6))
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().accent.opacity(0.4)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |_panel, _event: &MouseDownEvent, _window, cx| {
                    // TODO: Show routing dropdown menu to select output (Master, Aux 1, Aux 2, etc.)
                    eprintln!("🔌 Output routing clicked for track {}", track_id);
                    cx.notify();
                }))
                .tooltip("Select output destination")
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(cx.theme().accent_foreground)
                        .child(output_name)
                )
        )
}

fn render_send_controls(
    track: &Track,
    track_id: TrackId,
    cx: &mut Context<DawPanel>,
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
                .text_color(cx.theme().muted_foreground)
                .child("SENDS")
        )
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .child(render_send_row("Send A", send_a_amount, send_a_pre, track_id, 0, cx))
                .child(render_send_row("Send B", send_b_amount, send_b_pre, track_id, 1, cx))
        )
}

fn render_send_row(
    label: &'static str,
    value: f32,
    is_pre_fader: bool,
    track_id: TrackId,
    send_idx: usize,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let tooltip_text = format!("Send {}: {}%", label, (value * 100.0) as i32);

    h_flex()
        .w_full()
        .gap_1()
        .items_center()
        // Send label and pre/post toggle
        .child(
            Button::new(ElementId::Name(format!("send-{}-{}-prepost", track_id, send_idx).into()))
                .label(if is_pre_fader { "PRE" } else { "POST" })
                .compact()
                .small()
                .when(is_pre_fader, |b| b.primary())
                .when(!is_pre_fader, |b| b.ghost())
                .tooltip("Pre/Post Fader")
                .flex_shrink_0()
                .on_click(cx.listener(move |panel, _, _window, cx| {
                    // Toggle pre/post fader
                    if let Some(ref mut project) = panel.state.project {
                        if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                            if let Some(send) = track.sends.get_mut(send_idx) {
                                send.pre_fader = !send.pre_fader;
                                eprintln!("🎚️ Send {} set to {}", send_idx, if send.pre_fader { "PRE" } else { "POST" });
                            }
                        }
                    }
                    cx.notify();
                }))
        )
        // Send level knob (mini)
        .child(
            div()
                .id(ElementId::Name(format!("send-{}-{}-level", track_id, send_idx).into()))
                .flex_1()
                .h(px(20.0))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .bg(if value > 0.0 {
                    cx.theme().accent.opacity(0.4)
                } else {
                    cx.theme().secondary.opacity(0.3)
                })
                .rounded_sm()
                .border_1()
                .border_color(if value > 0.0 {
                    cx.theme().accent.opacity(0.6)
                } else {
                    cx.theme().border.opacity(0.5)
                })
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().accent.opacity(0.5)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |_panel, _event: &MouseDownEvent, _window, cx| {
                    // TODO: Implement send level dragging or show send routing menu
                    eprintln!("📤 Send {} level control clicked", send_idx);
                    cx.notify();
                }))
                .tooltip(tooltip_text.clone())
                .child(
                    div()
                        .text_xs()
                        .text_color(if value > 0.0 {
                            cx.theme().accent_foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(format!("{:.0}%", value * 100.0))
                )
        )
}

fn render_master_channel(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
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
        .shadow_lg()
        // Master label
        .child(
            div()
                .w_full()
                .h(px(3.0))
                .bg(cx.theme().accent)
                .rounded_sm()
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
        // Spacer for insert slots (master has no inserts in this simple version)
        .child(div().h(px(44.0)))
        // Master peak meters
        .child(render_master_meters(state, cx))
        // Master fader
        .child(render_master_fader(master_volume, cx))
        // Master volume readout
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
                    hsla(0.0, 0.9, 0.5, 1.0)
                } else {
                    cx.theme().accent
                })
                .child(format!("{:+.1} dB", volume_db))
        )
}

fn render_master_meters(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    // Get actual master meter data from audio service
    let (left_peak, right_peak) = (state.master_meter.peak_left, state.master_meter.peak_right);

    h_flex()
        .w_full()
        .h(px(48.0))
        .gap_1()
        .child(render_meter_bar(left_peak, cx))
        .child(render_meter_bar(right_peak, cx))
}

fn render_master_fader(master_volume: f32, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let volume_percent = ((master_volume / 1.5) * 100.0).clamp(0.0, 100.0);
    
    v_flex()
        .w_full()
        .flex_1()
        .min_h(px(100.0))
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().accent)
                .font_semibold()
                .text_center()
                .child("OUTPUT")
        )
        .child(
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id(ElementId::Name("master-fader-track".into()))
                        .relative()
                        .w(px(12.0))
                        .h_full()
                        .min_h(px(80.0))
                        .bg(cx.theme().secondary.opacity(0.4))
                        .rounded_sm()
                        .cursor_ns_resize()
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .w_full()
                                .h(relative(volume_percent / 100.0))
                                .bg(cx.theme().accent.opacity(0.8))
                                .rounded_sm()
                        )
                        .child(
                            div()
                                .id(ElementId::Name("master-fader-thumb".into()))
                                .absolute()
                                .w(px(28.0))
                                .h(px(16.0))
                                .left(px(-8.0))
                                .bottom(relative(volume_percent / 100.0))
                                .bg(cx.theme().accent)
                                .rounded_md()
                                .border_2()
                                .border_color(cx.theme().foreground.opacity(0.3))
                                .shadow_lg()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                                    // Start dragging master fader (use nil UUID for master)
                                    panel.state.drag_state = DragState::DraggingFader {
                                        track_id: uuid::Uuid::nil(),
                                        start_mouse_y: event.position.y.as_f32(),
                                        start_volume: master_volume,
                                    };
                                    cx.notify();
                                }))
                        )
                )
        )
}
