use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{
    button::Button,
    h_flex,
    v_flex, ActiveTheme as _, Icon, IconName, StyledExt,
};

use crate::screen::MultiplayerWindow;

pub fn render_file_sync_tab(
    this: &MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    if this.file_sync_in_progress {
        tracing::debug!(
            "RENDER: Showing sync progress - message: {:?}, percent: {:?}",
            this.sync_progress_message,
            this.sync_progress_percent
        );

        return v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child("Synchronizing files..."),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        this.sync_progress_message
                            .clone()
                            .unwrap_or_else(|| "Starting...".to_string()),
                    ),
            )
            .child(
                div()
                    .w(px(400.))
                    .h(px(12.))
                    .rounded(px(6.))
                    .bg(cx.theme().secondary)
                    .child(
                        div()
                            .w(px(400. * this.sync_progress_percent.unwrap_or(0.0)))
                            .h_full()
                            .rounded(px(6.))
                            .bg(cx.theme().accent),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{}%",
                        (this.sync_progress_percent.unwrap_or(0.0) * 100.0) as u32
                    )),
            )
            .into_any_element();
    }

    if let Some((diff, host_peer_id)) = &this.pending_file_sync {
        tracing::debug!("Rendering FileSync tab with pending diff");

        v_flex()
            .size_full()
            .child(
                div()
                    .p_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(format!(
                                                "Synchronize with {}",
                                                host_peer_id
                                            )),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_3()
                                            .text_xs()
                                            .child(div().text_color(cx.theme().success).child(
                                                format!("+{} added", diff.files_to_add.len()),
                                            ))
                                            .child(div().text_color(cx.theme().warning).child(
                                                format!(
                                                    "~{} modified",
                                                    diff.files_to_update.len()
                                                ),
                                            ))
                                            .child(div().text_color(cx.theme().danger).child(
                                                format!(
                                                    "-{} deleted",
                                                    diff.files_to_delete.len()
                                                ),
                                            )),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Button::new("sync-cancel").label("Cancel").on_click(
                                        cx.listener(|this, _, _window, cx| {
                                            crate::handlers::on_sync_cancel(this, cx);
                                        }),
                                    ))
                                    .child(
                                        Button::new("sync-approve")
                                            .label("Sync Files")
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                crate::handlers::on_sync_approve(this, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .child(
                div().flex_1().child(this.diff_viewer.clone()),
            )
            .into_any_element()
    } else {
        tracing::debug!("Rendering FileSync tab - no pending sync");
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                Icon::new(IconName::Check)
                    .size(px(48.))
                    .text_color(cx.theme().success),
            )
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child("Files are synchronized"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Your project is up to date with the session"),
            )
            .child(
                div().pt_8().child(
                    Button::new("simulate-diff-dev")
                        .label("Simulate Diff (Dev)")
                        .icon(IconName::Code)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.simulate_diff_for_dev(window, cx);
                        })),
                ),
            )
            .into_any_element()
    }
}
