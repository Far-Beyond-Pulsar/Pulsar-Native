//! Studio-Quality File Sync UI - Using Real Script Editor Components
//!
//! Features:
//! - Clean file list sidebar
//! - Side-by-side diff using actual TextInput editors
//! - Professional design matching script editor

use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
    input::{InputState, TextInput},
    resizable::{h_resizable, resizable_panel, ResizableState},
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt,
};
use std::path::PathBuf;

/// Status of a file in the sync
#[derive(Clone, Debug, PartialEq)]
pub enum FileSyncStatus {
    Added,
    Modified,
    Deleted,
}

/// A file entry in the sync list
#[derive(Clone, Debug)]
pub struct FileSyncEntry {
    pub path: String,
    pub status: FileSyncStatus,
    pub local_content: Option<String>,
    pub remote_content: Option<String>,
}

impl FileSyncEntry {
    pub fn new_added(path: String, remote_content: String) -> Self {
        Self {
            path,
            status: FileSyncStatus::Added,
            local_content: None,
            remote_content: Some(remote_content),
        }
    }

    pub fn new_modified(path: String, local_content: String, remote_content: String) -> Self {
        Self {
            path,
            status: FileSyncStatus::Modified,
            local_content: Some(local_content),
            remote_content: Some(remote_content),
        }
    }

    pub fn new_deleted(path: String, local_content: String) -> Self {
        Self {
            path,
            status: FileSyncStatus::Deleted,
            local_content: Some(local_content),
            remote_content: None,
        }
    }

    pub fn filename(&self) -> String {
        PathBuf::from(&self.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.path)
            .to_string()
    }

    pub fn extension(&self) -> Option<String> {
        PathBuf::from(&self.path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_string())
    }
}

/// The main file sync UI component
pub struct FileSyncUI {
    focus_handle: FocusHandle,
    /// List of files to sync
    pub files: Vec<FileSyncEntry>,
    /// Currently selected file index
    pub selected_file_index: Option<usize>,
    /// Resizable state for the file list / diff split
    file_list_split: Entity<ResizableState>,
    /// Resizable state for before/after diff split
    diff_split: Entity<ResizableState>,
    /// Editor states for before/after views
    before_editor: Entity<InputState>,
    after_editor: Entity<InputState>,
    /// Track whether editors have been initialized with content
    editors_initialized: bool,
}

impl FileSyncUI {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let file_list_split = ResizableState::new(cx);
        let diff_split = ResizableState::new(cx);

        let before_editor = cx.new(|cx| {
            InputState::new(window, cx).multi_line()
        });

        let after_editor = cx.new(|cx| {
            InputState::new(window, cx).multi_line()
        });

