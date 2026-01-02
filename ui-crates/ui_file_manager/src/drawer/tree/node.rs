use std::path::{Path, PathBuf};
use super::super::types::FileItem;

// ============================================================================
// FOLDER NODE - Hierarchical folder tree structure
// ============================================================================

#[derive(Clone, Debug)]
pub struct FolderNode {
    pub path: PathBuf,
    pub name: String,
    pub children: Vec<FolderNode>,
    pub expanded: bool,
}

impl FolderNode {
    pub fn from_path(path: &Path) -> Option<Self> {
        if !path.is_dir() {
            return None;
        }

        let name = path.file_name()?.to_str()?.to_string();

        // Check if this is a special file type folder (class, struct, etc.) - don't show in tree
        // We need to check for marker files like graph_save.json
        let has_marker_file = ["graph_save.json", "struct.json", "enum.json", "trait.json", "alias.json"]
            .iter()
            .any(|marker| path.join(marker).exists());
        
        if has_marker_file {
            return None;
        }

        // Read child folders (skip files)
        let children = std::fs::read_dir(path)
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let entry_path = entry.path();

                // Skip hidden files and non-directories
                if !entry_path.is_dir() {
                    return None;
                }
                if entry_path.file_name()?.to_str()?.starts_with('.') {
                    return None;
                }

                FolderNode::from_path(&entry_path)
            })
            .collect();

        Some(FolderNode {
            path: path.to_path_buf(),
            name,
            children,
            expanded: false,
        })
    }

    pub fn toggle_expanded(&mut self, target_path: &Path) -> bool {
        if self.path == target_path {
            self.expanded = !self.expanded;
            return true;
        }

        for child in &mut self.children {
            if child.toggle_expanded(target_path) {
                return true;
            }
        }

        false
    }

    pub fn collapse_all(&mut self) {
        self.expanded = false;
        for child in &mut self.children {
            child.collapse_all();
        }
    }

    pub fn expand_all(&mut self) {
        self.expanded = true;
        for child in &mut self.children {
            child.expand_all();
        }
    }
}
