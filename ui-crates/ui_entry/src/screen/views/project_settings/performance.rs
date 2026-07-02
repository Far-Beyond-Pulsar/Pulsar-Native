use gpui::*;
use gpui::prelude::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Icon, IconName};

use crate::screen::EntryScreen;
use super::helpers::render_size_bar;
use crate::util::formatters::format_size;

pub fn render_performance_tab(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let Some(ref settings) = screen.state.ui.project_settings else {
        return div().into_any_element();
    };

    let disk_size = settings.disk_size.unwrap_or(0);
    let git_size = settings.git_repo_size.unwrap_or(0);
    let total_size = disk_size + git_size;
    let commit_count = settings.commit_count.unwrap_or(0) as f64;
    let git_ratio = if total_size > 0 { git_size as f64 / total_size as f64 } else { 0.0 };

    let health_score = if git_ratio > 0.5 {
        60
    } else if git_ratio > 0.3 {
        75
    } else if git_ratio > 0.1 {
        85
    } else {
        95
    };

    let health_color = if health_score >= 80 { theme.success_foreground }
    else if health_score >= 60 { theme.warning }
    else { gpui::red() };

    v_flex()
        .gap_6()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Performance"),
        )
        .child(
            h_flex()
                .gap_6()
                .items_center()
                .child(
                    v_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_2xl()
                                .font_weight(FontWeight::BOLD)
                                .text_color(health_color)
                                .child(format!("{}", health_score)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Repo Health"),
                        ),
                )
                .child(
                    div()
                        .w(px(2.))
                        .h(px(40.))
                        .bg(theme.border),
                )
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(theme.success_foreground))
                                .child(div().text_sm().text_color(theme.foreground).child("Project size is reasonable")),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(if git_ratio < 0.3 { theme.success_foreground } else { theme.warning }))
                                .child(div().text_sm().text_color(theme.foreground).child(format!("Git history is {:.0}% of project", git_ratio * 100.0))),
                        ),
                ),
        )
        .child(
            render_size_bar("Total Project Size", total_size, total_size, theme.accent, cx.theme()),
        )
        .child(
            v_flex()
                .gap_4()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Statistics"),
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
                                .child(div().text_xs().text_color(theme.muted_foreground).child("Commits"))
                                .child(div().text_lg().font_weight(FontWeight::BOLD).text_color(theme.foreground).child(if commit_count > 0.0 { format!("{}", commit_count as u64) } else { "N/A".to_string() })),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .p_4()
                                .rounded_lg()
                                .bg(theme.secondary.opacity(0.08))
                                .gap_1()
                                .child(div().text_xs().text_color(theme.muted_foreground).child("Disk Usage"))
                                .child(div().text_lg().font_weight(FontWeight::BOLD).text_color(theme.foreground).child(format_size(total_size))),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .p_4()
                                .rounded_lg()
                                .bg(theme.secondary.opacity(0.08))
                                .gap_1()
                                .child(div().text_xs().text_color(theme.muted_foreground).child("Git Ratio"))
                                .child(div().text_lg().font_weight(FontWeight::BOLD).text_color(theme.foreground).child(format!("{:.0}%", git_ratio * 100.0))),
                        ),
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Optimization Recommendations"),
                )
                .child(when(git_ratio > 0.3, || {
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(theme.warning.opacity(0.15))
                        .text_sm()
                        .text_color(theme.warning)
                        .child("Your .git directory is large. Consider running Git GC or cleaning up branches.")
                }))
                .child(when(git_ratio <= 0.3, || {
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(theme.success_foreground.opacity(0.15))
                        .text_sm()
                        .text_color(theme.success_foreground)
                        .child("Repository is in good shape. No optimization needed.")
                })),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("refresh-perf")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| { this.refresh_project_settings(cx); })),
                ),
        )
        .into_any_element()
}

fn when(cond: bool, f: impl FnOnce() -> gpui::Div) -> impl IntoElement {
    if cond { f().into_any_element() } else { div().into_any_element() }
}
