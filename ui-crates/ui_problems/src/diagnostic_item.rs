//! Single diagnostic row rendering and view helpers.

use std::fs;
use std::sync::Arc;

use gpui::{prelude::*, *};
use rust_i18n::t;
use ui::Sizable as _;
use ui::StyledExt as _;
use ui::{
    button::Button,
    h_flex,
    indicator::Indicator,
    input::{InputState, TextInput},
    scroll::ScrollbarAxis,
    v_flex, ActiveTheme as _, IconName,
};

use crate::filter::{compute_aligned_diff, Diagnostic, DiagnosticSeverity, DiffLineType, Hint};
use crate::problems_drawer::ProblemsDrawer;

impl ProblemsDrawer {
    pub(crate) fn render_flat_view(
        &mut self,
        filtered_diagnostics: Vec<Diagnostic>,
        selected_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let drawer_entity = cx.entity().clone();

        let items: Vec<Div> = filtered_diagnostics
            .into_iter()
            .enumerate()
            .map(|(index, diagnostic)| {
                let is_selected = selected_index == Some(index);
                let drawer = drawer_entity.clone();
                let diag = diagnostic.clone();

                self.render_diagnostic_item(
                    index,
                    diagnostic,
                    is_selected,
                    move |_window, cx| {
                        drawer.update(cx, |drawer, cx| {
                            drawer.select_diagnostic(index, cx);
                            drawer.navigate_to_diagnostic(&diag, cx);
                        });
                    },
                    window,
                    cx,
                )
            })
            .collect();

        div()
            .id("problems-scroll-container")
            .size_full()
            .scrollable(ScrollbarAxis::Vertical)
            .child(v_flex().w_full().p_2().gap_2().children(items))
    }

