use gpui::{App, IntoElement, Styled as _, prelude::FluentBuilder};

use crate::{h_flex, ActiveTheme as _, Icon, IconName, StyledExt as _};

/// Standard empty-state placeholder used by list, table, and other collection views.
pub fn empty_state_placeholder(cx: &App) -> impl IntoElement {
    h_flex()
        .size_full()
        .justify_center()
        .items_center()
        .text_color(cx.theme().muted_foreground.opacity(0.6))
        .child(Icon::new(IconName::Inbox).size_12())
}
