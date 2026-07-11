use gpui::prelude::*;
use gpui::*;
use ui::{
    button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Icon, IconName,
};

use super::helpers::render_info_section;
use crate::screen::EntryScreen;
use crate::util::formatters::format_size;

pub fn render_git_info_tab(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let Some(ref settings) = screen.state.ui.project_settings else {
        return div().into_any_element();
    };

    let remote_url = settings
        .remote_url
        .clone()
        .unwrap_or_else(|| "Not configured".to_string());
    let remote_url_copy = remote_url.clone();
    let current_branch = settings
        .current_branch
        .clone()
        .unwrap_or_else(|| "None".to_string());
    let commit_count = settings
        .commit_count
        .map(|c| c.to_string())
        .unwrap_or_else(|| "Loading...".to_string());
    let branch_count = settings
        .branch_count
        .map(|c| c.to_string())
        .unwrap_or_else(|| "Loading...".to_string());
    let git_size = settings
        .git_repo_size
        .map(|s| format_size(s))
        .unwrap_or_else(|| "Loading...".to_string());
    let last_commit = settings
        .last_commit_date
        .clone()
        .unwrap_or_else(|| "N/A".to_string());
    let last_msg = settings
        .last_commit_message
        .clone()
        .unwrap_or_else(|| "".to_string());
    let uncommitted = settings
        .uncommitted_changes
        .map(|c| c.to_string())
        .unwrap_or_else(|| "Loading...".to_string());
    let project_path = settings.project_path.clone();

    v_flex()
        .gap_6()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Git Info"),
        )
        .child(render_info_section(
            vec![
                ("Remote URL".to_string(), remote_url),
                ("Current Branch".to_string(), current_branch),
                ("Commits".to_string(), commit_count),
                ("Branches".to_string(), branch_count),
                (".git Size".to_string(), git_size),
                ("Last Commit Date".to_string(), last_commit),
                (
                    "Working Directory".to_string(),
                    format!("{} uncommitted change(s)", uncommitted),
                ),
            ],
            cx,
        ))
        .when(!last_msg.is_empty(), |this| {
            this.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground)
                            .child("Last Commit Message"),
                    )
                    .child(
                        div()
                            .p_3()
                            .rounded_md()
                            .bg(theme.secondary.opacity(0.12))
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(last_msg),
                    ),
            )
        })
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("refresh-git-info")
                        .label("Refresh")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_project_settings(cx);
                        })),
                )
                .child(
                    Button::new("open-git-gui")
                        .label("Open Git GUI")
                        .ghost()
                        .on_click(cx.listener(move |_, _, _, _cx| {
                            let _ = open::that(&project_path);
                        })),
                )
                .child({
                    let remote_url2 = remote_url_copy.clone();
                    Button::new("copy-remote-url")
                        .label("Copy Remote URL")
                        .ghost()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                remote_url2.clone(),
                            ));
                        }))
                }),
        )
        .into_any_element()
}
