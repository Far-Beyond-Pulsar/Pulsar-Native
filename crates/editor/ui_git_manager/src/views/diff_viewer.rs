//! Side-by-side diff viewer for the Git manager.
//! Uses a single shared scroll container so both sides scroll together.
//! Monaco-style: shaded spacers where lines don't align.

use crate::{DiffLineKind, DiffResult, DiffSegment, GitManager, DIFF_LINE_ROW_H};
use gpui::*;
use ui::{ActiveTheme as _, h_flex, v_flex};

/// A single aligned row for side-by-side display.
/// Left and right lists always have the same length — spacers fill gaps.
#[derive(Clone, Debug)]
struct AlignedRow {
    left_num: Option<usize>,
    left_kind: DiffLineKind,
    left_content: String,
    left_is_spacer: bool,
    right_num: Option<usize>,
    right_kind: DiffLineKind,
    right_content: String,
    right_is_spacer: bool,
}

/// Compute aligned rows from a DiffResult.
/// Each output row represents one visual line: both sides have the same count,
/// with spacers inserted for additions (left spacer) and deletions (right spacer).
fn compute_aligned_rows(diff: &DiffResult) -> Vec<AlignedRow> {
    let mut rows = Vec::new();

    for segment in &diff.segments {
        let lines = match segment {
            DiffSegment::Hunk(l) => l,
            DiffSegment::Collapsed { lines: l, .. } => l,
        };

        for line in lines {
            match line.kind {
                DiffLineKind::Context => {
                    rows.push(AlignedRow {
                        left_num: line.old_line_num,
                        left_kind: DiffLineKind::Context,
                        left_content: line.content.clone(),
                        left_is_spacer: false,
                        right_num: line.new_line_num,
                        right_kind: DiffLineKind::Context,
                        right_content: line.content.clone(),
                        right_is_spacer: false,
                    });
                }
                DiffLineKind::Removed => {
                    rows.push(AlignedRow {
                        left_num: line.old_line_num,
                        left_kind: DiffLineKind::Removed,
                        left_content: line.content.clone(),
                        left_is_spacer: false,
                        right_num: None,
                        right_kind: DiffLineKind::Context,
                        right_content: String::new(),
                        right_is_spacer: true,
                    });
                }
                DiffLineKind::Added => {
                    rows.push(AlignedRow {
                        left_num: None,
                        left_kind: DiffLineKind::Context,
                        left_content: String::new(),
                        left_is_spacer: true,
                        right_num: line.new_line_num,
                        right_kind: DiffLineKind::Added,
                        right_content: line.content.clone(),
                        right_is_spacer: false,
                    });
                }
            }
        }
    }

    rows
}

/// Render the two-column header (BEFORE / AFTER) — not scrollable.
fn render_header(border: Hsla, rem_bg: Hsla, rem_fg: Hsla, add_bg: Hsla, add_fg: Hsla) -> impl IntoElement {
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

/// Render a single aligned row as two side-by-side columns.
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
    let (left_bg, left_text_color): (Hsla, Hsla) = match row.left_kind {
        DiffLineKind::Removed => (rem_bg, rem_fg),
        _ => {
            if row.left_is_spacer {
                (spacer_bg, line_num_color)
            } else {
                (gpui::transparent_black(), foreground)
            }
        }
    };

    let (right_bg, right_text_color): (Hsla, Hsla) = match row.right_kind {
        DiffLineKind::Added => (add_bg, add_fg),
        _ => {
            if row.right_is_spacer {
                (spacer_bg, line_num_color)
            } else {
                (gpui::transparent_black(), foreground)
            }
        }
    };

    h_flex()
        .w_full()
        .h(px(DIFF_LINE_ROW_H))
        .flex_shrink_0()
        .font(mono_font.clone())
        .text_size(px(13.))
        .child(
            // Left column
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
                        .child(
                            row.left_num
                                .map(|n| n.to_string())
                                .unwrap_or_default(),
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
                        .text_color(left_text_color)
                        .child(row.left_content.clone()),
                ),
        )
        // Vertical divider between columns
        .child(div().w(px(1.)).h_full().bg(border))
        .child(
            // Right column
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
                            row.right_num
                                .map(|n| n.to_string())
                                .unwrap_or_default(),
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
                        .text_color(right_text_color)
                        .child(row.right_content.clone()),
                ),
        )
}

/// Render a side-by-side diff panel.
/// Both sides share a single scroll container so they scroll in perfect sync.
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

    // Single shared scroll container — both columns scroll together
    let body = div()
        .id("side-by-side-diff-scroll")
        .flex_1()
        .overflow_y_scroll()
        .overflow_x_hidden()
        .child(
            v_flex().children(rows.iter().map(|row| {
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
