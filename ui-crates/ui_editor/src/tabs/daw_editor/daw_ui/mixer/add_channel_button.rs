use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::super::DawPanel;
use super::Track;

pub fn render_add_channel_button(cx: &mut Context<DawPanel>) -> impl IntoElement {
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
            // Add a new track with sync to audio service
            if let Some(ref mut project) = panel.state.project {
                let new_track_id = uuid::Uuid::new_v4();
                let new_track = Track {
                    id: new_track_id,
                    name: format!("Track {}", project.tracks.len() + 1),
                    track_type: super::super::super::audio_types::TrackType::Audio,
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
                project.tracks.push(new_track.clone());

                // Sync to audio service
                if let Some(ref service) = panel.state.audio_service {
                    let service = service.clone();
                    cx.spawn(async move |_this, _cx| {
                        let _ = service.add_track(new_track).await;
                    }).detach();
                }

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