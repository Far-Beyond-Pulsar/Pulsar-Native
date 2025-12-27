use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, Icon, IconName, ActiveTheme as _,
    input::TextInput,
};
use crate::entry_screen::EntryScreen;

pub fn render_upstream_prompt(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let (_, template_url) = screen.show_git_upstream_prompt.as_ref().unwrap();
    let is_template = !template_url.is_empty();
    
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.background.opacity(0.95))
        .child(
            v_flex()
                .w_full()
                .max_w(px(500.))
                .p_6()
                .gap_6()
                .bg(theme.background)
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::GitHub)
                                .size(px(24.))
                                .text_color(theme.primary)
                        )
                        .child(
                            div()
                                .text_xl()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(if is_template {
                                    "Configure Git Repository"
                                } else {
                                    "Link to Git Repository (Optional)"
                                })
                        )
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(if is_template {
                            "This project was cloned from a template. The template repository has been renamed to 'template'. Enter your repository URL to set it as the default 'origin' remote."
                        } else {
                            "Enter the URL of your git repository to enable version control for this project. You can skip this step and configure it later."
                        })
                )
                .when(is_template, |this| {
                    this.child(
                        div()
                            .w_full()
                            .p_3()
                            .rounded_lg()
                            .bg(theme.accent.opacity(0.1))
                            .border_1()
                            .border_color(theme.accent.opacity(0.3))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.accent)
                                            .child("Template Repository (renamed to 'template' remote)")
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(template_url.clone())
                                    )
                            )
                    )
                })
                .child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child("Your Repository URL")
                        )
                        .child(TextInput::new(&screen.git_upstream_url_input))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Examples: https://github.com/user/repo.git, git@github.com:user/repo.git")
                        )
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .justify_end()
                        .child(
                            Button::new("skip-upstream")
                                .ghost()
                                .label(if is_template { "Skip Setup" } else { "Skip for Now" })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.setup_git_upstream(true, cx);
                                }))
                        )
                        .child(
                            Button::new("setup-upstream")
                                .primary()
                                .label("Setup Repository")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.setup_git_upstream(false, cx);
                                }))
                        )
                )
        )
}
