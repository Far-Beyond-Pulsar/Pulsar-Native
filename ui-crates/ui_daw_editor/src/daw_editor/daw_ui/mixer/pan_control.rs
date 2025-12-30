use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    h_flex, v_flex, Sizable, StyledExt, ActiveTheme, PixelsExt,
};
use super::super::DawPanel;
use super::{Track, TrackId, DragState};

pub fn render_pan_control(
    track: &Track,
    track_id: TrackId,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let pan = track.pan; // -1.0 (left) to +1.0 (right)
    let pan_percent = ((pan + 1.0) / 2.0) * 100.0; // 0-100%

    // Pan label
    let pan_label = if pan < -0.01 {
        format!("L{:.0}", pan.abs() * 100.0)
    } else if pan > 0.01 {
        format!("R{:.0}", pan * 100.0)
    } else {
        "C".to_string()
    };

    v_flex()
        .w_full()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .text_center()
                .child("PAN")
        )
        .child(
            // Pan slider container
            div()
                .id(ElementId::Name(format!("pan-track-{}", track_id).into()))
                .w_full()
                .h(px(24.0))
                .px_1()
                .relative()
                .bg(cx.theme().secondary.opacity(0.3))
                .rounded_sm()
                .border_1()
                .border_color(cx.theme().border.opacity(0.6))
                .cursor_ew_resize()
                .hover(|style| style.bg(cx.theme().secondary.opacity(0.45)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                    // Calculate initial slider value (0.0 to 1.0)
                    let slider_value = (pan + 1.0) / 2.0;

                    panel.state.drag_state = DragState::DraggingTrackHeaderPan {
                        track_id,
                        start_mouse_x: event.position.x,
                        start_value: slider_value,
                    };
                    cx.notify();
                }))
                .child(
                    // Center marker
                    div()
                        .absolute()
                        .left(relative(0.5))
                        .top_0()
                        .w(px(2.0))
                        .h_full()
                        .bg(cx.theme().muted_foreground.opacity(0.3))
                )
                .child(
                    // Pan position indicator
                    div()
                        .absolute()
                        .left(relative(pan_percent / 100.0))
                        .top(px(2.0))
                        .w(px(16.0))
                        .h(px(18.0))
                        .ml(px(-8.0)) // Center the knob
                        .bg(cx.theme().accent)
                        .rounded_sm()
                        .border_2()
                        .border_color(cx.theme().foreground.opacity(0.3))
                        .shadow_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(2.0))
                                .h(px(8.0))
                                .bg(cx.theme().accent_foreground)
                                .rounded_sm()
                        )
                )
        )
        .child(
            // Pan value display
            div()
                .w_full()
                .h(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().foreground)
                .child(pan_label)
        )
}
