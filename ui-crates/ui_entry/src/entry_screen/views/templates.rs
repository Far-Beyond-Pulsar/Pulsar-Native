use crate::entry_screen::{virtual_grid::render_card_grid, EntryScreen};
use gpui::{prelude::*, *};
use ui::Sizable;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    progress::Progress,
    spinner::Spinner,
    tag::Tag,
    v_flex, ActiveTheme as _, Icon, IconName,
};

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
    _cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    // Match the layout used by `EntryScreen::calculate_columns`
    const CARD_WIDTH: f32 = 320.0;
    const THUMB_HEIGHT: f32 = 160.0;
    const STRIP_HEIGHT: f32 = 84.0;
    const CARD_HEIGHT: f32 = THUMB_HEIGHT + STRIP_HEIGHT;
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
            let repo_url = template.repo_url.clone();

            // Kick off (or continue) loading this template's thumbnail.
            view.ensure_template_thumbnail_loaded(&template, cx);
            let thumb_state = view.template_thumbnails.get(&template.repo_url).cloned();
            let thumb_loading = matches!(thumb_state, None | Some(None));
            let thumb_arc = thumb_state.flatten();

            let theme = cx.theme();
            let border_col = theme.border;
            let sidebar_bg = theme.sidebar;
            let primary = theme.primary;
            let primary_h = theme.primary.h;
            let primary_s = theme.primary.s;
            let primary_l = theme.primary.l;
            let fg = theme.foreground;
            let muted_fg = theme.muted_foreground;

            let category_accent = match category.as_str() {
                "2D" => cx.theme().success,
                "3D" => cx.theme().primary,
                "Strategy" => cx.theme().warning,
                "RPG" => cx.theme().info,
                "Racing" => cx.theme().danger,
                _ => cx.theme().muted,
            };
            let category_tag = match category.as_str() {
                "2D" => Tag::success(),
                "3D" => Tag::primary(),
                "Strategy" => Tag::warning(),
                "RPG" => Tag::info(),
                "Racing" => Tag::danger(),
                _ => Tag::secondary(),
            }
            .xsmall()
            .rounded_full()
            .child(category.clone());

            v_flex()
                .id(SharedString::from(format!("template-{}", name)))
                .w(px(card_w))
                .h(px(CARD_HEIGHT))
                .overflow_hidden()
                .border_1()
                .border_color(border_col)
                .rounded_xl()
                .bg(sidebar_bg)
                .shadow_sm()
                .hover(|this| this.border_color(primary).shadow_lg())
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.clone_template(&template, window, cx);
                }))
                // ── Thumbnail with overlaid controls ────────────────────────────
                .child(
                    div()
                        .id(SharedString::from(format!("template-thumb-{}", name)))
                        .relative()
                        .w_full()
                        .h(px(THUMB_HEIGHT))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .bg(hsla(primary_h, primary_s, primary_l, 0.15))
                        .map(|el| {
                            if let Some(arc) = thumb_arc {
                                el.child(
                                    img(gpui::ImageSource::Render(arc))
                                        .w_full()
                                        .h_full()
                                        .object_fit(gpui::ObjectFit::Cover),
                                )
                            } else if thumb_loading {
                                el.flex().items_center().justify_center().child(
                                    Spinner::new()
                                        .with_size(ui::Size::Medium)
                                        .color(muted_fg),
                                )
                            } else {
                                el.flex().items_center().justify_center().child(
                                    Icon::new(icon).size(px(48.0)).text_color(primary),
                                )
                            }
                        })
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .w_full()
                                .h(px(3.0))
                                .bg(category_accent),
                        )
                        .child(div().absolute().top_2().left_2().child(category_tag))
                        .child(
                            h_flex()
                                .absolute()
                                .top_2()
                                .right_2()
                                .gap_1()
                                .p_1()
                                .rounded_md()
                                .bg(hsla(0.0, 0.0, 0.0, 0.45))
                                .child(
                                    Button::new(SharedString::from(format!("template-github-{}", name)))
                                        .icon(IconName::Github)
                                        .xsmall()
                                        .tooltip("View on GitHub")
                                        .with_variant(ui::button::ButtonVariant::Ghost)
                                        .on_click({
                                            let url = repo_url.clone();
                                            move |_, _, _| {
                                                let _ = open::that(&url);
                                            }
                                        }),
                                ),
                        ),
                )
                // ── Title + meta strip ──────────────────────────────────────────
                .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .justify_center()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(name),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(desc),
                        ),
                )
        },
    )
}
