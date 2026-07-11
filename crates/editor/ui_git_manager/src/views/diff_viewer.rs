//! Side-by-side diff viewer for the Git manager.
//! Collapsed unchanged regions are shown as expandable bars (same as unified view).
//! Deletions immediately followed by insertions are paired on a single row.
//! Virtualized via `v_virtual_list` with a scrollbar overlay.

use crate::{
    DiffLineKind, DiffResult, DiffSegment, GitManager, DIFF_COLLAPSE_ROW_H, DIFF_LINE_ROW_H,
};
use gpui::*;
use std::collections::HashSet;
use std::rc::Rc;
use ui::{ActiveTheme as _, h_flex, scroll::Scrollbar, v_flex, v_virtual_list};

/// Visual style for one side of an aligned line row.
#[derive(Clone, Copy, PartialEq)]
pub enum CellStyle {
    Normal,
    Removed,
    Added,
    Spacer,
}

/// One visual row in the side-by-side virtual list.
#[derive(Clone)]
pub enum AlignedRow {
    /// A content row with left and right columns.
    Line {
        left_line: String,
        left_num: Option<usize>,
        left_style: CellStyle,
        right_line: String,
        right_num: Option<usize>,
        right_style: CellStyle,
    },
    /// A collapsed-region button row.
    Collapse { region_idx: usize, count: usize },
}

