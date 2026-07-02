use gpui::*;
use gpui::prelude::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _};

use crate::screen::EntryScreen;
use super::helpers::render_size_bar;
use crate::util::formatters::format_size;

pub fn render_disk_info_tab(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let Some(ref settings) = screen.state.ui.project_settings else {
        return div().into_any_element();
    };

    let disk_size = settings.disk_size.unwrap_or(0);
    let git_size = settings.git_repo_size.unwrap_or(0);
    let total_size = disk_size + git_size;
    let project_path = settings.project_path.clone();

    v_flex()
        .gap_6()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Disk Info"),
        )
        .child(
            v_flex()
                .gap_4()
                .child(
                    render_size_bar("Total Project Size", total_size, total_size, theme.accent, cx.theme()),
                )
                .child(
                    render_size_bar("Working Files", disk_size, total_size, theme.accent.opacity(0.7), cx.theme()),
                )
                .child(
                    render_size_bar(
                        ".git Repository",
                        git_size,
                        total_size,
                        theme.warning.opacity(0.8),
                        cx.theme(),
                    ),
                ),
        )
        .child(
            h_flex()
                .gap_4()
                .child(
                    v_flex()
                        .flex_1()
                        .p_4()
                        .rounded_lg()
                        .bg(theme.secondary.opacity(0.08))
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Total"),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.foreground)
                                .child(format_size(total_size)),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .p_4()
                        .rounded_lg()
                        .bg(theme.secondary.opacity(0.08))
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Working Files"),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.foreground)
                                .child(format_size(disk_size)),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .p_4()
                        .rounded_lg()
                        .bg(theme.secondary.opacity(0.08))
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Git Objects"),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.foreground)
                                .child(format_size(git_size)),
                        ),
                ),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("refresh-disk")
                        .label("Refresh")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| { this.refresh_project_settings(cx); })),
                )
                .child(
                    Button::new("run-gc")
                        .label("Run Git GC")
                        .ghost()
                        .tooltip("Run git garbage collection to reduce repository size")
                        .on_click(cx.listener(move |_, _, _, _cx| {
                            let _ = std::process::Command::new("git")
                                .args(["gc", "--prune=now"])
                                .current_dir(&project_path)
                                .output();
                        })),
                ),
        )
        .into_any_element()
}
