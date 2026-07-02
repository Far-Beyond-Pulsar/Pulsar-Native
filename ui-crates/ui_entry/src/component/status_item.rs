use gpui::prelude::*;
use gpui::*;
use ui::{h_flex, ActiveTheme as _, Icon, IconName};

use crate::screen::EntryScreen;

pub fn render_status_item(
    icon: IconName,
    label: &str,
    status: &str,
    status_color: gpui::Hsla,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();

    h_flex()
        .gap_2()
        .items_center()
        .child(Icon::new(icon).size_4().text_color(theme.muted_foreground))
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(status_color)
                .child(status.to_string()),
        )
        .into_any_element()
}
