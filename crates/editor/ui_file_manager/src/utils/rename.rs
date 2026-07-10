use gpui::prelude::*;
use gpui::*;
use std::path::PathBuf;

use crate::components::FileManagerDrawer;
use crate::utils::tree::FolderNode;

impl FileManagerDrawer {
    pub fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(old) = self.renaming_item.take() else {
            return;
        };
        let name = self
            .rename_input_state
            .read(cx)
            .text()
            .to_string()
            .trim()
            .to_string();
        if name.is_empty() {
            cx.notify();
            return;
        }
        let on = old.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == on {
            cx.notify();
            return;
        }
        if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            tracing::error!("Invalid filename");
            cx.notify();
            return;
        }
        match self.operations.rename_item(&old, &name) {
            Ok(new) => {
                if let Err(e) = self.fs_metadata.rename_file(&old, &new) {
                    tracing::error!("rename_file: {}", e);
                }
                if self.selected_folder.as_ref() == Some(&old) {
                    self.selected_folder = Some(new.clone());
                }
                if self.selected_items.remove(&old) {
                    self.selected_items.insert(new);
                }
                if let Some(ref p) = self.project_path {
                    self.folder_tree = FolderNode::from_path(p);
                }
                self.mark_directory_cache_dirty();
            }
            Err(e) => tracing::error!("Rename failed: {}", e),
        }
        cx.notify();
    }

    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming_item = None;
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(ref p) = self.project_path {
            self.folder_tree = FolderNode::from_path(p);
        }
        self.mark_directory_cache_dirty();
        cx.notify();
    }
}

pub fn start_rename(
    d: &mut FileManagerDrawer,
    path: PathBuf,
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    d.renaming_item = Some(path);
    d.rename_input_state.update(cx, |s, cx| {
        let len = s.text().len();
        if len > 0 {
            s.replace_text_in_range(Some(0..len), "", w, cx);
        }
        s.replace_text_in_range(Some(0..0), &name, w, cx);
    });
    cx.notify();
}
