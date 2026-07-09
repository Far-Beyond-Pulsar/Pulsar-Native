use gpui::prelude::*;
use gpui::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _};

use crate::screen::EntryScreen;

pub fn render_upstream_prompt(screen: &mut EntryScreen, _window: &mut Window, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let template_url = screen.state.ui.show_git_upstream_prompt.as_ref()
        .map(|(_, url)| url.clone())
        .unwrap_or_default();
    let upstream_url_input = screen.inputs().git_upstream_url.clone();

    div()
        .absolute()
        .size_full()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.background.opacity(0.86))
        .child(
            v_flex()
                .w_full()
                .max_w(px(520.))
                .p_6()
                .gap_5()
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .shadow_lg()
                .when(!template_url.is_empty(), |this| {
                    this.child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.muted_foreground)
                                    .child("Template"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child(template_url),
                            ),
                    )
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("Set Up Git Upstream"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .line_height(relative(1.5))
                                .child("This project was created from a template. Optionally, set up your own Git repository as the upstream remote (origin) so you can push changes to your own repo."),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child("Your Repository URL"),
                        )
                        .child(
                            ui::input::Input::new(&upstream_url_input).w_full(),
                        ),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .justify_end()
                        .child(
                            Button::new("skip-upstream")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.setup_git_upstream(cx);
                                })),
                        )
                        .child(
                            Button::new("setup-upstream")
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.setup_git_upstream(cx);
                                })),
                        ),
                ),
        )
}
