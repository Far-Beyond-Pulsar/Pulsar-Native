use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::{Track, DawUiState, TrackId, DragState};
use std::sync::Arc;
use parking_lot::RwLock;

pub fn render_insert_slots(track: &Track, state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<super::super::panel::DawPanel>) -> impl IntoElement {
    let track_id = track.id;

    v_flex()
        .w_full()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("INSERTS")
        )
        .child(
            h_flex()
                .w_full()
                .gap_0p5()
                .children((0..3).map(move |slot_idx| {
                    let has_effect = false; // Future: Check track.effects[slot_idx]

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
                        .font_medium()
                        .text_color(if has_effect {
                            cx.theme().accent_foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .bg(cx.theme().accent.opacity(0.5))
                                .shadow_sm()
                        })
                        .on_mouse_down(MouseButton::Left, move |_event: &MouseDownEvent, _window, _cx| {
                            // Future: Show effect browser/menu
                            eprintln!("📦 Insert slot {} clicked for track {}", slot_idx, track_id);
                        })
                        .child(if has_effect {
                            "FX".to_string()
                        } else {
                            format!("{}", slot_idx + 1)
                        })
                }))
        )
}
