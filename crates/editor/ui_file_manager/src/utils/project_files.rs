//! Path-based file operations, for the drawer's own handlers and for other
//! views of the project's files (the editor's left sidebar).
//!
//! The drawer's handlers act on its selection; these take the paths, so a view
//! with its own selection can use the same operations and the same clipboard.

use std::path::{Path, PathBuf};

use gpui::Context;

use crate::components::FileManagerDrawer;
use crate::utils::cloud_join;
use crate::utils::tree::FolderNode;
use crate::utils::types::FileItem;

/// Characters a file name cannot contain.
const INVALID_NAME_CHARS: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

fn remote(path: &Path) -> bool {
    engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(path)
}

fn exists(path: &Path) -> bool {
    if remote(path) {
        engine_fs::virtual_fs::exists(path).unwrap_or(false)
    } else {
        path.exists()
    }
}

/// `name` in `folder`, or `<stem> copy[ n].<ext>` there when `name` is taken.
pub fn free_name_in(folder: &Path, name: &str) -> PathBuf {
    let first = cloud_join(folder, name);
    if !exists(&first) {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    (1..)
        .map(|n| {
            let copy = if n == 1 {
                format!("{stem} copy")
            } else {
                format!("{stem} copy {n}")
            };
            let candidate = match ext {
                Some(ext) => format!("{copy}.{ext}"),
                None => copy,
            };
            cloud_join(folder, &candidate)
        })
        .find(|path| !exists(path))
        .expect("some copy name is free")
}

impl FileManagerDrawer {
    /// The files directly in `folder`, by name: not its subfolders, nor hidden
    /// files. A folder-based asset counts as a file. Read once and kept until
    /// something in the project changes on disk.
    pub fn files_in(&self, folder: &Path) -> Vec<FileItem> {
        if let Some(items) = self.tree_files.borrow().get(folder) {
            return items.clone();
        }
        let mut items: Vec<FileItem> = self
            .read_items_for_folder(folder)
            .into_iter()
            .filter(|item| !item.is_folder && !item.name.starts_with('.'))
            .collect();
        items.sort_by_key(|item| item.name.to_lowercase());
        self.tree_files
            .borrow_mut()
            .insert(folder.to_path_buf(), items.clone());
        items
    }

    /// List the folder holding `path` with `path` selected and scrolled into
    /// view.
    pub fn reveal(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(folder) = path.parent() else {
            return;
        };
        self.show_folder(folder.to_path_buf(), cx);
        self.selected_items.clear();
        self.selected_items.insert(path.clone());
        self.selection_anchor = Some(path.clone());
        self.pending_reveal = Some(path);
        cx.notify();
    }

    /// Whether `path` is among the items selected in the folder's contents.
    pub fn is_item_selected(&self, path: &Path) -> bool {
        self.selected_items.contains(path)
    }

    /// Whether there is something to paste.
    pub fn has_clipboard(&self) -> bool {
        self.clipboard
            .as_ref()
            .is_some_and(|(items, _)| !items.is_empty())
    }

    /// Put `paths` on the drawer's clipboard, to move (`cut`) or copy them on
    /// the next paste here or in the drawer.
    pub fn set_clipboard(&mut self, paths: Vec<PathBuf>, cut: bool) {
        self.clipboard = Some((paths, cut));
    }

    /// Paste the clipboard into `folder`. A name already taken there gets a
    /// "copy" suffix, so nothing is overwritten; a cut item already in
    /// `folder` stays put. Returns where the items went.
    pub fn paste_into(&mut self, folder: &Path, cx: &mut Context<Self>) -> Vec<PathBuf> {
        let Some((items, is_cut)) = self.clipboard.clone() else {
            return Vec::new();
        };
        let mut pasted = Vec::new();
        for item in &items {
            let Some(name) = item.file_name().map(|n| n.to_string_lossy().to_string()) else {
                continue;
            };
            if is_cut && item.parent() == Some(folder) {
                pasted.push(item.clone());
                continue;
            }
            if folder.starts_with(item) {
                tracing::error!(?item, ?folder, "paste: cannot paste a folder into itself");
                continue;
            }
            let target = free_name_in(folder, &name);
            let result = if is_cut {
                if remote(item) {
                    engine_fs::virtual_fs::rename(item, &target)
                } else {
                    std::fs::rename(item, &target).map_err(Into::into)
                }
            } else if remote(item) {
                engine_fs::virtual_fs::read_file(item)
                    .and_then(|data| engine_fs::virtual_fs::write_file(&target, &data))
            } else if item.is_dir() {
                Self::copy_dir_recursive(item, &target).map_err(Into::into)
            } else {
                std::fs::copy(item, &target).map(|_| ()).map_err(Into::into)
            };
            match result {
                Ok(()) => pasted.push(target),
                Err(error) => tracing::error!("paste: {}", error),
            }
        }
        if is_cut {
            self.clipboard = None;
        }
        self.files_changed(cx);
        pasted
    }

    /// Delete `paths` from disk.
    pub fn delete_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        for path in paths {
            let result = if remote(path) {
                engine_fs::virtual_fs::delete_path(path)
            } else if path.is_dir() {
                std::fs::remove_dir_all(path).map_err(Into::into)
            } else {
                std::fs::remove_file(path).map_err(Into::into)
            };
            if let Err(error) = result {
                tracing::error!("delete: {}", error);
            }
            self.selected_items.remove(path);
        }
        self.files_changed(cx);
    }

    /// Rename `old` to `new_name` in the same folder, carrying its colour and
    /// other metadata. Refuses an invalid name or one already taken.
    pub fn rename_path(
        &mut self,
        old: &Path,
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> Result<PathBuf, String> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err("A name is required".into());
        }
        if new_name.contains(INVALID_NAME_CHARS) {
            return Err("Invalid file name".into());
        }
        if old.file_name().and_then(|n| n.to_str()) == Some(new_name) {
            return Ok(old.to_path_buf());
        }
        if let Some(parent) = old.parent() {
            if exists(&cloud_join(parent, new_name)) {
                return Err(format!("{new_name} already exists"));
            }
        }
        let new = self
            .operations
            .rename_item(old, new_name)
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.fs_metadata.rename_file(old, &new) {
            tracing::error!("rename_file: {}", error);
        }
        if let Some(selected) = &self.selected_folder {
            if let Ok(rest) = selected.strip_prefix(old) {
                self.selected_folder = Some(new.join(rest));
            }
        }
        if self.selected_items.remove(old) {
            self.selected_items.insert(new.clone());
        }
        self.files_changed(cx);
        Ok(new)
    }

    /// Re-read the tree and listings after this drawer changed files.
    pub(crate) fn files_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(root) = &self.project_path {
            self.folder_tree = FolderNode::from_path(root);
        }
        self.mark_directory_cache_dirty();
        cx.notify();
    }

    /// The index of the item a [`Self::reveal`] asked to scroll to, among
    /// `items`, taking the request.
    pub(crate) fn take_pending_reveal(&mut self, items: &[FileItem]) -> Option<usize> {
        let path = self.pending_reveal.as_ref()?;
        let index = items.iter().position(|item| &item.path == path)?;
        self.pending_reveal = None;
        Some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_name_gets_a_copy_suffix_before_its_extension() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        assert_eq!(free_name_in(folder, "Hero.class"), folder.join("Hero.class"));
        std::fs::write(folder.join("Hero.class"), "").unwrap();
        assert_eq!(free_name_in(folder, "Hero.class"), folder.join("Hero copy.class"));
        std::fs::write(folder.join("Hero copy.class"), "").unwrap();
        assert_eq!(
            free_name_in(folder, "Hero.class"),
            folder.join("Hero copy 2.class")
        );
        std::fs::create_dir(folder.join("Maps")).unwrap();
        assert_eq!(free_name_in(folder, "Maps"), folder.join("Maps copy"));
        std::fs::write(folder.join(".env"), "").unwrap();
        assert_eq!(free_name_in(folder, ".env"), folder.join(".env copy"));
    }
}
