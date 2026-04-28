use super::super::*;

pub(crate) fn render_color_swatch(
    id_prefix: &'static str,
    color: Hsla,
    clickable: bool,
    state: Entity<ColorPickerState>,
    window: &mut Window,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("{id_prefix}-{}", color.to_hex())))
        .h_5()
        .w_5()
        .bg(color)
        .border_1()
        .border_color(color.darken(0.1))
        .when(clickable, |this| {
            this.hover(|this| {
                this.border_color(color.darken(0.3))
                    .bg(color.lighten(0.1))
                    .shadow_xs()
            })
            .active(|this| this.border_color(color.darken(0.5)).bg(color.darken(0.2)))
            .on_click(window.listener_for(
                &state,
                move |state, _, window, cx| {
                    state.apply_external_color(color, true, window, cx);
                },
            ))
        })
}
