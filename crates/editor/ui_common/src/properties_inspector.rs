use gpui::{
    prelude::FluentBuilder as _, px, Context, FontWeight, IntoElement, ParentElement, Styled,
};

use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable,
};

pub fn render_header<V>(
    title: impl Into<String>,
    has_selection: bool,
    selected_badge_label: impl Into<String>,
    menu_button_id: impl Into<String>,
    cx: &Context<V>,
) -> impl IntoElement {
    let title = title.into();
    let selected_badge_label = selected_badge_label.into();
    let menu_button_id = menu_button_id.into();

    h_flex()
        .w_full()
        .px_4()
        .py_3()
        .justify_between()
        .items_center()
        .bg(cx.theme().sidebar)
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    gpui::div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child(title),
                )
                .when(has_selection, |this| {
                    this.child(
                        gpui::div()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(cx.theme().accent.opacity(0.15))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child(selected_badge_label),
                    )
                }),
        )
        .child(
            h_flex().gap_1().child(
                Button::new(menu_button_id)
                    .icon(IconName::Ellipsis)
                    .xsmall(),
            ),
        )
}

pub fn render_empty_state<V>(
    icon: IconName,
    title: impl Into<String>,
    description: impl Into<String>,
    cx: &Context<V>,
) -> impl IntoElement {
    let title = title.into();
    let description = description.into();

    gpui::div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_8()
        .child(
            v_flex()
                .gap_3()
                .items_center()
                .child(
                    Icon::new(icon)
                        .size(px(48.0))
                        .text_color(cx.theme().muted_foreground.opacity(0.5)),
                )
                .child(
                    gpui::div()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().muted_foreground)
                        .child(title),
                )
                .child(
                    gpui::div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                        .text_center()
                        .child(description),
                ),
        )
}
