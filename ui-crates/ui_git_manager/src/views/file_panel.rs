//! File diff preview panel (right side) — Monaco-style collapsible diff viewer

use crate::{GitManager, DiffResult, DiffSegment, DiffLineKind};
use gpui::*;
use ui::{h_flex, v_flex, Icon, IconName, ActiveTheme as _};

/// Render the right-side diff panel for the Changes / Branches view
pub fn render_file_panel(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let border     = cx.theme().border;
    let muted_fg   = cx.theme().muted_foreground;
    let danger     = cx.theme().danger;

    let header_label = git_manager.selected_file.clone()
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
            (Some(diff), _) => {
                render_diff_segments(diff, &git_manager.file_diff_expanded, "file-diff", false, cx)
                    .into_any_element()
            }
            (None, Some(err)) => {
                v_flex()
                    .flex_1().items_center().justify_center().gap_2()
                    .child(Icon::new(IconName::CircleX).size(px(28.)).text_color(danger))
                    .child(div().text_sm().text_color(danger).child(err.clone()))
                    .into_any_element()
            }
            (None, None) => {
                v_flex().flex_1().items_center().justify_center()
                    .child(div().text_xs().text_color(muted_fg).child("Loading…"))
                    .into_any_element()
            }
        },
    };

    v_flex().size_full().child(header).child(body)
}

fn empty_placeholder(muted_fg: Hsla, msg: &'static str) -> impl IntoElement {
    v_flex()
        .flex_1().items_center().justify_center().gap_2()
        .child(Icon::new(IconName::Code).size(px(32.)).text_color(muted_fg))
        .child(div().text_xs().text_color(muted_fg).child(msg))
}

/// Shared diff segment renderer — used by both the file panel and commit detail panel.
/// `scroll_id` must be unique per panel instance.
/// `is_commit` controls which expand callback is fired.
pub fn render_diff_segments(
    diff: &DiffResult,
    expanded: &std::collections::HashSet<usize>,
    scroll_id: &'static str,
    is_commit: bool,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let theme         = cx.theme();
    let muted_fg      = theme.muted_foreground;
    let foreground    = theme.foreground;
    let border        = theme.border;
    let add_bg: Hsla  = rgba(0x00cc0033).into();
    let rem_bg: Hsla  = rgba(0xff222233).into();
    let add_fg: Hsla  = rgba(0x22dd22ff).into();
    let rem_fg: Hsla  = rgba(0xff5555ff).into();
    let col_bg: Hsla  = rgba(0x00000044).into(); // collapse bar bg

    let mono_font = Font {
        family: "JetBrains Mono".to_string().into(),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(vec!["monospace".to_string()])),
    };

    let mut rows: Vec<AnyElement> = Vec::new();

    for segment in &diff.segments {
        match segment {
            DiffSegment::Hunk(lines) => {
                for line in lines {
                    let (bg, gutter_char, gutter_color) = match line.kind {
                        DiffLineKind::Added   => (Some(add_bg), "+", add_fg),
                        DiffLineKind::Removed => (Some(rem_bg), "-", rem_fg),
                        DiffLineKind::Context => (None,         " ", muted_fg),
                    };
                    let line_num_text = line.new_line_num
                        .or(line.old_line_num)
                        .map(|n| format!("{:>5}", n))
                        .unwrap_or_else(|| "     ".to_string());
                    let content = line.content.clone();

                    let mut row = h_flex()
                        .w_full()
                        .py(px(1.))
                        .font(mono_font.clone())
                        .text_size(px(13.));
                    if let Some(bg) = bg {
                        row = row.bg(bg);
                    }

                    rows.push(
                        row
                            // line number column
                            .child(
                                div()
                                    .w(px(48.))
                                    .px_1()
                                    .flex_shrink_0()
                                    .text_color(muted_fg)
                                    .child(line_num_text),
                            )
                            // +/- gutter column
                            .child(
                                div()
                                    .w(px(16.))
                                    .flex_shrink_0()
                                    .text_color(gutter_color)
                                    .font_weight(FontWeight::BOLD)
                                    .child(gutter_char),
                            )
                            // content
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(foreground)
                                    .overflow_hidden()
                                    .child(content),
                            )
                            .into_any_element(),
                    );
                }
            }

            DiffSegment::Collapsed { lines, region_idx } => {
                let count = lines.len();
                let idx = *region_idx;

                if expanded.contains(&idx) {
                    // Show expanded lines
                    for line in lines {
                        let line_num_text = line.new_line_num
                            .or(line.old_line_num)
                            .map(|n| format!("{:>5}", n))
                            .unwrap_or_else(|| "     ".to_string());
                        let content = line.content.clone();
                        rows.push(
                            h_flex()
                                .w_full()
                                .py(px(1.))
                                .font(mono_font.clone())
                                .text_size(px(13.))
                                .child(div().w(px(48.)).px_1().flex_shrink_0().text_color(muted_fg).child(line_num_text))
                                .child(div().w(px(16.)).flex_shrink_0().child(" "))
                                .child(div().flex_1().text_color(foreground).overflow_hidden().child(content))
                                .into_any_element(),
                        );
                    }
                } else {
                    // Show collapse bar
                    let expand_cb = cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if is_commit {
                            this.expand_commit_diff_region(idx, cx);
                        } else {
                            this.expand_file_diff_region(idx, cx);
                        }
                    });

                    rows.push(
                        h_flex()
                            .w_full()
                            .py(px(3.))
                            .px_4()
                            .bg(col_bg)
                            .border_y_1()
                            .border_color(border)
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.7))
                            .on_mouse_down(MouseButton::Left, expand_cb)
                            .font(mono_font.clone())
                            .text_size(px(12.))
                            .text_color(muted_fg)
                            .child(div().child("↕"))
                            .child(div().child(format!("{} unchanged lines", count)))
                            .child(div().child("↕"))
                            .into_any_element(),
                    );
                }
            }
        }
    }

    div()
        .id(scroll_id)
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .w_full()
        .children(rows)
}
