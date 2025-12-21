use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::super::DawPanel;
use super::{Track, DawUiState, TrackId, DragState};

pub fn render_fader_slider(
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
                .font_semibold()
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
                    // Vertical fader track with precise control
                    div()
                        .id(ElementId::Name(format!("fader-track-{}", track_id).into()))
                        .relative()
                        .w(px(10.0))
                        .h_full()
                        .min_h(px(80.0))
                        .bg(cx.theme().secondary.opacity(0.5))
                        .rounded_sm()
                        .cursor_ns_resize()
                        // Click on track to jump to position
                        .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                            panel.state.drag_state = DragState::DraggingFader {
                                track_id,
                                start_mouse_y: event.position.y.as_f32(),
                                start_volume: volume,
                            };
                            cx.notify();
                        }))
                        .child(
                            // Volume fill - professional gradient
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .w_full()
                                .h(relative(volume_percent / 100.0))
                                .bg(hsla(0.55, 0.75, 0.55, 1.0)) // Vibrant teal-green
                                .rounded_sm()
                                .shadow_sm()
                        )
                        .child(
                            // Fader thumb - draggable with hover effect
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
                                .shadow_lg()
                                .cursor_pointer()
                                .hover(|style| {
                                    style.shadow_xl()
                                })
                                .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
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