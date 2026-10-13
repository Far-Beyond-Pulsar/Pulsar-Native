//! Small pieces for laying out settings forms.

use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, StyledExt};

/// A titled group of settings.
pub fn section(title: impl Into<SharedString>, icon: IconName, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .gap_2()
        .child(
            h_flex()
                .w_full()
                .gap_1p5()
                .items_center()
                .pb_1()
                .border_b_1()
                .border_color(theme.border)
                .child(Icon::new(icon).size(px(13.0)).text_color(theme.info))
                .child(div().text_sm().font_semibold().text_color(theme.foreground).child(title.into())),
        )
}

/// A setting: its name and what it does on the left, its control on the right.
pub fn setting(label: impl Into<SharedString>, hint: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    let theme = cx.theme();
    let hint: SharedString = hint.into();
    h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().text_color(theme.foreground).child(label.into()))
                .children((!hint.is_empty()).then(|| div().text_xs().text_color(theme.muted_foreground).child(hint))),
        )
        .child(div().w(px(190.0)).flex_shrink_0().child(control))
}

/// A line of small muted text.
pub fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div().w_full().text_xs().text_color(cx.theme().muted_foreground).child(text.into())
}

/// A small rounded label.
pub fn chip(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    div().px_1p5().py_0p5().rounded(px(4.0)).bg(theme.secondary).text_xs().text_color(theme.muted_foreground).child(text.into())
}
