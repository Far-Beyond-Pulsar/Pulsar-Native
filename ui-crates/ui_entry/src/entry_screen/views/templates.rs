use gpui::{ prelude::*, * };
use ui::{
    h_flex,
    v_flex,
    Icon,
    IconName,
    ActiveTheme as _,
    StyledExt,
    divider::Divider,
    scroll::ScrollbarAxis,
    progress::Progress,
};
use crate::entry_screen::{ EntryScreen, Template };

pub fn render_templates(
    screen: &mut EntryScreen,
    cols: usize,
    cx: &mut Context<EntryScreen>
) -> impl IntoElement {
    let theme = cx.theme();
    let templates = screen.templates.clone();
    let has_progress = screen.clone_progress.is_some();

    v_flex()
        .size_full()
        .scrollable(ScrollbarAxis::Vertical)
        .p_12()
        .gap_8()
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_3xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("Project Templates")
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Choose a template to start your project quickly")
                )
        )
        .children(
            if has_progress {
                Some(
                    v_flex()
                        .gap_4()
                        .p_6()
                        .border_1()
                        .border_color(theme.primary)
                        .rounded_lg()
                        .bg(theme.sidebar)
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("Cloning Repository...")
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Please wait while we clone the template...")
                        )
                        .child(Progress::new().value(50.0))
                )
            } else {
                None
            }
        )
        .child(render_template_grid(screen, templates, cols, cx))
}

fn render_template_grid(
    screen: &mut EntryScreen,
    templates: Vec<Template>,
    cols: usize,
    cx: &mut Context<EntryScreen>
) -> impl IntoElement {
    let theme = cx.theme();
    let mut container = v_flex().gap_8();
    let mut row = h_flex().gap_8();
    let mut count = 0;

    for template in templates {
        let template_clone = template.clone();
        let template_name = template.name.clone();
        let template_desc = template.description.clone();
        let template_category = template.category.clone();
        let template_icon = template.icon;

        let card = v_flex()
            .id(SharedString::from(format!("template-{}", template_name)))
            .w(px(340.0))
            .h(px(220.0))
            .gap_4()
            .p_5()
            .border_1()
            .border_color(theme.border)
            .rounded_xl()
            .bg(theme.sidebar)
            .shadow_sm()
            .hover(|this| {
                this.border_color(theme.primary)
                    .shadow_lg()
                    .bg(
                        hsla(
                            theme.sidebar.h,
                            theme.sidebar.s,
                            theme.sidebar.l * 1.05,
                            theme.sidebar.a
                        )
                    )
            })
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.clone_template(&template_clone, window, cx);
                })
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
                            .bg(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.15))
                            .child(
                                Icon::new(template_icon).size(px(32.0)).text_color(theme.primary)
                            )
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child(template_name)
                            )
                            .child(
                                div()
                                    .px_2p5()
                                    .py_1()
                                    .rounded_md()
                                    .bg(
                                        hsla(
                                            theme.primary.h,
                                            theme.primary.s,
                                            theme.primary.l,
                                            0.15
                                        )
                                    )
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.foreground)
                                    .child(template_category)
                            )
                    )
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .line_height(rems(1.4))
                    .text_color(theme.muted_foreground)
                    .child(template_desc)
            )
            .child(div().w_full().h(px(1.0)).bg(theme.border))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::GitHub)
                            .size(px(14.0))
                            .text_color(theme.muted_foreground)
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Click to clone from template")
                    )
            );

        row = row.child(card);
        count += 1;

        if count >= cols {
            container = container.child(row);
            row = h_flex().gap_8();
            count = 0;
        }
    }

    if count > 0 {
        container = container.child(row);
    }

    container
}
