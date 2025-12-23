use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::{Track, DawUiState, TrackId, DragState};
use std::sync::Arc;
use parking_lot::RwLock;

pub fn render_master_fader(master_volume: f32, state_arc: Arc<RwLock<DawUiState>>, cx: &mut Context<super::super::panel::DawPanel>) -> impl IntoElement {
    let volume_percent = ((master_volume / 1.5) * 100.0).clamp(0.0, 100.0);

    v_flex()
        .w_full()
        .flex_1()
        .min_h(px(100.0))
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_bold()
                .text_color(cx.theme().accent)
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
                        .bg(cx.theme().secondary.opacity(0.5))
                        .rounded_sm()
                        .cursor_ns_resize()
                        .on_mouse_down(MouseButton::Left, {
                            let state_arc = state_arc.clone();
                            move |event: &MouseDownEvent, window, _cx| {
                                state_arc.write().drag_state = DragState::DraggingFader {
                                    track_id: uuid::Uuid::nil(),
                                    start_mouse_y: event.position.y.as_f32(),
                                    start_volume: master_volume,
                                };
                                window.refresh();
                            }
                        })
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .w_full()
                                .h(relative(volume_percent / 100.0))
                                .bg(cx.theme().accent.opacity(0.9))
                                .rounded_sm()
                                .shadow_md()
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
                                .shadow_xl()
                                .cursor_pointer()
                                .hover(|style| {
                                    style.shadow_2xl()
                                })
                                .on_mouse_down(MouseButton::Left, {
                                    let state_arc = state_arc.clone();
                                    move |event: &MouseDownEvent, window, _cx| {
                                        state_arc.write().drag_state = DragState::DraggingFader {
                                            track_id: uuid::Uuid::nil(),
                                            start_mouse_y: event.position.y.as_f32(),
                                            start_volume: master_volume,
                                        };
                                        window.refresh();
                                    }
                                })
                        )
                )
        )
}

