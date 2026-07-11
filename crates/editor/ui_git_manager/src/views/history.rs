//! History view: scrollable commit list with author avatars

use super::toolbar::render_toolbar;
use crate::GitManager;
use gpui::{prelude::FluentBuilder as _, *};
use ui::{ActiveTheme as _, Icon, IconName, StyledExt, h_flex, scroll::ScrollbarAxis, v_flex};

/// Deterministic hue from author name for avatar color.
fn author_hue(name: &str) -> f32 {
    let hash: u32 = name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_add((b as u32).wrapping_mul(31)));
    (hash % 360) as f32 / 360.0
}

/// Two-letter initials from "First Last" or single name.
fn author_initials(name: &str) -> String {
    let mut parts = name.split_whitespace().filter(|s| !s.is_empty());
    let first = parts
        .next()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let last = parts
        .last()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    format!("{}{}", first, last)
}

pub fn render_history_view(
    git_manager: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let commits = repo_state.commits.clone();
    let selected_commit = git_manager.selected_commit.clone();
    drop(repo_state);

    let radius = cx.theme().radius;
    let foreground = cx.theme().foreground;
    let muted_fg = cx.theme().muted_foreground;
    let primary = cx.theme().primary;
    let list_active = cx.theme().list_active;
    let list_hover = cx.theme().list_hover;
    let list_active_border = cx.theme().list_active_border;

    let mut commit_list = v_flex().id("git-history-scroll").w_full().gap_px().p_1();

    for commit in &commits {
        // Ensure the avatar for this author is fetched (no-op if cached/in-flight).
        git_manager.ensure_avatar_loaded(&commit.email, cx);

        let is_selected = selected_commit.as_ref() == Some(&commit.hash);
        let commit_msg = commit.message.lines().next().unwrap_or("").to_string();
        let short_hash = commit.short_hash.clone();
        let author = commit.author.clone();
        let date_str = commit.timestamp.format("%b %d, %H:%M").to_string();
        let author_date = format!("{} · {}", author, date_str);
        let commit_hash = commit.hash.clone();

        // Avatar color — deterministic from author name (fallback)
        let h = author_hue(&author);
        let avatar_bg = hsla(h, 0.55, 0.45, 1.0);
        let initials = author_initials(&author);

        // Resolve cached avatar image (may be None if not yet fetched).
        let cached_avatar = git_manager
            .avatar_cache
            .get(&commit.email)
            .and_then(|v| v.clone());

        let avatar_el: AnyElement = if let Some(arc) = cached_avatar {
            img(gpui::ImageSource::Render(arc))
                .w(px(28.))
                .h(px(28.))
                .rounded_full()
                .flex_shrink_0()
                .object_fit(gpui::ObjectFit::Cover)
                .into_any_element()
        } else {
            h_flex()
                .w(px(28.))
                .h(px(28.))
                .rounded_full()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .bg(avatar_bg)
                .text_size(px(10.))
                .font_weight(FontWeight::BOLD)
                .text_color(white())
                .child(initials)
                .into_any_element()
        };

        let bg = if is_selected {
            list_active
        } else {
            transparent_black()
        };

        let row = h_flex()
            .px_2()
            .py(px(6.))
            .rounded(radius)
            .gap_2()
            .bg(bg)
            .hover(move |s| s.bg(if is_selected { list_active } else { list_hover }))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.select_commit(commit_hash.clone(), cx);
                }),
            )
            // Left accent line for selected row
            .when(is_selected, |this| {
                this.border_l_2().border_color(list_active_border)
            })
            // Author avatar circle
            .child(avatar_el)
            // Message + meta
            .child(
                v_flex()
                    .flex_1()
                    .gap_px()
                    .overflow_hidden()
                    .child(
                        // Commit message
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .overflow_hidden()
                            .child(commit_msg),
                    )
                    .child(
                        // Author + date
                        div()
                            .text_size(px(10.))
                            .text_color(muted_fg)
                            .overflow_hidden()
                            .child(author_date),
                    ),
            )
            // Short hash pill on the right
            .child(
                div()
                    .px(px(5.))
                    .py(px(2.))
                    .rounded(radius)
                    .bg(primary.opacity(0.12))
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(primary)
                    .flex_shrink_0()
                    .child(short_hash),
            );

        commit_list = commit_list.child(row);
    }

    if commits.is_empty() {
        commit_list = commit_list.child(
            v_flex()
                .flex_1()
                .py_8()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::GitCommit)
                        .size(px(28.))
                        .text_color(muted_fg),
                )
                .child(div().text_xs().text_color(muted_fg).child("No commits yet")),
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
