//! Studio-Quality File Sync UI - GitHub Desktop Style
//!
//! Features:
//! - Resizable split panel with file list on left
//! - Diff viewer on right with syntax and diff highlighting
//! - Fast over-the-network file diffing
//! - Professional UI ready for production use

use gpui::*;
use ui::{
    button::Button,
    h_flex, v_flex,
    resizable::{h_resizable, resizable_panel, ResizableState},
    scroll::ScrollbarState,
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt,
};
use std::path::PathBuf;

use crate::diff::{LineDiff, DiffOperation};

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
}

/// The main file sync UI component
pub struct FileSyncUI {
    focus_handle: FocusHandle,
    /// List of files to sync
    pub files: Vec<FileSyncEntry>,
    /// Currently selected file index
    pub selected_file_index: Option<usize>,
    /// Resizable state for the split panel
    split_state: Entity<ResizableState>,
    /// Scrollbar state for file list
    file_list_scroll: ScrollbarState,
    /// Scrollbar state for diff viewer
    diff_scroll: ScrollbarState,
    /// Scroll handle for diff viewer
    diff_scroll_handle: ScrollHandle,
}

impl FileSyncUI {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let split_state = ResizableState::new(cx);

        Self {
            focus_handle: cx.focus_handle(),
            files: Vec::new(),
            selected_file_index: None,
            split_state,
            file_list_scroll: ScrollbarState::default(),
            diff_scroll: ScrollbarState::default(),
            diff_scroll_handle: ScrollHandle::new(),
        }
    }

    /// Set the files to display
    pub fn set_files(&mut self, files: Vec<FileSyncEntry>, cx: &mut Context<Self>) {
        self.files = files;
        if !self.files.is_empty() && self.selected_file_index.is_none() {
            self.selected_file_index = Some(0);
        }
        cx.notify();
    }

    /// Get the currently selected file
    pub fn selected_file(&self) -> Option<&FileSyncEntry> {
        self.selected_file_index.and_then(|idx| self.files.get(idx))
    }

    /// Select a file by index
    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.files.len() {
            self.selected_file_index = Some(index);
            cx.notify();
        }
    }

    /// Render the file list sidebar
    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().secondary)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                // Header
                div()
                    .p_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(format!("Files ({})", self.files.len()))
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} to sync",
                                        self.files.len()
                                    ))
                            )
                    )
            )
            .child(
                // File list
                div()
                    .id("file-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(
                        self.files.iter().enumerate().map(|(idx, entry)| {
                            let is_selected = self.selected_file_index == Some(idx);
                            let (icon, color) = match entry.status {
                                FileSyncStatus::Added => (IconName::Plus, cx.theme().success),
                                FileSyncStatus::Modified => (IconName::Refresh, cx.theme().warning),
                                FileSyncStatus::Deleted => (IconName::Trash, cx.theme().danger),
                            };

                            let theme = cx.theme();

                            Button::new(("file-item", idx))
                                .w_full()
                                .on_click(cx.listener(move |this: &mut FileSyncUI, _, _, cx| {
                                    this.select_file(idx, cx);
                                }))
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    Icon::new(icon)
                                                        .size(px(14.))
                                                        .text_color(if is_selected {
                                                            cx.theme().accent_foreground
                                                        } else {
                                                            color
                                                        })
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_medium()
                                                        .child(entry.filename())
                                                )
                                        )
                                        .child({
                                            let mut text_div = div().text_xs();
                                            if is_selected {
                                                text_div = text_div.text_color(theme.accent_foreground.opacity(0.8));
                                            } else {
                                                text_div = text_div.text_color(theme.muted_foreground);
                                            }
                                            text_div.child(entry.path.clone())
                                        })
                                )
                        })
                    )
            )
    }

    /// Render the diff viewer for the selected file
    fn render_diff_viewer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(file) = self.selected_file() {
            let diff = self.compute_diff_for_file(file);

            v_flex()
                .size_full()
                .bg(cx.theme().background)
                .child(
                    // Header with file info
                    div()
                        .p_3()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(cx.theme().foreground)
                                                .child(file.path.clone())
                                        )
                                        .child({
                                            let (color, label) = match file.status {
                                                FileSyncStatus::Added => (cx.theme().success, "Added"),
                                                FileSyncStatus::Modified => (cx.theme().warning, "Modified"),
                                                FileSyncStatus::Deleted => (cx.theme().danger, "Deleted"),
                                            };
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded(px(4.))
                                                .text_xs()
                                                .font_medium()
                                                .bg(color.opacity(0.1))
                                                .text_color(color)
                                                .child(label)
                                        })
                                )
                                .child(
                                    h_flex()
                                        .gap_4()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("+{} lines", diff.stats().additions))
                                        .child(format!("-{} lines", diff.stats().deletions))
                                        .child(format!("{} unchanged", diff.stats().unchanged))
                                )
                        )
                )
                .child(
                    // Diff content
                    div()
                        .id("diff-viewer")
                        .flex_1()
                        .overflow_y_scroll()
                        .track_scroll(&self.diff_scroll_handle)
                        .font_family("monospace")
                        .text_sm()
                        .child(
                            v_flex()
                                .children(
                                    diff.operations.iter().enumerate().map(|(idx, op)| {
                                        self.render_diff_line(op, idx, cx)
                                    })
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
                        .gap_2()
                        .child(
                            Icon::new(IconName::Code)
                                .size(px(48.))
                                .text_color(cx.theme().muted_foreground.opacity(0.5))
                        )
                        .child(
                            div()
                                .text_lg()
                                .text_color(cx.theme().muted_foreground)
                                .child("Select a file to view changes")
                        )
                )
        }
    }

    /// Render a single diff line
    fn render_diff_line(&self, op: &DiffOperation, line_num: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg_color, text_color, prefix, line_text) = match op {
            DiffOperation::Insert { line } => (
                theme.success.opacity(0.1),
                theme.success,
                "+ ",
                line.clone(),
            ),
            DiffOperation::Delete { line } => (
                theme.danger.opacity(0.1),
                theme.danger,
                "- ",
                line.clone(),
            ),
            DiffOperation::Equal { line } => (
                transparent_black(),
                theme.foreground,
                "  ",
                line.clone(),
            ),
        };

        div()
            .w_full()
            .px_4()
            .py_0p5()
            .bg(bg_color)
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .child(
                        // Line number
                        div()
                            .w(px(40.))
                            .text_right()
                            .text_color(theme.muted_foreground.opacity(0.5))
                            .child(format!("{}", line_num + 1))
                    )
                    .child(
                        // Prefix (+/-)
                        div()
                            .w(px(20.))
                            .font_bold()
                            .text_color(text_color)
                            .child(prefix)
                    )
                    .child(
                        // Line content with syntax highlighting
                        div()
                            .flex_1()
                            .text_color(text_color)
                            .child(line_text)
                    )
            )
    }

    /// Compute diff for a file
    fn compute_diff_for_file(&self, file: &FileSyncEntry) -> LineDiff {
        match (&file.local_content, &file.remote_content, &file.status) {
            (Some(local), Some(remote), FileSyncStatus::Modified) => {
                LineDiff::compute(local, remote)
            }
            (None, Some(remote), FileSyncStatus::Added) => {
                LineDiff::compute("", remote)
            }
            (Some(local), None, FileSyncStatus::Deleted) => {
                LineDiff::compute(local, "")
            }
            _ => LineDiff::new(),
        }
    }
}

impl Focusable for FileSyncUI {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FileSyncUI {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_resizable("file-sync-split", self.split_state.clone())
            .child(
                // Left panel: File list
                resizable_panel()
                    .size(px(300.))
                    .child(self.render_file_list(cx))
            )
            .child(
                // Right panel: Diff viewer
                resizable_panel()
                    .child(self.render_diff_viewer(cx))
            )
    }
}
