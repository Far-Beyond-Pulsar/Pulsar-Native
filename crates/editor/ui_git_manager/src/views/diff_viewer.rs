//! Side-by-side diff viewer for the Git manager.
//! Uses `similar::DiffOp` for proper line-number–based alignment so that
//! deletions and insertions at the same position are paired on one row.
//! Both sides share a single scroll container for synced scrolling.

use crate::{DiffResult, GitManager, DIFF_LINE_ROW_H};
use gpui::*;
use similar::{DiffOp, TextDiff};
use ui::{ActiveTheme as _, h_flex, v_flex};

/// Visual style for one side of an aligned row.
#[derive(Clone, Copy, PartialEq)]
enum CellStyle {
    Normal,
    Removed,
    Added,
    Spacer,
}

/// One visual row in the side-by-side view.
struct AlignedRow {
    left_line: String,
    left_num: Option<usize>,
    left_style: CellStyle,
    right_line: String,
    right_num: Option<usize>,
    right_style: CellStyle,
}

/// Build aligned rows directly from the full old/new text using `similar::DiffOp`.
/// This properly pairs Replace operations so deleted+inserted content
/// at the same position appears on a single row.
fn compute_aligned_rows(diff: &DiffResult) -> Vec<AlignedRow> {
    let old = diff.old_lines.as_slice();
    let new = diff.new_lines.as_slice();
    let old_text = old.join("\n");
    let new_text = new.join("\n");
    let text_diff = TextDiff::from_lines(&old_text, &new_text);
    let mut rows: Vec<AlignedRow> = Vec::new();

    for op in text_diff.ops() {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for i in 0..*len {
                    rows.push(AlignedRow {
                        left_line: old[old_index + i].clone(),
                        left_num: Some(old_index + i + 1),
                        left_style: CellStyle::Normal,
                        right_line: new[new_index + i].clone(),
                        right_num: Some(new_index + i + 1),
                        right_style: CellStyle::Normal,
                    });
                }
            }
            DiffOp::Delete {
                old_index,
                old_len,
                ..
            } => {
                for i in 0..*old_len {
                    rows.push(AlignedRow {
                        left_line: old[old_index + i].clone(),
                        left_num: Some(old_index + i + 1),
                        left_style: CellStyle::Removed,
                        right_line: String::new(),
                        right_num: None,
                        right_style: CellStyle::Spacer,
                    });
                }
            }
            DiffOp::Insert {
                new_index,
                new_len,
                ..
            } => {
                for i in 0..*new_len {
                    rows.push(AlignedRow {
                        left_line: String::new(),
                        left_num: None,
                        left_style: CellStyle::Spacer,
                        right_line: new[new_index + i].clone(),
                        right_num: Some(new_index + i + 1),
                        right_style: CellStyle::Added,
                    });
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let max = (*old_len).max(*new_len);
                for i in 0..max {
                    let left = if i < *old_len {
                        old[old_index + i].clone()
                    } else {
                        String::new()
                    };
                    let right = if i < *new_len {
                        new[new_index + i].clone()
                    } else {
                        String::new()
                    };
                    rows.push(AlignedRow {
                        left_line: left,
                        left_num: if i < *old_len {
                            Some(old_index + i + 1)
                        } else {
                            None
                        },
                        left_style: if i < *old_len {
                            CellStyle::Removed
                        } else {
                            CellStyle::Spacer
                        },
                        right_line: right,
                        right_num: if i < *new_len {
                            Some(new_index + i + 1)
                        } else {
                            None
                        },
                        right_style: if i < *new_len {
                            CellStyle::Added
                        } else {
                            CellStyle::Spacer
                        },
                    });
                }
            }
        }
    }

    rows
}

