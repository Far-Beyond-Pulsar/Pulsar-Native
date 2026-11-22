//! The details of an open code diagnostic: its quick fixes as side-by-side
//! diffs and a few lines of its file around the problem.

use gpui::{prelude::*, *};
use rust_i18n::t;
use ui::{
    h_flex,
    input::{InputState, TextInput},
    v_flex, ActiveTheme as _, IconName,
};

use crate::screen::ProblemsDrawer;
use crate::utils::filter::compute_aligned_diff;
use crate::utils::types::{Diagnostic, DiffLineType, Hint};

pub fn render_hint_diff(
    _drawer: &mut ProblemsDrawer,
    _diagnostic_index: usize,
    _hint_index: usize,
    hint: &Hint,
    _window: &mut Window,
    cx: &mut Context<ProblemsDrawer>,
) -> Div {
    if hint.before_content.is_none() && hint.after_content.is_none() {
        return v_flex()
            .gap_1()
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(format!("💡 {}", hint.message)),
            );
    }

    let before_content = hint.before_content.clone().unwrap_or_default();
    let after_content = hint.after_content.clone().unwrap_or_default();
    let (left_lines, right_lines) = compute_aligned_diff(&before_content, &after_content);

    let deleted_bg = Hsla {
        h: 0.0,
        s: 0.4,
        l: 0.15,
        a: 1.0,
    };
    let deleted_text = Hsla {
        h: 0.0,
        s: 0.7,
        l: 0.7,
        a: 1.0,
    };
    let added_bg = Hsla {
        h: 120.0,
        s: 0.4,
        l: 0.15,
        a: 1.0,
    };
    let added_text = Hsla {
        h: 120.0,
        s: 0.7,
        l: 0.7,
        a: 1.0,
    };
    let spacer_bg = Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.12,
        a: 1.0,
    };
    let unchanged_bg = cx.theme().sidebar;
    let unchanged_text = cx.theme().foreground;
    let line_num_color = cx.theme().muted_foreground;

    fn build_side(
        lines: &[(Option<usize>, DiffLineType, String)],
        deleted_bg: Hsla,
        deleted_text: Hsla,
        added_bg: Hsla,
        added_text: Hsla,
        spacer_bg: Hsla,
        unchanged_bg: Hsla,
        unchanged_text: Hsla,
        line_num_color: Hsla,
        is_left: bool,
    ) -> Div {
        let mut container = v_flex().w_full();
        for (line_num, line_type, content) in lines {
            let (bg, text_color) = match line_type {
                DiffLineType::Deleted if is_left => (deleted_bg, deleted_text),
                DiffLineType::Added if !is_left => (added_bg, added_text),
                DiffLineType::Spacer => (spacer_bg, line_num_color),
                _ => (unchanged_bg, unchanged_text),
            };
            container = container.child(
                h_flex()
                    .w_full()
                    .h(px(20.0))
                    .bg(bg)
                    .child(
                        div()
                            .w(px(40.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_end()
                            .pr_2()
                            .text_xs()
                            .font_family("JetBrains Mono")
                            .text_color(line_num_color)
                            .child(if *line_type == DiffLineType::Spacer {
                                String::new()
                            } else {
                                line_num.map(|n| n.to_string()).unwrap_or_default()
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .pl_2()
                            .text_xs()
                            .font_family("JetBrains Mono")
                            .text_color(text_color)
                            .overflow_x_hidden()
                            .child(content.clone()),
                    ),
            );
        }
        container
    }

    let left_container = build_side(
        &left_lines,
        deleted_bg,
        deleted_text,
        added_bg,
        added_text,
        spacer_bg,
        unchanged_bg,
        unchanged_text,
        line_num_color,
        true,
    );
    let right_container = build_side(
        &right_lines,
        deleted_bg,
        deleted_text,
        added_bg,
        added_text,
        spacer_bg,
        unchanged_bg,
        unchanged_text,
        line_num_color,
        false,
    );

    let total_lines = left_lines.len().max(right_lines.len());
    let content_height = (total_lines as f32 * 20.0).max(40.0);

    v_flex()
        .gap_0()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().accent.opacity(0.3))
        .bg(cx.theme().background.opacity(0.5))
        .overflow_hidden()
        .shadow_md()
        .child(
            div()
                .px_4()
                .py_2p5()
                .border_b_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().accent.opacity(0.05))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(ui::Icon::new(IconName::Info).size_4())
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .child(hint.message.clone()),
                        ),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .child(
                    div()
                        .w_1_2()
                        .min_w_0()
                        .px_3()
                        .py_1p5()
                        .border_r_1()
                        .border_b_1()
                        .border_color(cx.theme().border.opacity(0.6))
                        .bg(Hsla {
                            h: 0.0,
                            s: 0.4,
                            l: 0.12,
                            a: 1.0,
                        })
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(ui::Icon::new(IconName::Close).size_3().text_color(Hsla {
                                    h: 0.0,
                                    s: 0.8,
                                    l: 0.6,
                                    a: 1.0,
                                }))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(Hsla {
                                            h: 0.0,
                                            s: 0.7,
                                            l: 0.65,
                                            a: 1.0,
                                        })
                                        .child(t!("Problems.Before").to_string()),
                                ),
                        ),
                )
                .child(
                    div()
                        .w_1_2()
                        .min_w_0()
                        .px_3()
                        .py_1p5()
                        .border_b_1()
                        .border_color(cx.theme().border.opacity(0.6))
                        .bg(Hsla {
                            h: 120.0,
                            s: 0.4,
                            l: 0.12,
                            a: 1.0,
                        })
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(ui::Icon::new(IconName::Check).size_3().text_color(Hsla {
                                    h: 120.0,
                                    s: 0.8,
                                    l: 0.5,
                                    a: 1.0,
                                }))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(Hsla {
                                            h: 120.0,
                                            s: 0.7,
                                            l: 0.55,
                                            a: 1.0,
                                        })
                                        .child(t!("Problems.After").to_string()),
                                ),
                        ),
                ),
        )
        .child(
            div()
                .id("diff-scroll-container")
                .w_full()
                .h(px(content_height))
                .overflow_y_scroll()
                .overflow_x_hidden()
                .child(
                    h_flex()
                        .w_full()
                        .child(
                            div()
                                .w_1_2()
                                .min_w_0()
                                .overflow_hidden()
                                .border_r_1()
                                .border_color(cx.theme().border.opacity(0.4))
                                .child(left_container),
                        )
                        .child(
                            div()
                                .w_1_2()
                                .min_w_0()
                                .overflow_hidden()
                                .child(right_container),
                        ),
                ),
        )
}

