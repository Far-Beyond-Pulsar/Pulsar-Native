//! Diff viewer component for file synchronization
//! Uses proper line-by-line diffing like the problems drawer

use gpui::{*, prelude::*};
use ui::{v_flex, h_flex, scroll::ScrollbarAxis, StyledExt, IconName, ActiveTheme};
use std::path::PathBuf;
use similar::{ChangeTag, TextDiff};

/// Represents the type of a diff line for rendering
#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffLineType {
    Unchanged,
    Added,
    Deleted,
    Spacer, // Empty line to align both sides
}

/// Compute aligned diff lines for side-by-side display
/// Returns (left_lines, right_lines) where each line is (line_number, type, content)
fn compute_aligned_diff(before: &str, after: &str) -> (Vec<(Option<usize>, DiffLineType, String)>, Vec<(Option<usize>, DiffLineType, String)>) {
    let diff = TextDiff::from_lines(before, after);
    
    let mut left_lines: Vec<(Option<usize>, DiffLineType, String)> = Vec::new();
    let mut right_lines: Vec<(Option<usize>, DiffLineType, String)> = Vec::new();
    
    let mut left_line_num = 1usize;
    let mut right_line_num = 1usize;
    
    for change in diff.iter_all_changes() {
        let content = change.value().trim_end_matches('\n').to_string();
        
        match change.tag() {
            ChangeTag::Equal => {
                // Unchanged line appears on both sides
                left_lines.push((Some(left_line_num), DiffLineType::Unchanged, content.clone()));
                right_lines.push((Some(right_line_num), DiffLineType::Unchanged, content));
                left_line_num += 1;
                right_line_num += 1;
            }
            ChangeTag::Delete => {
                // Deleted line only appears on left side, add spacer on right
                left_lines.push((Some(left_line_num), DiffLineType::Deleted, content));
                right_lines.push((None, DiffLineType::Spacer, "".to_string()));
                left_line_num += 1;
            }
            ChangeTag::Insert => {
                // Added line only appears on right side, add spacer on left
                left_lines.push((None, DiffLineType::Spacer, "".to_string()));
                right_lines.push((Some(right_line_num), DiffLineType::Added, content));
                right_line_num += 1;
            }
        }
    }
    
    (left_lines, right_lines)
}

/// Represents a file with before/after content for diff viewing
#[derive(Clone, Debug)]
pub struct DiffFileEntry {
    pub path: String,
    pub before_content: String,
    pub after_content: String,
}

/// Diff viewer component that displays side-by-side file diffs
pub struct DiffViewer {
    focus_handle: FocusHandle,
    /// List of files to show diffs for
    diff_files: Vec<DiffFileEntry>,
    /// Project root path
    project_root: Option<PathBuf>,
    /// Currently selected file index
    selected_file_index: Option<usize>,
}

