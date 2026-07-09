//! Branches view: local / remote branch sections with click-to-switch

use super::toolbar::render_toolbar;
use crate::{GitManager, models::Branch};
use gpui::{prelude::FluentBuilder as _, *};
use ui::{ActiveTheme as _, Icon, IconName, StyledExt, h_flex, scroll::ScrollbarAxis, v_flex};

pub fn render_branches_view(
    git_manager: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let branches = repo_state.branches.clone();
    drop(repo_state);

    let local: Vec<Branch> = branches.iter().filter(|b| !b.is_remote).cloned().collect();
    let remote: Vec<Branch> = branches.iter().filter(|b| b.is_remote).cloned().collect();

    let branch_list = v_flex()
        .id("git-branches-scroll")
        .w_full()
        .p_2()
        .gap_2()
        .child(render_branch_section("Local", &local, cx))
        .child(render_branch_section("Remote", &remote, cx));

    v_flex()
        .size_full()
        .child(render_toolbar(git_manager, cx))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .child(branch_list.scrollable(ScrollbarAxis::Vertical)),
        )
}

fn render_branch_section(
    title: &str,
    branches: &[Branch],
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let radius = cx.theme().radius;
    let foreground = cx.theme().foreground;
    let muted_fg = cx.theme().muted_foreground;
    let primary = cx.theme().primary;
    let list_active = cx.theme().list_active;
    let list_hover = cx.theme().list_hover;
    let list_active_border = cx.theme().list_active_border;
    let list_head = cx.theme().list_head;
    let border = cx.theme().border;

    // Section header
    let header = h_flex()
        .w_full()
        .px_2()
        .py(px(5.))
        .bg(list_head)
        .rounded(radius)
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted_fg)
                .child(title.to_uppercase()),
        )
        .child(
            div()
                .px(px(5.))
                .py(px(1.))
                .rounded_full()
                .bg(border)
                .text_size(px(10.))
                .font_weight(FontWeight::BOLD)
                .text_color(muted_fg)
                .child(branches.len().to_string()),
        );

    let mut list = v_flex().gap_px();

    for branch in branches {
        let branch_name = branch.name.clone();
        let branch_name_for_switch = branch.name.clone();
        let is_current = branch.is_current;

        let text_color = if is_current { foreground } else { muted_fg };
        let font_weight = if is_current {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        };
        let bg = if is_current {
            list_active
        } else {
            transparent_black()
        };
        let icon_color = if is_current { primary } else { muted_fg };

        // Display name: for remote branches, dim the remote prefix
        let display_name = {
            // "origin/main" → show "main" bold + "origin/" dimmed prefix
            if let Some(slash) = branch_name.find('/') {
                let _remote = &branch_name[..slash];
                let local_part = &branch_name[slash + 1..];
                local_part.to_string()
            } else {
                branch_name.clone()
            }
        };
        let remote_prefix: Option<String> = if branch.is_remote {
            branch_name
                .find('/')
                .map(|i| format!("{}/", &branch_name[..i]))
        } else {
            None
        };

        let mut item = h_flex()
            .px_2()
            .py(px(6.))
            .rounded(radius)
            .gap_1()
            .items_center()
            .bg(bg)
            .hover(move |s| s.bg(if is_current { list_active } else { list_hover }))
            // Left accent line for current branch
            .when(is_current, |this| {
                this.border_l_2().border_color(list_active_border)
            })
            .child(
                Icon::new(IconName::GitBranch)
                    .size(px(12.))
                    .text_color(icon_color),
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_px()
                    .overflow_hidden()
                    .when_some(remote_prefix, |this, prefix| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .flex_shrink_0()
                                .child(prefix),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(text_color)
                            .font_weight(font_weight)
                            .overflow_hidden()
                            .child(display_name),
                    ),
            );

        if is_current {
            item = item.child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .px(px(5.))
                    .py(px(2.))
                    .rounded_full()
                    .bg(primary.opacity(0.15))
                    .border_1()
                    .border_color(primary.opacity(0.4))
                    .child(Icon::new(IconName::Check).size(px(10.)).text_color(primary))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(primary)
                            .child("current"),
                    ),
            );
        } else {
            item = item.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.switch_branch(branch_name_for_switch.clone(), cx);
                }),
            );
        }

        list = list.child(item);
    }

    if branches.is_empty() {
        list = list.child(
            div()
                .px_2()
                .py_3()
                .text_xs()
                .text_color(muted_fg)
                .child("No branches"),
        );
    }

    v_flex().gap_1().child(header).child(list)
}