pub fn render_file_preview(
    drawer: &mut ProblemsDrawer,
    diagnostic: &Diagnostic,
    window: &mut Window,
    cx: &mut Context<ProblemsDrawer>,
) -> Div {
    const CONTEXT_LINES: usize = 2;
    let key = (diagnostic.file_path.clone(), diagnostic.line);
    // Read the file once per (file, line), not on every frame.
    if !drawer.preview_inputs.contains_key(&key) {
        let path = std::path::Path::new(&diagnostic.file_path);
        let preview = engine_fs::virtual_fs::read_to_string(path)
            .ok()
            .and_then(|content| {
                let lines: Vec<&str> = content.lines().collect();
                let error_line = diagnostic.line.saturating_sub(1);
                (error_line < lines.len()).then(|| {
                    let start = error_line.saturating_sub(CONTEXT_LINES);
                    let end = (error_line + CONTEXT_LINES + 1).min(lines.len());
                    (lines[start..end].join("\n"), end - start)
                })
            })
            .map(|(text, rows)| {
                // A language's own grammar (its registration names it); the
                // languages with no plugin keep a small table.
                let language = pulsar_language::LanguageRegistry::current()
                    .for_path(path)
                    .map(|language| language.highlight_id)
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| {
                        match path.extension().and_then(|e| e.to_str()).unwrap_or_default() {
                            "js" => "javascript",
                            "py" => "python",
                            "toml" => "toml",
                            "json" => "json",
                            "md" => "markdown",
                            "html" => "html",
                            "css" => "css",
                            _ => "text",
                        }
                    });
                let state = cx.new(|cx| {
                    let mut state = InputState::new(window, cx)
                        .code_editor(language)
                        .soft_wrap(false)
                        .rows(rows);
                    state.set_value(&text, window, cx);
                    state
                });
                (state, rows)
            });
        drawer.preview_inputs.insert(key.clone(), preview);
    }
    let Some(Some((state, rows))) = drawer.preview_inputs.get(&key).cloned() else {
        return div();
    };
    div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar)
        .overflow_hidden()
        .child(
            TextInput::new(&state)
                .w_full()
                .h(px(rows as f32 * 20.0 + 16.0))
                .font_family("JetBrains Mono")
                .text_size(px(12.0))
                .border_0(),
        )
}
