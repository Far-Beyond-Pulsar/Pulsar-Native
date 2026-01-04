//! Diff viewer component for file synchronization
//! Uses the same editor approach as the problems drawer

use gpui::{*, prelude::*};
use ui::{v_flex, h_flex, input::{InputState, TextInput}, scroll::ScrollbarAxis, StyledExt, IconName, ActiveTheme};
use std::path::PathBuf;
use std::collections::HashMap;

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
    /// Cache of InputState entities for before/after content, keyed by file index
    editor_cache: HashMap<usize, (Entity<InputState>, Entity<InputState>)>,
}

impl DiffViewer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            diff_files: Vec::new(),
            project_root: None,
            selected_file_index: None,
            editor_cache: HashMap::new(),
        }
    }

    /// Enter diff mode with a list of files
    pub fn enter_diff_mode(&mut self, diff_files: Vec<DiffFileEntry>, project_root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_files = diff_files;
        self.project_root = Some(project_root);
        self.selected_file_index = if !self.diff_files.is_empty() { Some(0) } else { None };
        self.editor_cache.clear();
        
        // Pre-create editors for all files (collect indices first to avoid borrow issues)
        let indices_and_files: Vec<(usize, DiffFileEntry)> = self.diff_files.iter()
            .enumerate()
            .map(|(i, f)| (i, f.clone()))
            .collect();
            
        for (index, file) in indices_and_files {
            self.get_or_create_editors(index, &file, window, cx);
        }
        
        cx.notify();
    }

    /// Update the after content for a specific file
    pub fn update_diff_file_after_content(&mut self, file_path: &str, content: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.diff_files.iter().position(|f| f.path == file_path) {
            self.diff_files[index].after_content = content.clone();
            
            // Update the after editor if it exists in cache
            if let Some((_, after_editor)) = self.editor_cache.get(&index) {
                after_editor.update(cx, |editor, cx| {
                    editor.set_value(&content, window, cx);
                });
            }
            
            cx.notify();
        }
    }

    fn get_or_create_editors(&mut self, index: usize, file: &DiffFileEntry, window: &mut Window, cx: &mut Context<Self>) -> (Entity<InputState>, Entity<InputState>) {
        if let Some(editors) = self.editor_cache.get(&index) {
            return editors.clone();
        }

        // Determine language from file extension
        let language = std::path::Path::new(&file.path)
            .extension()
            .and_then(|ext| ext.to_str())
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

        let before_editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor(language)
                .soft_wrap(false)
                .rows(20);
            state.set_value(&file.before_content, window, cx);
            state
        });

        let after_editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor(language)
                .soft_wrap(false)
                .rows(20);
            state.set_value(&file.after_content, window, cx);
            state
        });

        self.editor_cache.insert(index, (before_editor.clone(), after_editor.clone()));
        (before_editor, after_editor)
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

    fn render_diff_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(index) = self.selected_file_index {
            if let Some(file) = self.diff_files.get(index).cloned() {
                let (before_editor, after_editor) = self.get_or_create_editors(index, &file, window, cx);
                
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
                            .flex_1()
                            .overflow_hidden()
                            .child(
                                v_flex()
                                    .w_1_2()
                                    .h_full()
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
                                            .flex_1()
                                            .overflow_hidden()
                                            .child(
                                                TextInput::new(&before_editor)
                                                    .w_full()
                                                    .h_full()
                                            )
                                    )
                            )
                            .child(
                                v_flex()
                                    .w_1_2()
                                    .h_full()
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
                                            .flex_1()
                                            .overflow_hidden()
                                            .child(
                                                TextInput::new(&after_editor)
                                                    .w_full()
                                                    .h_full()
                                            )
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
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_diff_view(window, cx))
            )
    }
}
