use crate::entry_screen::{virtual_grid::render_card_grid, EntryScreen};
use gpui::{prelude::*, *};
use ui::Sizable;
use ui::{h_flex, progress::Progress, tag::Tag, v_flex, ActiveTheme as _, Icon, IconName};

pub fn render_templates(
    screen: &mut EntryScreen,
    available_width: f32,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let _border_col = theme.border;
    let muted_fg = theme.muted_foreground;
    let fg = theme.foreground;
    let sidebar_bg = theme.sidebar;
    let primary = theme.primary;
    let _accent = theme.accent;
    let _accent_fg = theme.accent_foreground;

    let has_progress = screen.clone_progress.is_some();

    v_flex()
        .size_full()
        .gap_8()
        .p_8()
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_3xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(fg)
                        .child("Project Templates"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_fg)
                        .child("Choose a template to start your project quickly"),
                ),
        )
        .children(if has_progress {
            Some(
                v_flex()
                    .gap_4()
                    .p_6()
                    .border_1()
                    .border_color(primary)
                    .rounded_lg()
                    .bg(sidebar_bg)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child("Cloning Repository..."),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted_fg)
                            .child("Please wait while we clone the template..."),
                    )
                    .child(Progress::new().value(50.0)),
            )
        } else {
            None
        })
        .child(render_template_grid(screen, available_width, cx))
}

fn render_template_grid(
    screen: &mut EntryScreen,
    available_width: f32,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let border_col = theme.border;
    let sidebar_bg = theme.sidebar;
    let primary = theme.primary;
    let primary_h = theme.primary.h;
    let primary_s = theme.primary.s;
    let primary_l = theme.primary.l;
    let fg = theme.foreground;
    let muted_fg = theme.muted_foreground;
    let sidebar_hover_bg = hsla(
        theme.sidebar.h,
        theme.sidebar.s,
        theme.sidebar.l * 1.05,
        theme.sidebar.a,
    );

    // Match the layout used by `EntryScreen::calculate_columns`
    const CARD_WIDTH: f32 = 320.0;
    const CARD_HEIGHT: f32 = 220.0;
    const GAP: f32 = 24.0;
    const PADDING: f32 = 24.0;

    render_card_grid(
        screen.entity.clone().unwrap(),
        "templates-grid",
        available_width,
        screen.templates.len(),
        CARD_WIDTH,
        CARD_HEIGHT,
        GAP,
        PADDING,
        &screen.templates_scroll_handle,
        move |view, idx, card_w, _window, cx| {
            let template = view.templates[idx].clone();
            let name = template.name.clone();
            let desc = template.description.clone();
            let category = template.category.clone();
            let icon = template.icon.clone();

            let category_accent = match category.as_str() {
                "2D" => cx.theme().success,
                "3D" => cx.theme().primary,
                "Strategy" => cx.theme().warning,
                "RPG" => cx.theme().info,
                "Racing" => cx.theme().danger,
                _ => cx.theme().muted,
            };

            v_flex()
                .id(SharedString::from(format!("template-{}", name)))
                .w(px(card_w))
                .h(px(CARD_HEIGHT))
                .relative()
                .overflow_hidden()
                .gap_3()
                .p_5()
                .border_1()
                .border_color(border_col)
                .rounded_xl()
                .bg(sidebar_bg)
                .shadow_sm()
                .hover(|this| this.border_color(primary).shadow_lg().bg(sidebar_hover_bg))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.clone_template(&template, window, cx);
                }))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(px(3.0))
                        .bg(category_accent),
                )
                .child(
                    h_flex()
                        .items_start()
                        .gap_3()
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(px(56.0))
                                .h(px(56.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_xl()
                                .bg(hsla(primary_h, primary_s, primary_l, 0.15))
                                .child(Icon::new(icon).size(px(32.0)).text_color(primary)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(fg)
                                        .child(name),
                                )
                                .child(
                                    match category.as_str() {
                                        "2D" => Tag::success(),
                                        "3D" => Tag::primary(),
                                        "Strategy" => Tag::warning(),
                                        "RPG" => Tag::info(),
                                        "Racing" => Tag::danger(),
                                        _ => Tag::secondary(),
                                    }
                                    .xsmall()
                                    .rounded_full()
                                    .child(category),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .line_height(rems(1.4))
                        .text_color(muted_fg)
                        .child(desc),
                )
                .child(div().w_full().h(px(1.0)).bg(border_col))
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Github)
                                .size(px(14.0))
                                .text_color(muted_fg),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child("Click to clone from template"),
                        ),
                )
        },
    )
}
