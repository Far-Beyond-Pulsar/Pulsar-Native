use gpui::prelude::*;
use gpui::*;
use std::path::PathBuf;

use crate::components::FileManagerDrawer;
use crate::utils::tree::FolderNode;

impl FileManagerDrawer {
    pub fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(old) = self.renaming_item.clone() else {
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
            self.renaming_item = Some(old);
            cx.notify();
            return;
        }
        match self.rename_path(&old, &name, cx) {
            Ok(_) => self.renaming_item = None,
            Err(error) => {
                tracing::error!("Rename failed: {}", error);
                self.renaming_item = Some(old);
            }
        }
        cx.notify();
    }

    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming_item = None;
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(ref p) = self.project_path {
            self.set_folder_tree(FolderNode::from_path(p));
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
        s.focus(w, cx);
    });
    w.dispatch_action(Box::new(ui::input::SelectAll), cx);
    cx.notify();
}
