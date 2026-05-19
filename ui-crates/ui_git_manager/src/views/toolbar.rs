//! Compact toolbar: segmented tab selector + branch pill + sync row

use crate::{GitManager, GitView};
use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    ActiveTheme as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex,
};

pub fn render_toolbar(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let current_view = git_manager.current_view;
    let current_branch = repo_state.current_branch.clone();
    let ahead = repo_state.ahead;
    let behind = repo_state.behind;
    drop(repo_state);

    let border = cx.theme().border;
    let danger = cx.theme().danger;
    let warning = cx.theme().warning;
    let primary = cx.theme().primary;
    let foreground = cx.theme().foreground;
    let radius = cx.theme().radius;
    let tab_bar = cx.theme().tab_bar;
    let tab_active = cx.theme().tab_active;
    let tab_active_fg = cx.theme().tab_active_foreground;
    let tab_fg = cx.theme().tab_foreground;
    let tab_seg = cx.theme().tab_bar_segmented;
    let muted = cx.theme().muted;

    // ── Segmented tab control ─────────────────────────────────────────────────
    let is_changes = current_view == GitView::Changes;
    let tab_changes = h_flex()
        .id("tab-changes")
        .flex_1()
        .py(px(5.))
        .rounded(radius)
        .justify_center()
        .text_xs()
        .font_weight(if is_changes {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(if is_changes { tab_active_fg } else { tab_fg })
        .bg(if is_changes {
            tab_active
        } else {
            transparent_black()
        })
        .hover(move |s| {
            if is_changes {
                s
            } else {
                s.bg(muted.opacity(0.6))
            }
        })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.switch_view(GitView::Changes, cx)),
        )
        .child("Changes");

    let is_history = current_view == GitView::History;
    let tab_history = h_flex()
        .id("tab-history")
        .flex_1()
        .py(px(5.))
        .rounded(radius)
        .justify_center()
        .text_xs()
        .font_weight(if is_history {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(if is_history { tab_active_fg } else { tab_fg })
        .bg(if is_history {
            tab_active
        } else {
            transparent_black()
        })
        .hover(move |s| {
            if is_history {
                s
            } else {
                s.bg(muted.opacity(0.6))
            }
        })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.switch_view(GitView::History, cx)),
        )
        .child("History");

    let is_branches = current_view == GitView::Branches;
    let tab_branches = h_flex()
        .id("tab-branches")
        .flex_1()
        .py(px(5.))
        .rounded(radius)
        .justify_center()
        .text_xs()
        .font_weight(if is_branches {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(if is_branches { tab_active_fg } else { tab_fg })
        .bg(if is_branches {
            tab_active
        } else {
            transparent_black()
        })
        .hover(move |s| {
            if is_branches {
                s
            } else {
                s.bg(muted.opacity(0.6))
            }
        })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.switch_view(GitView::Branches, cx)),
        )
        .child("Branches");

    let tabs = h_flex()
        .w_full()
        .p(px(3.))
        .bg(tab_seg)
        .rounded(radius)
        .gap_px()
        .child(tab_changes)
        .child(tab_history)
        .child(tab_branches);

    // ── Branch pill + sync row ────────────────────────────────────────────────
    let branch_pill = h_flex()
        .flex_1()
        .px(px(7.))
        .py(px(4.))
        .gap_1()
        .rounded(radius)
        .bg(muted.opacity(0.4))
        .border_1()
        .border_color(border)
        .items_center()
        .overflow_hidden()
        .child(
            Icon::new(IconName::GitBranch)
                .size(px(11.))
                .text_color(primary),
        )
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(foreground)
                .overflow_hidden()
                .child(current_branch),
        );

    let pull_btn = h_flex()
        .gap(px(2.))
        .items_center()
        .child(
            Button::new("pull")
                .icon(IconName::ArrowDown)
                .ghost()
                .compact()
                .tooltip(if behind > 0 {
                    format!("Pull {} commit(s)", behind)
                } else {
                    "Pull".to_string()
                })
                .on_click(cx.listener(|this, _, _, cx| this.pull(cx))),
        )
        .when(behind > 0, |this| {
            this.child(
                div()
                    .px(px(4.))
                    .py(px(1.))
                    .rounded_full()
                    .bg(primary.opacity(0.18))
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(primary)
                    .child(behind.to_string()),
            )
        });

    let push_btn = h_flex()
        .gap(px(2.))
        .items_center()
        .child(
            Button::new("push")
                .icon(IconName::ArrowUp)
                .ghost()
                .compact()
                .tooltip(if ahead > 0 {
                    format!("Push {} commit(s)", ahead)
                } else {
                    "Push".to_string()
                })
                .on_click(cx.listener(|this, _, _, cx| this.push(cx))),
        )
        .when(ahead > 0, |this| {
            this.child(
                div()
                    .px(px(4.))
                    .py(px(1.))
                    .rounded_full()
                    .bg(primary.opacity(0.18))
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(primary)
                    .child(ahead.to_string()),
            )
        });

    let sync_row = h_flex()
        .w_full()
        .gap_1()
        .items_center()
        .child(branch_pill)
        .child(
            Button::new("refresh")
                .icon(IconName::Refresh)
                .ghost()
                .compact()
                .tooltip("Refresh".to_string())
                .on_click(cx.listener(|this, _, _, cx| this.refresh_state(cx))),
        )
        .child(
            Button::new("fetch")
                .icon(IconName::ArrowRight)
                .ghost()
                .compact()
                .tooltip("Fetch from remote".to_string())
                .on_click(cx.listener(|this, _, _, cx| this.fetch(cx))),
        )
        .child(pull_btn)
        .child(push_btn);

    let mut toolbar = v_flex()
        .w_full()
        .px_2()
        .pt_2()
        .pb(px(7.))
        .gap_2()
        .bg(tab_bar)
        .border_b_1()
        .border_color(border)
        .child(tabs)
        .child(sync_row);

    // ── Auth credential prompt ────────────────────────────────────────────────
    if let Some(pending_op) = git_manager.pending_auth_op {
        toolbar = toolbar.child(
            v_flex()
                .w_full()
                .px_2()
                .py_2()
                .gap(px(6.))
                .rounded(radius)
                .bg(warning.opacity(0.08))
                .border_1()
                .border_color(warning.opacity(0.3))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(px(12.))
                                .text_color(warning),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(warning)
                                .child(format!(
                                    "Authentication required for {}",
                                    pending_op.label()
                                )),
                        ),
                )
                .child(TextInput::new(&git_manager.auth_username_input))
                .child(TextInput::new(&git_manager.auth_password_input))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("auth-retry")
                                .label(pending_op.label())
                                .primary()
                                .compact()
                                .on_click(cx.listener(|this, _, _, cx| this.retry_with_auth(cx))),
                        )
                        .child(
                            Button::new("auth-cancel")
                                .label("Cancel")
                                .ghost()
                                .compact()
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_auth(cx))),
                        ),
                ),
        );
    } else if let Some(err) = &git_manager.op_error {
        let err = err.clone();
        toolbar = toolbar.child(
            h_flex()
                .w_full()
                .px_2()
                .py(px(7.))
                .gap(px(6.))
                .items_center()
                .rounded(radius)
                .bg(danger.opacity(0.1))
                .border_1()
                .border_color(danger.opacity(0.25))
                .child(
                    Icon::new(IconName::CircleX)
                        .size(px(12.))
                        .text_color(danger),
                )
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(danger)
                        .overflow_hidden()
                        .child(err),
                )
                .child(
                    Button::new("dismiss-err")
                        .icon(IconName::Close)
                        .ghost()
                        .compact()
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_error(cx))),
                ),
        );
    }

    toolbar
}
