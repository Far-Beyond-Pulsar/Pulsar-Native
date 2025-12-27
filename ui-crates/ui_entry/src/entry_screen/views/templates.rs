use gpui::{prelude::*, *};
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _, StyledExt,
    divider::Divider, scroll::ScrollbarAxis, progress::Progress,
};
use crate::entry_screen::{EntryScreen, Template};

pub fn render_templates(screen: &mut EntryScreen, cols: usize, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let templates = screen.templates.clone();
    let has_progress = screen.clone_progress.is_some();
    
    v_flex()
        .size_full()
        .scrollable(ScrollbarAxis::Vertical)
        .p_12()
        .gap_6()
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Project Templates")
        )
        .child(Divider::horizontal())
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .mb_4()
                .child("Choose a template to start your project. Templates are cloned from Git with full progress tracking.")
        )
        .children(if has_progress {
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
        })
        .child(render_template_grid(screen, templates, cols, cx))
}

fn render_template_grid(screen: &mut EntryScreen, templates: Vec<Template>, cols: usize, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let mut container = v_flex().gap_6();
    let mut row = h_flex().gap_6();
    let mut count = 0;
    
    for template in templates {
        let template_clone = template.clone();
        let template_name = template.name.clone();
        let template_desc = template.description.clone();
        let template_category = template.category.clone();
        let template_icon = template.icon;
        
        let card = v_flex()
            .id(SharedString::from(format!("template-{}", template_name)))
            .w(px(320.))
            .h(px(200.))
            .gap_3()
            .p_4()
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .bg(theme.sidebar)
            .hover(|this| this.border_color(theme.primary).shadow_md())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.clone_template(&template_clone, window, cx);
            }))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(template_icon)
                            .size(px(32.))
                            .text_color(theme.primary)
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(template_name)
                    )
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(template_desc)
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(theme.accent.opacity(0.1))
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.accent)
                            .child(template_category)
                    )
                    .child(
                        Icon::new(IconName::GitHub)
                            .size(px(16.))
                            .text_color(theme.muted_foreground)
                    )
            );
        
        row = row.child(card);
        count += 1;
        
        if count >= cols {
            container = container.child(row);
            row = h_flex().gap_6();
            count = 0;
        }
    }
    
    if count > 0 {
        container = container.child(row);
    }
    
    container
}
