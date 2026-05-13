//! File diff preview panel (right side) — virtualized diff viewer

use crate::{DiffLineKind, DiffRow, GitManager, DIFF_COLLAPSE_ROW_H, DIFF_LINE_ROW_H};
use gpui::*;
use std::rc::Rc;
use ui::{
    ActiveTheme as _, Icon, IconName, h_flex,
    scroll::{Scrollbar, ScrollbarAxis},
    v_flex, v_virtual_list,
};

/// Render the right-side diff panel for the Changes / Branches view
pub fn render_file_panel(
    git_manager: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let border = cx.theme().border;
    let muted_fg = cx.theme().muted_foreground;
    let danger = cx.theme().danger;

    let header_label = git_manager
        .selected_file
        .clone()
        .unwrap_or_else(|| "File Preview".to_string());

    let header = h_flex()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted_fg)
                .overflow_hidden()
                .child(header_label),
        );

    let body: AnyElement = match &git_manager.selected_file {
        None => empty_placeholder(muted_fg, "Select a file to view its diff").into_any_element(),

        Some(_) => match (&git_manager.file_diff, &git_manager.file_diff_error) {
            (Some(_), _) => render_diff_virtual(git_manager, false, cx).into_any_element(),
            (None, Some(err)) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::CircleX)
                        .size(px(28.))
                        .text_color(danger),
                )
                .child(div().text_sm().text_color(danger).child(err.clone()))
                .into_any_element(),
            (None, None) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(div().text_xs().text_color(muted_fg).child("Loading…"))
                .into_any_element(),
        },
    };

    v_flex().size_full().child(header).child(body)
}

fn empty_placeholder(muted_fg: Hsla, msg: &'static str) -> impl IntoElement {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .child(Icon::new(IconName::Code).size(px(32.)).text_color(muted_fg))
        .child(div().text_xs().text_color(muted_fg).child(msg))
}

/// Render the virtualized diff view.
/// `is_commit = false` → uses `file_diff_rows`, `file_diff_scroll`, `file_diff_scrollbar`.
/// `is_commit = true`  → uses `commit_diff_rows`, `commit_diff_scroll`, `commit_diff_scrollbar`.
pub fn render_diff_virtual(
    git_manager: &mut GitManager,
    is_commit: bool,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let mono_font = Font {
        family: "JetBrains Mono".to_string().into(),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(vec!["monospace".to_string()])),
    };

    let rows = if is_commit { &git_manager.commit_diff_rows } else { &git_manager.file_diff_rows };
    let item_sizes: Vec<Size<Pixels>> = rows
        .iter()
        .map(|r| match r {
            DiffRow::Line { .. } => size(px(0.0), px(DIFF_LINE_ROW_H)),
            DiffRow::Collapse { .. } => size(px(0.0), px(DIFF_COLLAPSE_ROW_H)),
        })
        .collect();
    let rows_len = rows.len();
    let item_sizes = Rc::new(item_sizes);

    let scroll_handle = if is_commit {
        git_manager.commit_diff_scroll.clone()
    } else {
        git_manager.file_diff_scroll.clone()
    };
    let scrollbar_state = if is_commit {
        git_manager.commit_diff_scrollbar.clone()
    } else {
        git_manager.file_diff_scrollbar.clone()
    };

    let list_id = if is_commit { "commit-diff-vlist" } else { "file-diff-vlist" };
    let entity = cx.entity().clone();

    if rows_len == 0 {
        return div()
            .flex_1()
            .min_h_0()
            .into_any_element()
            .into_any();
    }

    let list = v_virtual_list(entity, list_id, item_sizes, move |gm: &mut GitManager, range, _window, cx| {
        let add_bg: Hsla = rgba(0x00cc0033).into();
        let rem_bg: Hsla = rgba(0xff222233).into();
        let add_fg: Hsla = rgba(0x22dd22ff).into();
        let rem_fg: Hsla = rgba(0xff5555ff).into();
        let col_bg: Hsla = rgba(0x00000044).into();

        let muted_fg = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        let rows = if is_commit { &gm.commit_diff_rows } else { &gm.file_diff_rows };

        range.map(|i| -> AnyElement {
            match &rows[i] {
                DiffRow::Line { kind, content, line_num_str } => {
                    let (bg, gutter_char, gutter_color) = match kind {
                        DiffLineKind::Added   => (Some(add_bg), "+", add_fg),
                        DiffLineKind::Removed => (Some(rem_bg), "-", rem_fg),
                        DiffLineKind::Context => (None,         " ", muted_fg),
                    };
                    let line_num_str = line_num_str.clone();
                    let content = content.clone();

                    let mut row = h_flex()
                        .w_full()
                        .h(px(DIFF_LINE_ROW_H))
                        .overflow_hidden()
                        .flex_shrink_0()
                        .font(mono_font.clone())
                        .text_size(px(13.));
                    if let Some(bg) = bg {
                        row = row.bg(bg);
                    }
                    row
                        .child(
                            div()
                                .w(px(48.))
                                .px_1()
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_color(muted_fg)
                                .child(line_num_str),
                        )
                        .child(
                            div()
                                .w(px(16.))
                                .flex_shrink_0()
                                .text_color(gutter_color)
                                .font_weight(FontWeight::BOLD)
                                .child(gutter_char),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(foreground)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(content),
                        )
                        .into_any_element()
                }
                DiffRow::Collapse { region_idx, count } => {
                    let idx = *region_idx;
                    let count = *count;
                    h_flex()
                        .w_full()
                        .h(px(DIFF_COLLAPSE_ROW_H))
                        .px_4()
                        .bg(col_bg)
                        .border_y_1()
                        .border_color(border)
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.7))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                if is_commit {
                                    this.expand_commit_diff_region(idx, cx);
                                } else {
                                    this.expand_file_diff_region(idx, cx);
                                }
                            }),
                        )
                        .font(mono_font.clone())
                        .text_size(px(12.))
                        .text_color(muted_fg)
                        .child(div().child("↕"))
                        .child(div().child(format!("{} unchanged lines", count)))
                        .child(div().child("↕"))
                        .into_any_element()
                }
            }
        }).collect()
    })
    .track_scroll(&scroll_handle);

    div()
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .child(list)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(&scrollbar_state, &scroll_handle)),
        )
        .into_any_element()
        .into_any()
}
