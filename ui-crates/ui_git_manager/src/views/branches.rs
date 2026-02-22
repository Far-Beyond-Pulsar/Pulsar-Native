//! Branches view: local / remote branch sections with click-to-switch

use crate::{GitManager, models::Branch};
use gpui::*;
use ui::{h_flex, v_flex, Icon, IconName, ActiveTheme as _, StyledExt, scroll::ScrollbarAxis};
use super::toolbar::render_toolbar;

pub fn render_branches_view(git_manager: &mut GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let branches = repo_state.branches.clone();
    drop(repo_state);

    let local: Vec<Branch> = branches.iter().filter(|b| !b.is_remote).cloned().collect();
    let remote: Vec<Branch> = branches.iter().filter(|b| b.is_remote).cloned().collect();

    let branch_list = v_flex()
        .id("git-branches-scroll")
        .w_full()
        .p_2()
        .gap_3()
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
    // Copy theme values before any listener() calls
    let radius = cx.theme().radius;
    let foreground = cx.theme().foreground;
    let muted_fg = cx.theme().muted_foreground;
    let primary = cx.theme().primary;
    let list_active = cx.theme().list_active;
    let list_hover = cx.theme().list_hover;

    let mut list = v_flex().gap_px();

    for branch in branches {
        let branch_name = branch.name.clone();
        let branch_name_for_switch = branch.name.clone();
        let is_current = branch.is_current;
        let is_remote = branch.is_remote;

        let icon_color = if is_current { primary } else { muted_fg };
        let text_color = if is_current { foreground } else { muted_fg };
        let font_weight = if is_current {
            gpui::FontWeight::SEMIBOLD
        } else {
            gpui::FontWeight::NORMAL
        };
        let bg = if is_current { list_active } else { gpui::transparent_black() };

        let mut item = h_flex()
            .px_2()
            .py_1()
            .rounded(radius)
            .gap_1()
            .items_center()
            .bg(bg)
            .hover(move |s| s.bg(if is_current { list_active } else { list_hover }))
            .child(Icon::new(IconName::GitBranch).size(px(12.)).text_color(icon_color))
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(text_color)
                    .font_weight(font_weight)
                    .overflow_hidden()
                    .child(branch_name),
            );

        if is_current {
            item = item.child(
                div()
                    .px_1()
                    .rounded(radius)
                    .bg(primary.opacity(0.15))
                    .text_xs()
                    .text_color(primary)
                    .child("✓"),
            );
        } else if !is_remote {
            // Local non-current branch: click to switch (carries uncommitted changes)
            item = item.cursor_pointer().on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    this.switch_branch(branch_name_for_switch.clone(), cx);
                }),
            );
        }

        list = list.child(item);
    }

    v_flex()
        .gap_1()
        .child(
            div()
                .px_1()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(muted_fg)
                .child(format!("{} ({})", title, branches.len())),
        )
        .child(list)
}