/// Render the two-column header (BEFORE / AFTER) — not scrollable.
fn render_header(
    border: Hsla,
    rem_bg: Hsla,
    rem_fg: Hsla,
    add_bg: Hsla,
    add_fg: Hsla,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .flex_shrink_0()
        .child(
            h_flex()
                .flex_1()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(border)
                .bg(rem_bg)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rem_fg)
                        .child("BEFORE"),
                ),
        )
        .child(
            h_flex()
                .flex_1()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(border)
                .bg(add_bg)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(add_fg)
                        .child("AFTER"),
                ),
        )
}

/// Render a single row.
fn render_row(
    row: &AlignedRow,
    mono_font: &Font,
    rem_bg: Hsla,
    rem_fg: Hsla,
    add_bg: Hsla,
    add_fg: Hsla,
    spacer_bg: Hsla,
    line_num_color: Hsla,
    foreground: Hsla,
    border: Hsla,
) -> impl IntoElement {
    let (left_bg, left_text) = match row.left_style {
        CellStyle::Removed => (rem_bg, rem_fg),
        CellStyle::Spacer => (spacer_bg, line_num_color),
        _ => (gpui::transparent_black(), foreground),
    };
    let (right_bg, right_text) = match row.right_style {
        CellStyle::Added => (add_bg, add_fg),
        CellStyle::Spacer => (spacer_bg, line_num_color),
        _ => (gpui::transparent_black(), foreground),
    };

    h_flex()
        .w_full()
        .h(px(DIFF_LINE_ROW_H))
        .flex_shrink_0()
        .font(mono_font.clone())
        .text_size(px(13.))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(left_bg)
                .child(
                    div()
                        .w(px(50.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_end()
                        .pr_2()
                        .text_xs()
                        .text_color(line_num_color)
                        .child(row.left_num.map(|n| n.to_string()).unwrap_or_default()),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .pl_2()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(left_text)
                        .child(row.left_line.clone()),
                ),
        )
        .child(div().w(px(1.)).h_full().bg(border))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(right_bg)
                .child(
                    div()
                        .w(px(50.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_end()
                        .pr_2()
                        .text_xs()
                        .text_color(line_num_color)
                        .child(
                            row.right_num.map(|n| n.to_string()).unwrap_or_default(),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .pl_2()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(right_text)
                        .child(row.right_line.clone()),
                ),
        )
}

/// Render a side-by-side diff panel.
/// `is_commit = false` → uses `file_diff`, `is_commit = true` → uses `commit_file_diff`.
pub fn render_side_by_side_diff(
    git_manager: &mut GitManager,
    is_commit: bool,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let diff = if is_commit {
        &git_manager.commit_file_diff
    } else {
        &git_manager.file_diff
    };

    let Some(diff) = diff else {
        return div().flex_1().into_any_element();
    };

    let rows = compute_aligned_rows(diff);
    if rows.is_empty() {
        return div().flex_1().into_any_element();
    }

    let theme = cx.theme();
    let mono_font = Font {
        family: "JetBrains Mono".to_string().into(),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(vec!["monospace".to_string()])),
    };

    let add_bg: Hsla = rgba(0x00cc0033).into();
    let rem_bg: Hsla = rgba(0xff222233).into();
    let add_fg: Hsla = rgba(0x22dd22ff).into();
    let rem_fg: Hsla = rgba(0xff5555ff).into();
    let spacer_bg: Hsla = rgba(0x00000033).into();
    let line_num_color = theme.muted_foreground;
    let foreground = theme.foreground;
    let border = theme.border;

    let header = render_header(border, rem_bg, rem_fg, add_bg, add_fg);

    let body = div()
        .id("side-by-side-diff-scroll")
        .flex_1()
        .overflow_y_scroll()
        .overflow_x_hidden()
        .child(
            v_flex()
                .w_full()
                .children(rows.iter().map(|row| {
                    render_row(
                        row,
                        &mono_font,
                        rem_bg,
                        rem_fg,
                        add_bg,
                        add_fg,
                        spacer_bg,
                        line_num_color,
                        foreground,
                        border,
                    )
                })),
        );

    v_flex()
        .size_full()
        .overflow_hidden()
        .child(header)
        .child(body)
        .into_any_element()
}