        Self {
            focus_handle: cx.focus_handle(),
            files: Vec::new(),
            selected_file_index: None,
            file_list_split,
            diff_split,
            before_editor,
            after_editor,
            editors_initialized: false,
        }
    }

    /// Set the files to display
    pub fn set_files(&mut self, files: Vec<FileSyncEntry>, cx: &mut Context<Self>) {
        self.files = files;
        // Auto-select first file if nothing is selected
        if !self.files.is_empty() && self.selected_file_index.is_none() {
            self.selected_file_index = Some(0);
        }
        // Reset initialization flag so editors will be loaded on next render
        self.editors_initialized = false;
        cx.notify();
    }

    /// Load the currently selected file into the editors
    pub fn load_selected_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(idx) = self.selected_file_index {
            // Extract content into local variables BEFORE borrowing self mutably
            let (local_content, remote_content) = if let Some(file) = self.files.get(idx) {
                let local = file.local_content.clone().unwrap_or_default();
                let remote = file.remote_content.clone().unwrap_or_default();
                (local, remote)
            } else {
                (String::new(), String::new())
            };

            // Update editors
            self.before_editor.update(cx, |editor, cx| {
                editor.set_value(&local_content, window, cx);
            });

            self.after_editor.update(cx, |editor, cx| {
                editor.set_value(&remote_content, window, cx);
            });
        }
    }

    /// Select a file by index and update editors
    fn select_file(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.files.len() {
            // Extract content into local variables BEFORE borrowing self mutably
            let (local_content, remote_content) = if let Some(file) = self.files.get(index) {
                let local = file.local_content.clone().unwrap_or_default();
                let remote = file.remote_content.clone().unwrap_or_default();
                (local, remote)
            } else {
                (String::new(), String::new())
            };

            self.selected_file_index = Some(index);

            // Update editors with the selected file's content
            self.before_editor.update(cx, |editor, cx| {
                editor.set_value(&local_content, window, cx);
            });

            self.after_editor.update(cx, |editor, cx| {
                editor.set_value(&remote_content, window, cx);
            });

            cx.notify();
        }
    }


    /// Render the toolbar
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (added, modified, deleted) = self.files.iter().fold((0, 0, 0), |(a, m, d), f| {
            match f.status {
                FileSyncStatus::Added => (a + 1, m, d),
                FileSyncStatus::Modified => (a, m + 1, d),
                FileSyncStatus::Deleted => (a, m, d + 1),
            }
        });

        h_flex()
            .w_full()
            .p_2()
            .bg(cx.theme().secondary)
            .border_b_1()
            .border_color(cx.theme().border)
            .justify_between()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child("File Changes")
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .bg(cx.theme().success.opacity(0.1))
                            .text_color(cx.theme().success)
                            .text_xs()
                            .font_medium()
                            .child(format!("+{}", added))
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .bg(cx.theme().warning.opacity(0.1))
                            .text_color(cx.theme().warning)
                            .text_xs()
                            .font_medium()
                            .child(format!("~{}", modified))
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .bg(cx.theme().danger.opacity(0.1))
                            .text_color(cx.theme().danger)
                            .text_xs()
                            .font_medium()
                            .child(format!("-{}", deleted))
                    )
            )
    }

    /// Render the clean file list sidebar
    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .id("file-list")
                    .size_full()
                    .overflow_y_scroll()
                    .children(
                        self.files.iter().enumerate().map(|(idx, entry)| {
                            let is_selected = self.selected_file_index == Some(idx);
                            let (status_icon, status_color) = match entry.status {
                                FileSyncStatus::Added => (IconName::Plus, cx.theme().success),
                                FileSyncStatus::Modified => (IconName::Refresh, cx.theme().warning),
                                FileSyncStatus::Deleted => (IconName::Trash, cx.theme().danger),
                            };

                            let mut base = div()
                                .w_full()
                                .px_3()
                                .py_2p5()
                                .cursor_pointer()
                                .border_b_1()
                                .border_color(cx.theme().border.opacity(0.3));

                            if is_selected {
                                base = base
                                    .bg(cx.theme().accent)
                                    .border_l_2()
                                    .border_color(cx.theme().accent);
                            } else {
                                base = base.hover(|s| s.bg(cx.theme().muted.opacity(0.08)));
                            }

                            Button::new(("file-item", idx))
                                .w_full()
                                .on_click(cx.listener(move |this: &mut FileSyncUI, _, window, cx| {
                                    this.select_file(idx, window, cx);
                                }))
                                .child(
                                    base.child(
                                        h_flex()
                                            .w_full()
                                            .items_start()
                                            .gap_2p5()
                                            .child(
                                                Icon::new(status_icon)
                                                    .size(px(16.))
                                                    .text_color(if is_selected {
                                                        cx.theme().accent_foreground
                                                    } else {
                                                        status_color
                                                    })
                                            )
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .gap_0p5()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_medium()
                                                            .text_color(if is_selected {
                                                                cx.theme().accent_foreground
                                                            } else {
                                                                cx.theme().foreground
                                                            })
                                                            .child(entry.filename())
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(if is_selected {
                                                                cx.theme().accent_foreground.opacity(0.7)
                                                            } else {
                                                                cx.theme().muted_foreground
                                                            })
                                                            .child(entry.path.clone())
                                                    )
                                            )
                                    )
                                )
                        })
                    )
            )
    }

    /// Render side-by-side diff using real editors
    fn render_diff_viewer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.selected_file_index.is_some() {
            v_flex()
                .size_full()
                .child(
                    // Diff header
                    h_flex()
                        .w_full()
                        .bg(cx.theme().secondary)
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .flex_1()
                                .px_3()
                                .py_2()
                                .border_r_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().danger)
                                        .child("BEFORE (Local)")
                                )
                        )
                        .child(
                            div()
                                .flex_1()
                                .px_3()
                                .py_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().success)
                                        .child("AFTER (Remote)")
                                )
                        )
                )
                .child(
                    // Side-by-side editors
                    div()
                        .flex_1()
                        .child(
                            h_resizable("diff-split", self.diff_split.clone())
                                .child(
                                    resizable_panel()
                                        .child(
                                            div()
                                                .size_full()
                                                .bg(cx.theme().background)
                                                .border_r_1()
                                                .border_color(cx.theme().border)
                                                .child(TextInput::new(&self.before_editor))
                                        )
                                )
                                .child(
                                    resizable_panel()
                                        .child(
                                            div()
                                                .size_full()
                                                .bg(cx.theme().background)
                                                .child(TextInput::new(&self.after_editor))
                                        )
                                )
                        )
                )
        } else {
            // No file selected
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .child(
                    v_flex()
                        .items_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::Code)
                                .size(px(56.))
                                .text_color(cx.theme().muted_foreground.opacity(0.3))
                        )
                        .child(
                            div()
                                .text_base()
                                .font_semibold()
                                .text_color(cx.theme().foreground.opacity(0.7))
                                .child("Select a file to view changes")
                        )
                )
        }
    }

    /// Render status bar
    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let file_info = if let Some(idx) = self.selected_file_index {
            if let Some(file) = self.files.get(idx) {
                format!("Viewing: {} ({})", file.filename(), match file.status {
                    FileSyncStatus::Added => "Added",
                    FileSyncStatus::Modified => "Modified",
                    FileSyncStatus::Deleted => "Deleted",
                })
            } else {
                "No file selected".to_string()
            }
        } else {
            "No file selected".to_string()
        };

        h_flex()
            .w_full()
            .px_4()
            .py_2()
            .bg(cx.theme().secondary)
            .border_t_1()
            .border_color(cx.theme().border)
            .justify_between()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} files to sync", self.files.len()))
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(file_info)
            )
    }
}

impl Focusable for FileSyncUI {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FileSyncUI {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Load the selected file content on first render
        if !self.editors_initialized && self.selected_file_index.is_some() {
            self.load_selected_file(window, cx);
            self.editors_initialized = true;
        }

        v_flex()
            .size_full()
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .child(
                        h_resizable("file-sync-split", self.file_list_split.clone())
                            .child(
                        resizable_panel()
                            .size(px(280.))
                            .child(self.render_file_list(cx))
                    )
                    .child(
                        resizable_panel()
                            .child(self.render_diff_viewer(cx))
                    )
                    )
            )
            .child(self.render_status_bar(cx))
    }
}
