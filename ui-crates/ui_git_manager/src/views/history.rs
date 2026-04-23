//! History view: scrollable commit list

use super::toolbar::render_toolbar;
use crate::GitManager;
use gpui::*;
use ui::{ActiveTheme as _, StyledExt, h_flex, scroll::ScrollbarAxis, v_flex};

pub fn render_history_view(
    git_manager: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let commits = repo_state.commits.clone();
    let selected_commit = git_manager.selected_commit.clone();
    drop(repo_state);

    // Copy theme values before any listener() calls
    let radius = cx.theme().radius;
    let foreground = cx.theme().foreground;
    let muted_fg = cx.theme().muted_foreground;
    let primary = cx.theme().primary;
    let list_active = cx.theme().list_active;
    let list_hover = cx.theme().list_hover;

    let mut commit_list = v_flex().id("git-history-scroll").w_full().gap_px().p_1();

    for commit in &commits {
        let is_selected = selected_commit.as_ref() == Some(&commit.hash);
        let commit_msg = commit.message.lines().next().unwrap_or("").to_string();
        let short_hash = commit.short_hash.clone();
        let author_date = format!(
            "{} · {}",
            commit.author,
            commit.timestamp.format("%m/%d %H:%M")
        );
        let commit_hash = commit.hash.clone();
        let bg = if is_selected {
            list_active
        } else {
            gpui::transparent_black()
        };

        commit_list = commit_list.child(
            v_flex()
                .px_2()
                .py_1()
                .rounded(radius)
                .gap_px()
                .bg(bg)
                .hover(move |s| s.bg(list_hover))
                .cursor_pointer()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                        this.select_commit(commit_hash.clone(), cx);
                    }),
                )
                // First line: commit message
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .overflow_hidden()
                        .child(commit_msg),
                )
                // Second line: hash + author + date
                .child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(div().text_color(primary).child(short_hash))
                        .child(author_date),
                ),
        );
    }

    v_flex()
        .size_full()
        .child(render_toolbar(git_manager, cx))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .child(commit_list.scrollable(ScrollbarAxis::Vertical)),
        )
}
