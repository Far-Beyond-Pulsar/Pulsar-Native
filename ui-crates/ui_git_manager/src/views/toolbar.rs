//! Compact toolbar: tab selector + branch/sync row

use crate::{GitManager, GitView, PendingAuthOp};
use gpui::*;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    input::TextInput,
};

pub fn render_toolbar(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let current_view = git_manager.current_view;
    let current_branch = repo_state.current_branch.clone();
    let ahead = repo_state.ahead;
    let behind = repo_state.behind;
    drop(repo_state);

    let border = cx.theme().border;
    let muted_fg = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    let warning = cx.theme().warning;

    // Tab row
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

    // Branch + sync row — always show Fetch; show Push/Pull with counts when known
    let sync_row = h_flex()
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
        )
        .child(
            Button::new("fetch")
                .icon(IconName::ArrowRight)
                .ghost()
                .compact()
                .tooltip("Fetch from remote".to_string())
                .on_click(cx.listener(|this, _, _, cx| this.fetch(cx))),
        )
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
        );

    let mut toolbar = v_flex()
        .w_full()
        .px_2()
        .pt_2()
        .pb_1()
        .gap_1()
        .border_b_1()
        .border_color(border)
        .child(tabs)
        .child(sync_row);

    // Auth credential prompt — shown when a remote op returns 401
    if let Some(pending_op) = git_manager.pending_auth_op {
        toolbar = toolbar.child(
            v_flex()
                .w_full()
                .px_1()
                .py_1()
                .gap_1()
                .rounded(cx.theme().radius)
                .bg(warning.opacity(0.08))
                .border_1()
                .border_color(warning.opacity(0.3))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(warning)
                        .child(format!("Authentication required for {}", pending_op.label())),
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
        // Non-auth errors: plain dismissible banner
        let err = err.clone();
        toolbar = toolbar.child(
            h_flex()
                .w_full()
                .px_2()
                .py_1()
                .gap_1()
                .items_center()
                .rounded(cx.theme().radius)
                .bg(danger.opacity(0.12))
                .child(Icon::new(IconName::CircleX).size(px(11.)).text_color(danger))
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
