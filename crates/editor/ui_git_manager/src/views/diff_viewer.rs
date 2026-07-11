//! Side-by-side diff viewer for the Git manager

use crate::{DiffLineKind, DiffResult, DiffSegment, GitManager, DIFF_LINE_ROW_H};
use gpui::*;
use ui::{ActiveTheme as _, h_flex, v_flex};

/// Aligned line for side-by-side display
#[derive(Clone, Debug)]
struct AlignedLine {
    line_num: Option<usize>,
    kind: DiffLineKind,
    content: String,
    is_spacer: bool,
}

/// Compute aligned left/right line lists from a DiffResult for side-by-side rendering.
fn compute_aligned_diff(diff: &DiffResult) -> (Vec<AlignedLine>, Vec<AlignedLine>) {
    let mut left: Vec<AlignedLine> = Vec::new();
    let mut right: Vec<AlignedLine> = Vec::new();

    for segment in &diff.segments {
        let lines = match segment {
            DiffSegment::Hunk(l) => l,
            DiffSegment::Collapsed { lines: l, .. } => l,
        };

        for line in lines {
            match line.kind {
                DiffLineKind::Context => {
                    left.push(AlignedLine {
                        line_num: line.old_line_num,
                        kind: DiffLineKind::Context,
                        content: line.content.clone(),
                        is_spacer: false,
                    });
                    right.push(AlignedLine {
                        line_num: line.new_line_num,
                        kind: DiffLineKind::Context,
                        content: line.content.clone(),
                        is_spacer: false,
                    });
                }
                DiffLineKind::Removed => {
                    left.push(AlignedLine {
                        line_num: line.old_line_num,
                        kind: DiffLineKind::Removed,
                        content: line.content.clone(),
                        is_spacer: false,
                    });
                    right.push(AlignedLine {
                        line_num: None,
                        kind: DiffLineKind::Context,
                        content: String::new(),
                        is_spacer: true,
                    });
                }
                DiffLineKind::Added => {
                    left.push(AlignedLine {
                        line_num: None,
                        kind: DiffLineKind::Context,
                        content: String::new(),
                        is_spacer: true,
                    });
                    right.push(AlignedLine {
                        line_num: line.new_line_num,
                        kind: DiffLineKind::Added,
                        content: line.content.clone(),
                        is_spacer: false,
                    });
                }
            }
        }
    }

    (left, right)
}

/// Render a side-by-side diff panel using resizable panels.
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

    let (left_lines, right_lines) = compute_aligned_diff(diff);
    if left_lines.is_empty() {
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
    let spacer_bg: Hsla = rgba(0x00000022).into();
    let line_num_color = theme.muted_foreground;
    let foreground = theme.foreground;
    let border = theme.border;

    // ── Left (before) panel ─────────────────────────────────────────────
    let left_panel = v_flex()
        .size_full()
        .child(
            // Header
            h_flex()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(border)
                .bg(rem_bg)
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rem_fg)
                                .child("BEFORE"),
                        ),
                ),
        )
        .child(
            // Content
            div()
                .id("left-diff-scroll")
                .flex_1()
                .overflow_y_scroll()
                .overflow_x_hidden()
                .child(
                    v_flex().children(left_lines.iter().map(|line| {
                        let (bg, text_color): (Hsla, Hsla) = match line.kind {
                            DiffLineKind::Removed => (rem_bg, rem_fg),
                            _ => {
                                if line.is_spacer {
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
                            .bg(bg)
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
                                        line.line_num
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
                                    .text_color(text_color)
                                    .child(line.content.clone()),
                            )
                    })),
                ),
        );

    // ── Right (after) panel ────────────────────────────────────────────
    let right_panel = v_flex()
        .size_full()
        .child(
            // Header
            h_flex()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(border)
                .bg(add_bg)
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(add_fg)
                                .child("AFTER"),
                        ),
                ),
        )
        .child(
            // Content
            div()
                .id("right-diff-scroll")
                .flex_1()
                .overflow_y_scroll()
                .overflow_x_hidden()
                .child(
                    v_flex().children(right_lines.iter().map(|line| {
                        let (bg, text_color): (Hsla, Hsla) = match line.kind {
                            DiffLineKind::Added => (add_bg, add_fg),
                            _ => {
                                if line.is_spacer {
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
                            .bg(bg)
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
                                        line.line_num
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
                                    .text_color(text_color)
                                    .child(line.content.clone()),
                            )
                    })),
                ),
        );

    h_flex()
        .size_full()
        .overflow_hidden()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .border_r_1()
                .border_color(border)
                .child(left_panel),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(right_panel),
        )
        .into_any_element()
}
