use gpui::prelude::*;
use gpui::*;
use ui::{
    button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Disableable,
    Icon, IconName,
};

use crate::screen::EntryScreen;

pub fn render_clone_git(
    screen: &mut EntryScreen,
    _window: &mut Window,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let repo_url_input = screen.inputs().git_repo_url.clone();
    let is_cloning = screen.state.clone_progress.is_some();

    v_flex()
        .flex_1()
        .h_full()
                .overflow_hidden()
        .px_8()
        .pt_6()
        .gap_6()
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("Clone from Git"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Clone an existing Git repository to work on in Pulsar"),
                ),
        )
        .child(
            v_flex()
                .max_w(px(600.))
                .gap_4()
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child("Repository URL"),
                        )
                        .child(
                            ui::input::Input::new(&repo_url_input).w_full(),
                        ),
                )
                .child(
                    v_flex()
                        .p_4()
                        .gap_2()
                        .rounded_lg()
                        .bg(theme.secondary.opacity(0.12))
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("Supported URL Formats"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .line_height(relative(1.6))
                                .child("https://github.com/user/repo.git\ngit@github.com:user/repo.git\nhttps://gitlab.com/user/repo.git"),
                        ),
                )
                .child(
                    v_flex()
                        .gap_3()
                        .when(is_cloning, |this| {
                            this.child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Download)
                                                    .size(px(16.)),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(theme.foreground)
                                                    .child("Cloning repository..."),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(6.))
                                            .bg(theme.secondary.opacity(0.3))
                                            .rounded_full()
                                            .child(
                                                div()
                                                    .h_full()
                                                    .rounded_full()
                                                    .bg(theme.accent)
                                                    .w(relative(0.3)),
                                            ),
                                    ),
                            )
                        })
                        .child(
                            Button::new("clone-repo-btn")
                                .label("Clone Repository")
                                .primary()
                                .disabled(is_cloning)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let url = this.state.input.git_repo_url_text.clone();
                                    if !url.trim().is_empty() {
                                        this.clone_git_repo(Some(url), cx);
                                    }
                                })),
                        ),
                ),
        )
}
