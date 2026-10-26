//! The diff panel: file header, mode toggle and the read-only Mockaco views.

use super::common::placeholder;
use crate::{DiffViewMode, GitManager};
use gpui::*;
use ui::{
    ActiveTheme as _, IconName,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

pub(crate) fn render_diff_panel(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (border, muted_fg, foreground, success, danger) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.success,
        theme.danger,
    );
    let split = gm.diff_mode == DiffViewMode::SideBySide;

    let Some(target) = gm.target.clone() else {
        return placeholder(IconName::Code, "Select a file to see its changes", cx)
            .into_any_element();
    };
    let path = target.path().to_string();
    let (name, directory) = match path.rsplit_once('/') {
        Some((dir, name)) => (name.to_string(), dir.to_string()),
        None => (path.clone(), String::new()),
    };

    let header = h_flex()
        .h(px(32.))
        .flex_shrink_0()
        .px_3()
        .gap_2()
        .items_center()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(10.))
                .text_color(muted_fg)
                .child(directory),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(success)
                .child(format!("+{}", gm.diff.additions)),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(danger)
                .child(format!("-{}", gm.diff.deletions)),
        )
        .child(
            Button::new("toggle-diff-mode")
                .icon(if split {
                    IconName::AlignLeft
                } else {
                    IconName::ViewColumns2
                })
                .ghost()
                .compact()
                .tooltip(if split {
                    "Unified view"
                } else {
                    "Side-by-side view"
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_diff_view_mode(cx))),
        );

    let body: AnyElement = if let Some(message) = gm.diff_message.clone() {
        placeholder(IconName::Code, message, cx).into_any_element()
    } else if gm.diff_loading {
        placeholder(IconName::Loader, "Loading diff…", cx).into_any_element()
    } else if split {
        let caption = |text: &'static str| {
            div()
                .h(px(20.))
                .px_3()
                .flex_shrink_0()
                .text_size(px(10.))
                .text_color(muted_fg)
                .border_b_1()
                .border_color(border)
                .child(text)
        };
        h_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(caption("Before"))
                    .child(div().flex_1().min_h_0().child(gm.diff.left.clone())),
            )
            .child(div().w(px(1.)).h_full().bg(border))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(caption("After"))
                    .child(div().flex_1().min_h_0().child(gm.diff.right.clone())),
            )
            .into_any_element()
    } else {
        div()
            .size_full()
            .overflow_hidden()
            .child(gm.diff.unified.clone())
            .into_any_element()
    };

    v_flex()
        .size_full()
        .child(header)
        .child(div().flex_1().min_h_0().overflow_hidden().child(body))
        .into_any_element()
}
