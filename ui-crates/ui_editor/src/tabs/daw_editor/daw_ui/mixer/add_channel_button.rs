use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::{Track, DawUiState};
use std::sync::Arc;
use parking_lot::RwLock;

pub fn render_add_channel_button(state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<super::super::panel::DawPanel>) -> impl IntoElement {
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
        .on_mouse_down(MouseButton::Left, {
            let state_arc = state_arc.clone();
            move |_event: &MouseDownEvent, window, cx| {
                // Add a new track with sync to audio service
                if let Some(ref mut project) = state_arc.write().project {
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
                    if let Some(ref service) = state_arc.read().audio_service {
                        let service = service.clone();
                        cx.spawn(async move {
                            let _ = service.add_track(new_track).await;
                        }).detach();
                    }

                    window.refresh();
                }
            }
        })
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
