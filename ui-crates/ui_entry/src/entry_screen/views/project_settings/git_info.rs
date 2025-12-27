use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, divider::Divider, ActiveTheme as _, IconName,
};
use super::{types::{ProjectSettings, format_size}, helpers::render_info_section};
use crate::entry_screen::EntryScreen;

pub fn render_git_info_tab(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    
    v_flex()
        .gap_6()
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Git Information")
        )
        .child(Divider::horizontal())
        .child(render_info_section("Repository", vec![
            ("Remote URL", settings.remote_url.clone().unwrap_or_else(|| "No remote configured".to_string())),
            ("Current Branch", settings.current_branch.clone().unwrap_or_else(|| "N/A".to_string())),
            ("Total Commits", settings.commit_count.map(|c| c.to_string()).unwrap_or_else(|| "N/A".to_string())),
            ("Total Branches", settings.branch_count.map(|c| c.to_string()).unwrap_or_else(|| "N/A".to_string())),
            (".git Size", format_size(settings.git_repo_size)),
        ], &theme))
        .child(render_info_section("Latest Commit", vec![
            ("Date", settings.last_commit_date.clone().unwrap_or_else(|| "N/A".to_string())),
            ("Message", settings.last_commit_message.clone().unwrap_or_else(|| "N/A".to_string())),
        ], &theme))
        .child(render_info_section("Working Directory", vec![
            ("Modified Files", settings.uncommitted_changes.map(|c| {
                if c == 0 {
                    "Clean - No changes".to_string()
                } else {
                    format!("{} file(s) modified", c)
                }
            }).unwrap_or_else(|| "N/A".to_string())),
            ("Untracked Files", settings.untracked_files.map(|c| format!("{} file(s)", c)).unwrap_or_else(|| "N/A".to_string())),
            ("Stashed Changes", settings.stash_count.map(|c| format!("{} stash(es)", c)).unwrap_or_else(|| "0 stashes".to_string())),
        ], &theme))
        .child(
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            Button::new("refresh-git-info")
                                .label("Refresh Git Info")
                                .icon(IconName::ArrowUp)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Primary)
                                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.refresh_project_settings(cx);
                                }))
                        )
                        .child(
                            Button::new("open-git-ui")
                                .label("Open Git GUI")
                                .icon(IconName::GitHub)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        use crate::entry_screen::integration_launcher;
                                        let _ = integration_launcher::launch_git_tool("git", &path);
                                    }
                                })
                        )
                )
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            Button::new("view-git-log")
                                .label("View Git Log")
                                .icon(IconName::GitHub)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        use crate::entry_screen::integration_launcher;
                                        // Use preferred git tool or default to "git"
                                        let _ = integration_launcher::launch_git_tool("git", &path);
                                    }
                                })
                        )
                        .child(
                            Button::new("copy-remote-url")
                                .label("Copy Remote URL")
                                .icon(IconName::Copy)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let url = settings.remote_url.clone();
                                    move |_, _, cx| {
                                        if let Some(remote_url) = &url {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(remote_url.clone()));
                                        }
                                    }
                                })
                        )
                )
        )
}