    pub(crate) fn render_grouped_view(
        &mut self,
        selected_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let grouped = self.get_grouped_diagnostics();
        let mut files: Vec<_> = grouped.keys().cloned().collect();
        files.sort();

        let drawer_entity = cx.entity().clone();
        let mut global_index = 0;
        let mut file_groups: Vec<Div> = Vec::new();

        for file_path in files {
            let diagnostics = grouped.get(&file_path).unwrap();
            let file_error_count = diagnostics
                .iter()
                .filter(|d| matches!(d.severity, DiagnosticSeverity::Error))
                .count();
            let file_warning_count = diagnostics
                .iter()
                .filter(|d| matches!(d.severity, DiagnosticSeverity::Warning))
                .count();

            let display_path = self.get_display_path(&file_path);

            let items: Vec<Div> = diagnostics
                .iter()
                .map(|diagnostic| {
                    let is_selected = selected_index == Some(global_index);
                    let drawer = drawer_entity.clone();
                    let diag = diagnostic.clone();
                    let idx = global_index;
                    global_index += 1;

                    self.render_diagnostic_item(
                        idx,
                        diag.clone(),
                        is_selected,
                        move |_window, cx| {
                            drawer.update(cx, |drawer, cx| {
                                drawer.select_diagnostic(idx, cx);
                                drawer.navigate_to_diagnostic(&diag, cx);
                            });
                        },
                        window,
                        cx,
                    )
                })
                .collect();

            let file_group = v_flex()
                .w_full()
                .px_3()
                .child(
                    div()
                        .w_full()
                        .px_3()
                        .py_2()
                        .mb_2()
                        .rounded_md()
                        .bg(cx.theme().secondary.opacity(0.3))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.3))
                        .child(
                            h_flex()
                                .w_full()
                                .gap_3()
                                .items_center()
                                .child(
                                    ui::Icon::new(IconName::Folder)
                                        .size_4()
                                        .text_color(cx.theme().accent),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().foreground)
                                        .child(display_path.clone()),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .when(file_error_count > 0, |this| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(DiagnosticSeverity::Error
                                                        .color(cx)
                                                        .opacity(0.15))
                                                    .child(
                                                        ui::Icon::new(IconName::Close)
                                                            .size_3()
                                                            .text_color(
                                                                DiagnosticSeverity::Error.color(cx),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .text_color(
                                                                DiagnosticSeverity::Error.color(cx),
                                                            )
                                                            .child(file_error_count.to_string()),
                                                    ),
                                            )
                                        })
                                        .when(file_warning_count > 0, |this| {
                                            this.child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(DiagnosticSeverity::Warning
                                                        .color(cx)
                                                        .opacity(0.15))
                                                    .child(
                                                        ui::Icon::new(IconName::TriangleAlert)
                                                            .size_3()
                                                            .text_color(
                                                                DiagnosticSeverity::Warning
                                                                    .color(cx),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .text_color(
                                                                DiagnosticSeverity::Warning
                                                                    .color(cx),
                                                            )
                                                            .child(file_warning_count.to_string()),
                                                    ),
                                            )
                                        }),
                                ),
                        ),
                )
                .children(items);

            file_groups.push(file_group);
        }

        div()
            .id("problems-scroll-container-grouped")
            .size_full()
            .scrollable(ScrollbarAxis::Vertical)
            .child(v_flex().w_full().p_2().gap_2().children(file_groups))
    }

    pub(crate) fn render_diagnostic_item<F>(
        &mut self,
        diagnostic_index: usize,
        diagnostic: Diagnostic,
        is_selected: bool,
        on_click: F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        let on_click = Arc::new(on_click);

        let mut main = v_flex()
            .gap_2()
            .w_full()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .w_full()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                ui::Icon::new(diagnostic.severity.icon())
                                    .size_4()
                                    .text_color(diagnostic.severity.color(cx)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(diagnostic.severity.color(cx))
                                    .child(diagnostic.severity.label()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_family("monospace")
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{}:{}", diagnostic.line, diagnostic.column)),
                    )
                    .when_some(diagnostic.source.as_ref(), |this, source| {
                        this.child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .bg(cx.theme().border)
                                .text_xs()
                                .font_family("monospace")
                                .text_color(cx.theme().muted_foreground)
                                .child(source.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .line_height(rems(1.4))
                    .child(diagnostic.message.clone()),
            );

        if diagnostic.loading_actions {
            main = main.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .mt_2()
                    .p_2()
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .child(
                        Indicator::new()
                            .with_size(ui::Size::Small)
                            .color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("Problems.Loading").to_string()),
                    ),
            );
        }

        tracing::debug!(
            "🎨 Rendering diagnostic {}: hints={}, loading={}",
            diagnostic_index,
            diagnostic.hints.len(),
            diagnostic.loading_actions
        );

        if !diagnostic.hints.is_empty() && !diagnostic.loading_actions {
            let mut hints_container = v_flex().gap_2().w_full().mt_2().child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("Problems.SuggestedFixes").to_string()),
            );

            for (hint_index, hint) in diagnostic.hints.iter().enumerate() {
                let hint_el = self.render_hint_diff(diagnostic_index, hint_index, hint, window, cx);
                hints_container = hints_container.child(hint_el);
            }
            main = main.child(hints_container);
        }

        if !diagnostic.subitems.is_empty() {
            let mut subitems_container = v_flex().gap_1().w_full().child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("Problems.Related").to_string()),
            );

            for sub in &diagnostic.subitems {
                subitems_container = subitems_container.child(
                    div().pl_4().py_1().child(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        ui::Icon::new(sub.severity.icon())
                                            .size_3()
                                            .text_color(sub.severity.color(cx)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family("monospace")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{}:{}", sub.line, sub.column)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(sub.message.clone()),
                            ),
                    ),
                );
            }
            main = main.child(subitems_container);
        }

        div().w_full().px_3().py_2().child(
            div()
                .w_full()
                .px_4()
                .py_3()
                .rounded_lg()
                .border_1()
                .border_color(if is_selected {
                    cx.theme().accent
                } else {
                    cx.theme().border.opacity(0.5)
                })
                .bg(if is_selected {
                    cx.theme().accent.opacity(0.08)
                } else {
                    cx.theme().sidebar.opacity(0.5)
                })
                .shadow_sm()
                .when(is_selected, |this| {
                    this.border_l_3().border_color(cx.theme().accent)
                })
                .hover(|this| {
                    this.bg(cx.theme().secondary.opacity(0.7))
                        .border_color(cx.theme().accent.opacity(0.5))
                })
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Left, move |_, _window, cx| {
                    on_click(_window, cx);
                })
                .child(main),
        )
    }

    pub(crate) fn render_hint_diff(
        &mut self,
        _diagnostic_index: usize,
        hint_index: usize,
        hint: &Hint,
        _window: &mut Window,
        cx: &mut Context<Self>,
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
                            .child(
                                ui::Icon::new(IconName::Info)
                                    .size_4()
                                    .text_color(cx.theme().accent),
                            )
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
                                    .child(ui::Icon::new(IconName::Close).size_3().text_color(
                                        Hsla {
                                            h: 0.0,
                                            s: 0.8,
                                            l: 0.6,
                                            a: 1.0,
                                        },
                                    ))
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
                                    .child(ui::Icon::new(IconName::Check).size_3().text_color(
                                        Hsla {
                                            h: 120.0,
                                            s: 0.8,
                                            l: 0.5,
                                            a: 1.0,
                                        },
                                    ))
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

    pub(crate) fn render_file_preview(
        &mut self,
        diagnostic: &Diagnostic,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let context_lines = 2;

        if let Ok(content) = fs::read_to_string(&diagnostic.file_path) {
            let lines: Vec<&str> = content.lines().collect();
            let error_line = diagnostic.line.saturating_sub(1);

            if error_line < lines.len() {
                let start_line = error_line.saturating_sub(context_lines);
                let end_line = (error_line + context_lines + 1).min(lines.len());
                let preview_content: String = lines[start_line..end_line].join("\n");

                let key = (diagnostic.file_path.clone(), diagnostic.line);
                let input_state = if let Some(existing) = self.preview_inputs.get(&key) {
                    existing.clone()
                } else {
                    let num_lines = end_line - start_line;
                    let language = std::path::Path::new(&diagnostic.file_path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|ext| match ext {
                            "rs" => "rust",
                            "js" => "javascript",
                            "ts" => "typescript",
                            "py" => "python",
                            "toml" => "toml",
                            "json" => "json",
                            "md" => "markdown",
                            "html" => "html",
                            "css" => "css",
                            _ => "text",
                        })
                        .unwrap_or("text");

                    let new_state = cx.new(|cx| {
                        let mut state = InputState::new(window, cx)
                            .code_editor(language)
                            .soft_wrap(false)
                            .rows(num_lines);
                        state.set_value(&preview_content, window, cx);
                        state
                    });
                    self.preview_inputs.insert(key, new_state.clone());
                    new_state
                };

                let calculated_height = (end_line - start_line) as f32 * 20.0 + 16.0;

                return div()
                    .w_full()
                    .mt_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .overflow_hidden()
                    .child(
                        TextInput::new(&input_state)
                            .w_full()
                            .h(px(calculated_height))
                            .font_family("JetBrains Mono")
                            .text_size(px(12.0))
                            .border_0(),
                    );
            }
        }
        div()
    }
}
