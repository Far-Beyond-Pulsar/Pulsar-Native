use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, Sizable, StyledExt, ActiveTheme, PixelsExt};
use super::super::DawPanel;
use super::{TrackId, DragState};

pub fn render_send_row(
    label: &'static str,
    value: f32,
    is_pre_fader: bool,
    track_id: TrackId,
    send_idx: usize,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_1()
        .items_center()
        // Send label and pre/post toggle
        .child(
            Button::new(ElementId::Name(format!("send-{}-{}-prepost", track_id, send_idx).into()))
                .label(if is_pre_fader { "PRE" } else { "PST" })
                .compact()
                .small()
                .when(is_pre_fader, |b| b.primary())
                .when(!is_pre_fader, |b| b.ghost())
                .tooltip(format!("Send {}: Pre/Post Fader", label))
                .flex_shrink_0()
                .on_click(cx.listener(move |panel, _, _window, cx| {
                    // Toggle pre/post fader
                    if let Some(ref mut project) = panel.state.project {
                        if let Some(track) = project.tracks.iter_mut().find(|t| t.id == track_id) {
                            // Ensure send exists
                            while track.sends.len() <= send_idx {
                                track.sends.push(super::super::super::audio_types::Send {
                                    target_track: None,
                                    amount: 0.0,
                                    pre_fader: false,
                                    enabled: false,
                                });
                            }
                            if let Some(send) = track.sends.get_mut(send_idx) {
                                send.pre_fader = !send.pre_fader;
                                eprintln!("🎚️ Send {} set to {}", label, if send.pre_fader { "PRE" } else { "POST" });
                            }
                        }
                    }
                    cx.notify();
                }))
        )
        // Send level control with dragging
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
                .cursor_ew_resize()
                .hover(|style| {
                    style
                        .bg(cx.theme().accent.opacity(0.55))
                        .shadow_sm()
                })
                .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                    // Start dragging send level
                    panel.state.drag_state = DragState::DraggingSend {
                        track_id,
                        send_idx,
                        start_mouse_x: event.position.x.as_f32(),
                        start_amount: value,
                    };
                    cx.notify();
                }))
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(if value > 0.0 {
                            cx.theme().accent_foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(format!("{:.0}", value * 100.0))
                )
        )
}