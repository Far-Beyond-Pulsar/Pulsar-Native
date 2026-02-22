//! Compact toolbar: tab selector + branch/sync row

use crate::{GitManager, GitView};
use gpui::*;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
};

pub fn render_toolbar(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let current_view = git_manager.current_view;
    let current_branch = repo_state.current_branch.clone();
    let ahead = repo_state.ahead;
    let behind = repo_state.behind;
    drop(repo_state);

    // Copy theme values (all Copy types) before any cx.listener() calls
    let border = cx.theme().border;
    let muted_fg = cx.theme().muted_foreground;

    // Tab row — compact buttons, no fixed width so they share space naturally
    let tabs = h_flex()
        .w_full()
        .gap_px()
        .child(
            Button::new("tab-changes")
                .label("Changes")
                .compact()
                .with_variant(if current_view == GitView::Changes { ButtonVariant::Primary } else { ButtonVariant::Ghost })
                .on_click(cx.listener(|this, _, _, cx| this.switch_view(GitView::Changes, cx))),
        )
        .child(
            Button::new("tab-history")
                .label("History")
                .compact()
                .with_variant(if current_view == GitView::History { ButtonVariant::Primary } else { ButtonVariant::Ghost })
                .on_click(cx.listener(|this, _, _, cx| this.switch_view(GitView::History, cx))),
        )
        .child(
            Button::new("tab-branches")
                .label("Branches")
                .compact()
                .with_variant(if current_view == GitView::Branches { ButtonVariant::Primary } else { ButtonVariant::Ghost })
                .on_click(cx.listener(|this, _, _, cx| this.switch_view(GitView::Branches, cx))),
        );

    // Branch + sync row
    let mut sync_row = h_flex()
        .w_full()
        .gap_1()
        .items_center()
        .child(Icon::new(IconName::GitBranch).size(px(12.)).text_color(muted_fg))
        .child(
            div()
                .flex_1()
                .text_xs()
                .text_color(muted_fg)
                .overflow_hidden()
                .child(current_branch),
        )
        .child(
            Button::new("refresh")
                .icon(IconName::Refresh)
                .ghost()
                .compact()
                .tooltip("Refresh".to_string())
                .on_click(cx.listener(|this, _, _, cx| this.refresh_state(cx))),
        );

    if ahead > 0 {
        sync_row = sync_row.child(
            Button::new("push")
                .icon(IconName::ArrowUp)
                .ghost()
                .compact()
                .tooltip(format!("Push {} commit(s)", ahead))
                .on_click(cx.listener(|this, _, _, cx| this.push(cx))),
        );
    }
    if behind > 0 {
        sync_row = sync_row.child(
            Button::new("pull")
                .icon(IconName::ArrowDown)
                .ghost()
                .compact()
                .tooltip(format!("Pull {} commit(s)", behind))
                .on_click(cx.listener(|this, _, _, cx| this.pull(cx))),
        );
    }

    v_flex()
        .w_full()
        .px_2()
        .pt_2()
        .pb_1()
        .gap_1()
        .border_b_1()
        .border_color(border)
        .child(tabs)
        .child(sync_row)
}
