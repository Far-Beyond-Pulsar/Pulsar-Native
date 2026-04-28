use super::super::*;
use super::render_color_swatch;

pub(crate) fn render_relation_row_component(
    title: &'static str,
    colors: Vec<Hsla>,
    state: Entity<ColorPickerState>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .child(
            div()
                .w(px(108.0))
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
        .child(
            h_flex().gap_1().children(
                colors
                    .into_iter()
                    .map(|color| render_color_swatch("color", color, true, state.clone(), window)),
            ),
        )
}
