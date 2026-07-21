use gpui::prelude::*;
use gpui::*;
use ui::{
    button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Icon, IconName,
    StyledExt,
};

use crate::core::types::Template;
use crate::screen::EntryScreen;

pub fn render_templates(
    screen: &mut EntryScreen,
    available_width: f32,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let columns = screen.calculate_columns(px(available_width + 220.0 + 64.0));

    v_flex()
        .flex_1()
        .h_full()
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .px_8()
                .pt_6()
                .pb_4()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("Templates"),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Start your project from a template"),
                ),
        )
        .when_some(screen.state.clone_progress.as_ref(), |this, _| {
            this.child(
                h_flex()
                    .w_full()
                    .px_8()
                    .pb_4()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .h(px(4.))
                            .bg(theme.secondary.opacity(0.3))
                            .rounded_full()
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(theme.accent)
                                    .w(relative(0.5)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Cloning..."),
                    ),
            )
        })
        .child(
            v_flex()
                .id("templates-scroll")
                .flex_1()
                .min_h_0()
                .scrollable(gpui::Axis::Vertical)
                .px_8()
                .pb_6()
                .child(
                    h_flex().flex_wrap().gap_6().children(
                        screen
                            .state
                            .templates
                            .clone()
                            .iter()
                            .map(|template| render_template_card(screen, template, columns, cx)),
                    ),
                ),
        )
}

fn render_template_card(
    screen: &mut EntryScreen,
    template: &Template,
    _columns: usize,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let name = template.name.clone();
    let name_label = name.clone();
    let name_button = name.clone();
    let description = template.description.clone();
    let desc_label = description.clone();
    let category = template.category.clone();
    let category_label = category.clone();
    let repo_url = template.repo_url.clone();
    let repo_url_button = repo_url.clone();
    let template_icon = template.icon.clone();
    let template_icon_listener = template_icon.clone();
    let thumbnail = screen
        .state
        .template_thumbnails
        .get(&name)
        .and_then(|t| t.clone());

    v_flex()
        .id(SharedString::from(format!("template-card-{}", name)))
        .w(px(320.))
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.08))
        .overflow_hidden()
        .cursor_pointer()
        .hover(|this| {
            this.bg(theme.secondary.opacity(0.15))
                .border_color(theme.accent.opacity(0.4))
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            if !window.default_prevented() {
                let t = Template::new(
                    &name,
                    &description,
                    template_icon_listener.clone(),
                    &repo_url,
                    &category,
                );
                this.clone_template(t, cx);
            }
        }))
        .child(
            div()
                .w_full()
                .h(px(140.))
                .relative()
                .overflow_hidden()
                .rounded_t_xl()
                .bg(theme.secondary.opacity(0.2))
                .group("card-image")
                .when_some(thumbnail.clone(), |this, render_img| {
                    this.child(
                        img(ImageSource::Render(render_img))
                            .w_full()
                            .h_full()
                            .rounded_t_xl()
                            .object_fit(gpui::ObjectFit::Cover),
                    )
                })
                .when(thumbnail.is_none(), |this| {
                    this.child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(template_icon.clone())
                                    .size(px(36.))
                                    .text_color(theme.muted_foreground.opacity(0.3)),
                            ),
                    )
                })
                .child(
                    h_flex()
                        .absolute()
                        .top_2()
                        .right_2()
                        .gap_1()
                        .opacity(0.0)
                        .group_hover("card-image", |this| this.opacity(1.0))
                        .capture_any_mouse_up(|_, window, _| {
                            window.prevent_default();
                        })
                        .child(
                            Button::new(SharedString::from(format!(
                                "view-on-github-{}",
                                name_button
                            )))
                            .icon(IconName::Eye)
                            .compact()
                            .ghost()
                            .tooltip("View on GitHub")
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.open_url(&repo_url_button);
                                },
                            )),
                        ),
                ),
        )
        .child(
            v_flex()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(name_label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .line_height(relative(1.4))
                        .child(desc_label),
                )
                .child(
                    h_flex().gap_2().items_center().child(
                        div()
                            .px_2()
                            .py(px(2.))
                            .rounded_full()
                            .bg(theme.accent.opacity(0.15))
                            .text_xs()
                            .text_color(theme.foreground)
                            .child(category_label),
                    ),
                ),
        )
}