impl DiffViewer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            diff_files: Vec::new(),
            project_root: None,
            selected_file_index: None,
        }
    }

    /// Enter diff mode with a list of files
    pub fn enter_diff_mode(&mut self, diff_files: Vec<DiffFileEntry>, project_root: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        self.diff_files = diff_files;
        self.project_root = Some(project_root);
        self.selected_file_index = if !self.diff_files.is_empty() { Some(0) } else { None };
        cx.notify();
    }

    /// Update the after content for a specific file
    pub fn update_diff_file_after_content(&mut self, file_path: &str, content: String, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.diff_files.iter().position(|f| f.path == file_path) {
            self.diff_files[index].after_content = content;
            cx.notify();
        }
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_file_index;
        
        v_flex()
            .w(px(250.0))
            .h_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(cx.theme().foreground)
                    .child(format!("Files ({})", self.diff_files.len()))
            )
            .child(
                div()
                    .id("file-list-scroll")
                    .flex_1()
                    .scrollable(ScrollbarAxis::Vertical)
                    .child(
                        v_flex()
                            .w_full()
                            .children(
                                self.diff_files.iter().enumerate().map(|(index, file)| {
                                    let is_selected = selected == Some(index);
                                    
                                    div()
                                        .w_full()
                                        .px_3()
                                        .py_2()
                                        .cursor_pointer()
                                        .when(is_selected, |d| d.bg(cx.theme().accent.opacity(0.1)))
                                        .hover(|d| d.bg(cx.theme().secondary))
                                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                            this.selected_file_index = Some(index);
                                            cx.notify();
                                        }))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(if is_selected {
                                                    cx.theme().accent
                                                } else {
                                                    cx.theme().foreground
                                                })
                                                .child(file.path.clone())
                                        )
                                })
                            )
                    )
            )
    }

    fn render_diff_view(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(index) = self.selected_file_index {
            if let Some(file) = self.diff_files.get(index) {
                // Compute the aligned diff lines
                let (left_lines, right_lines) = compute_aligned_diff(&file.before_content, &file.after_content);
                
                // Colors for diff highlighting
                let deleted_bg = Hsla { h: 0.0, s: 0.4, l: 0.15, a: 1.0 };
                let deleted_text = Hsla { h: 0.0, s: 0.7, l: 0.7, a: 1.0 };
                let added_bg = Hsla { h: 120.0, s: 0.4, l: 0.15, a: 1.0 };
                let added_text = Hsla { h: 120.0, s: 0.7, l: 0.7, a: 1.0 };
                let spacer_bg = Hsla { h: 0.0, s: 0.0, l: 0.12, a: 1.0 };
                let unchanged_bg = cx.theme().sidebar;
                let unchanged_text = cx.theme().foreground;
                let line_num_color = cx.theme().muted_foreground;
                
                // Build left side (before) lines
                let mut left_container = v_flex().w_full();
                for (line_num, line_type, content) in &left_lines {
                    let (bg, text_color) = match line_type {
                        DiffLineType::Deleted => (deleted_bg, deleted_text),
                        DiffLineType::Spacer => (spacer_bg, line_num_color),
                        DiffLineType::Unchanged => (unchanged_bg, unchanged_text),
                        _ => (unchanged_bg, unchanged_text),
                    };
                    
                    left_container = left_container.child(
                        h_flex()
                            .w_full()
                            .h(px(20.0))
                            .bg(bg)
                            .child(
                                div()
                                    .w(px(50.0))
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .pr_2()
                                    .text_xs()
                                    .font_family("JetBrains Mono")
                                    .text_color(line_num_color)
                                    .child(if *line_type == DiffLineType::Spacer { 
                                        "".to_string() 
                                    } else { 
                                        line_num.map(|n| n.to_string()).unwrap_or_default()
                                    })
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
                                    .child(content.clone())
                            )
                    );
                }
                
                // Build right side (after) lines
                let mut right_container = v_flex().w_full();
                for (line_num, line_type, content) in &right_lines {
                    let (bg, text_color) = match line_type {
                        DiffLineType::Added => (added_bg, added_text),
                        DiffLineType::Spacer => (spacer_bg, line_num_color),
                        DiffLineType::Unchanged => (unchanged_bg, unchanged_text),
                        _ => (unchanged_bg, unchanged_text),
                    };
                    
                    right_container = right_container.child(
                        h_flex()
                            .w_full()
                            .h(px(20.0))
                            .bg(bg)
                            .child(
                                div()
                                    .w(px(50.0))
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .pr_2()
                                    .text_xs()
                                    .font_family("JetBrains Mono")
                                    .text_color(line_num_color)
                                    .child(if *line_type == DiffLineType::Spacer { 
                                        "".to_string() 
                                    } else { 
                                        line_num.map(|n| n.to_string()).unwrap_or_default()
                                    })
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
                                    .child(content.clone())
                            )
                    );
                }
                
                return v_flex()
                    .size_full()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().sidebar)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        ui::Icon::new(IconName::Folder)
                                            .size_4()
                                            .text_color(cx.theme().accent)
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(cx.theme().foreground)
                                            .child(file.path.clone())
                                    )
                            )
                    )
                    .child(
                        h_flex()
                            .h_full()
                            .min_h_0()
                            .overflow_hidden()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .h_full()
                                    .w_1_2()
                                    .border_r_1()
                                    .border_color(cx.theme().border)
                                    .child(
                                        div()
                                            .px_3()
                                            .py_2()
                                            .border_b_1()
                                            .border_color(cx.theme().border)
                                            .bg(Hsla { h: 0.0, s: 0.4, l: 0.12, a: 1.0 })
                                            .child(
                                                h_flex()
                                                    .gap_1p5()
                                                    .items_center()
                                                    .child(
                                                        ui::Icon::new(IconName::Close)
                                                            .size_3()
                                                            .text_color(Hsla { h: 0.0, s: 0.8, l: 0.6, a: 1.0 })
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(gpui::FontWeight::BOLD)
                                                            .text_color(Hsla { h: 0.0, s: 0.7, l: 0.65, a: 1.0 })
                                                            .child("BEFORE")
                                                    )
                                            )
                                    )
                                    .child(
                                        div()
                                            .id("left-diff-scroll")
                                            .flex_1()
                                            .overflow_y_scroll()
                                            .overflow_x_hidden()
                                            .child(left_container)
                                    )
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .h_full()
                                    .w_1_2()
                                    .child(
                                        div()
                                            .px_3()
                                            .py_2()
                                            .border_b_1()
                                            .border_color(cx.theme().border)
                                            .bg(Hsla { h: 120.0, s: 0.4, l: 0.12, a: 1.0 })
                                            .child(
                                                h_flex()
                                                    .gap_1p5()
                                                    .items_center()
                                                    .child(
                                                        ui::Icon::new(IconName::Check)
                                                            .size_3()
                                                            .text_color(Hsla { h: 120.0, s: 0.8, l: 0.5, a: 1.0 })
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(gpui::FontWeight::BOLD)
                                                            .text_color(Hsla { h: 120.0, s: 0.7, l: 0.55, a: 1.0 })
                                                            .child("AFTER")
                                                    )
                                            )
                                    )
                                    .child(
                                        div()
                                            .id("right-diff-scroll")
                                            .flex_1()
                                            .overflow_y_scroll()
                                            .overflow_x_hidden()
                                            .child(right_container)
                                    )
                            )
                    )
                    .into_any_element();
            }
        }
        
        // No file selected - show empty state
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select a file to view diff")
            )
            .into_any_element()
    }
}

impl Focusable for DiffViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DiffViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_file_list(cx))
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .child(self.render_diff_view(window, cx))
            )
    }
}
