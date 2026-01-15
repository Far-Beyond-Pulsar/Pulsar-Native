use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, Icon, IconName, ActiveTheme as _,
    divider::Divider, progress::Progress,
    input::TextInput,
};
use crate::entry_screen::EntryScreen;

pub fn render_clone_git(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();

    let progress_message = screen.clone_progress.as_ref()
        .map(|p| {
            let prog = p.lock();
            (prog.message.clone(), prog.current, prog.total, prog.error.clone())
        });

    v_flex()
        .size_full()
        .p_12()
        .gap_8()
        .child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            Icon::new(IconName::GitHub)
                                .size(px(28.))
                                .text_color(theme.primary)
                        )
                        .child(
                            div()
                                .text_3xl()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(theme.foreground)
                                .child("Clone from Git")
                        )
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Clone an existing Pulsar project from any Git repository")
                )
        )
        .child(
            v_flex()
                .max_w(px(700.))
                .gap_8()
                .p_8()
                .border_1()
                .border_color(theme.border)
                .rounded_xl()
                .bg(theme.sidebar)
                .shadow_sm()
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("Repository URL")
                        )
                        .child(TextInput::new(&screen.git_repo_url_input))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Enter the Git repository URL (HTTPS or SSH)")
                        )
                )
                .child(
                    div()
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
                                        .child("Supported URL Formats")
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("• HTTPS: https://github.com/user/repo.git")
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("• SSH: git@github.com:user/repo.git")
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("• GitLab, Bitbucket, and other Git hosts")
                                )
                        )
                )
                .children(if let Some((message, current, total, error)) = progress_message {
                    Some(
                        v_flex()
                            .gap_3()
                            .p_4()
                            .border_1()
                            .border_color(theme.primary)
                            .rounded_md()
                            .bg(theme.background)
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
                                    .child(message)
                            )
                            .child(
                                Progress::new()
                                    .value(if total > 0 {
                                        (current as f32 / total as f32) * 100.0
                                    } else {
                                        0.0
                                    })
                            )
                            .children(error.map(|e| {
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(e)
                            }))
                    )
                } else {
                    None
                })
                .child(
                    h_flex()
                        .justify_end()
                        .child(
                            Button::new("clone-repo")
                                .label("Clone Repository")
                                .icon(IconName::GitHub)
                                .with_variant(ui::button::ButtonVariant::Primary)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let url = this.git_repo_url.clone();
                                    if !url.is_empty() {
                                        this.clone_git_repo(url, "Cloned Project".to_string(), false, window, cx);
                                    }
                                }))
                        )
                )
        )
}
