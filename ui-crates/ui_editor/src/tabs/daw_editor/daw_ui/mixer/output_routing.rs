use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme, PixelsExt,
    h_virtual_list, scroll::{Scrollbar, ScrollbarAxis},
};
use super::super::DawPanel;
use super::{Track, DawUiState, TrackId, DragState};

/// Output routing dropdown - selects which bus/output this track routes to
pub fn render_output_routing(
    track: &Track,
    track_id: TrackId,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let output_name = "Master";

    v_flex()
        .w_full()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .font_semibold()
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
                .hover(|style| {
                    style
                        .bg(cx.theme().accent.opacity(0.45))
                        .shadow_sm()
                })
                .on_mouse_down(MouseButton::Left, cx.listener(move |_panel, _event: &MouseDownEvent, _window, cx| {
                    // Future: Show routing dropdown menu
                    eprintln!("🔌 Output routing clicked for track {}", track_id);
                    cx.notify();
                }))
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(cx.theme().accent_foreground)
                        .child(output_name)
                )
        )
}