/// Build aligned rows from a `DiffResult`, mirroring the segment structure
/// of the unified view so collapsed unchanged regions are preserved.
///
/// Within hunks, adjacent Removed + Added lines are paired into a single row
/// (avoiding checkerboard gaps for simple replacements).
pub(crate) fn compute_aligned_rows(
    diff: &DiffResult,
    expanded: &HashSet<usize>,
) -> Vec<AlignedRow> {
    let mut rows = Vec::new();

    for segment in &diff.segments {
        match segment {
            DiffSegment::Hunk(lines) => {
                let mut i = 0;
                while i < lines.len() {
                    match lines[i].kind {
                        DiffLineKind::Context => {
                            rows.push(AlignedRow::Line {
                                left_line: lines[i].content.clone(),
                                left_num: lines[i].old_line_num,
                                left_style: CellStyle::Normal,
                                right_line: lines[i].content.clone(),
                                right_num: lines[i].new_line_num,
                                right_style: CellStyle::Normal,
                            });
                            i += 1;
                        }
                        DiffLineKind::Removed => {
                            // Pair with following Added if present → single Replace row
                            if i + 1 < lines.len()
                                && lines[i + 1].kind == DiffLineKind::Added
                            {
                                rows.push(AlignedRow::Line {
                                    left_line: lines[i].content.clone(),
                                    left_num: lines[i].old_line_num,
                                    left_style: CellStyle::Removed,
                                    right_line: lines[i + 1].content.clone(),
                                    right_num: lines[i + 1].new_line_num,
                                    right_style: CellStyle::Added,
                                });
                                i += 2;
                            } else {
                                rows.push(AlignedRow::Line {
                                    left_line: lines[i].content.clone(),
                                    left_num: lines[i].old_line_num,
                                    left_style: CellStyle::Removed,
                                    right_line: String::new(),
                                    right_num: None,
                                    right_style: CellStyle::Spacer,
                                });
                                i += 1;
                            }
                        }
                        DiffLineKind::Added => {
                            rows.push(AlignedRow::Line {
                                left_line: String::new(),
                                left_num: None,
                                left_style: CellStyle::Spacer,
                                right_line: lines[i].content.clone(),
                                right_num: lines[i].new_line_num,
                                right_style: CellStyle::Added,
                            });
                            i += 1;
                        }
                    }
                }
            }
            DiffSegment::Collapsed { lines, region_idx } => {
                if expanded.contains(region_idx) {
                    for line in lines {
                        rows.push(AlignedRow::Line {
                            left_line: line.content.clone(),
                            left_num: line.old_line_num,
                            left_style: CellStyle::Normal,
                            right_line: line.content.clone(),
                            right_num: line.new_line_num,
                            right_style: CellStyle::Normal,
                        });
                    }
                } else {
                    rows.push(AlignedRow::Collapse {
                        region_idx: *region_idx,
                        count: lines.len(),
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

/// Render a single aligned row into an element.
pub(crate) fn render_aligned_row(
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
    col_bg: Hsla,
    is_commit: bool,
    cx: &mut Context<GitManager>,
) -> AnyElement {
    match row {
        AlignedRow::Line {
            left_line,
            left_num,
            left_style,
            right_line,
            right_num,
            right_style,
        } => {
            let (l_bg, l_text) = match left_style {
                CellStyle::Removed => (rem_bg, rem_fg),
                CellStyle::Spacer => (spacer_bg, line_num_color),
                _ => (gpui::transparent_black(), foreground),
            };
            let (r_bg, r_text) = match right_style {
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
                        .bg(l_bg)
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
                                    left_num
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
                                .text_color(l_text)
                                .child(left_line.clone()),
                        ),
                )
                .child(div().w(px(1.)).h_full().bg(border))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .bg(r_bg)
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
                                    right_num
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
                                .text_color(r_text)
                                .child(right_line.clone()),
                        ),
                )
                .into_any_element()
        }
        AlignedRow::Collapse { region_idx, count } => {
            let idx = *region_idx;
            let cnt = *count;
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
                .text_color(line_num_color)
                .child(div().child("↕"))
                .child(div().child(format!("{} unchanged lines", cnt)))
                .child(div().child("↕"))
                .into_any_element()
        }
    }
}

/// Render a virtualized side-by-side diff panel with a scrollbar.
/// `is_commit = false` → uses `file_aligned_rows`, `is_commit = true` → uses `commit_aligned_rows`.
pub fn render_side_by_side_diff(
    git_manager: &mut GitManager,
    is_commit: bool,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let rows: &[AlignedRow] = if is_commit {
        &git_manager.commit_aligned_rows
    } else {
        &git_manager.file_aligned_rows
    };

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
    let col_bg: Hsla = rgba(0x00000044).into();
    let line_num_color = theme.muted_foreground;
    let foreground = theme.foreground;
    let border = theme.border;

    let header = render_header(border, rem_bg, rem_fg, add_bg, add_fg);

    let item_sizes: Vec<Size<Pixels>> = rows
        .iter()
        .map(|r| match r {
            AlignedRow::Line { .. } => size(px(0.0), px(DIFF_LINE_ROW_H)),
            AlignedRow::Collapse { .. } => size(px(0.0), px(DIFF_COLLAPSE_ROW_H)),
        })
        .collect();
    let item_sizes = Rc::new(item_sizes);

    let scroll_handle = if is_commit {
        git_manager.commit_align_scroll.clone()
    } else {
        git_manager.file_align_scroll.clone()
    };
    let scrollbar_state = if is_commit {
        git_manager.commit_align_scrollbar.clone()
    } else {
        git_manager.file_align_scrollbar.clone()
    };

    let list_id = if is_commit {
        "commit-align-vlist"
    } else {
        "file-align-vlist"
    };
    let entity = cx.entity().clone();

    let list = v_virtual_list(
        entity,
        list_id,
        item_sizes,
        move |gm: &mut GitManager, range, _window, cx| {
            let add_bg: Hsla = rgba(0x00cc0033).into();
            let rem_bg: Hsla = rgba(0xff222233).into();
            let add_fg: Hsla = rgba(0x22dd22ff).into();
            let rem_fg: Hsla = rgba(0xff5555ff).into();
            let spacer_bg: Hsla = rgba(0x00000033).into();
            let col_bg: Hsla = rgba(0x00000044).into();
            let line_num_color = cx.theme().muted_foreground;
            let foreground = cx.theme().foreground;
            let border = cx.theme().border;

            let rows = if is_commit {
                &gm.commit_aligned_rows
            } else {
                &gm.file_aligned_rows
            };

            range
                .map(|i| -> AnyElement {
                    render_aligned_row(
                        &rows[i],
                        &mono_font,
                        rem_bg,
                        rem_fg,
                        add_bg,
                        add_fg,
                        spacer_bg,
                        line_num_color,
                        foreground,
                        border,
                        col_bg,
                        is_commit,
                        cx,
                    )
                })
                .collect()
        },
    )
    .track_scroll(&scroll_handle);

    v_flex()
        .size_full()
        .overflow_hidden()
        .child(header)
        .child(
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
                        .child(Scrollbar::vertical(
                            &scrollbar_state,
                            &scroll_handle,
                        )),
                ),
        )
        .into_any_element()
}
