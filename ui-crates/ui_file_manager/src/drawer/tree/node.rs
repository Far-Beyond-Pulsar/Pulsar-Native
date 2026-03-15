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
    /// Build a folder tree from `path`.
    ///
    /// When `path` starts with `cloud+pulsar://` the tree is populated
    /// by calling the remote virtual-filesystem API in one round-trip
    /// (via [`engine_fs::virtual_fs::manifest`]).  Otherwise the local
    /// disk is walked as before.
    pub fn from_path(path: &Path) -> Option<Self> {
        if engine_fs::is_cloud_path(path) {
            return Self::from_cloud_path(path);
        }

        if !path.is_dir() {
            return None;
        }

        let name = path.file_name()?.to_str()?.to_string();

        // Skip special engine type-definition folders.
        let has_marker_file = ["graph_save.json", "struct.json", "enum.json", "trait.json", "alias.json"]
            .iter()
            .any(|marker| path.join(marker).exists());
        if has_marker_file {
            return None;
        }

        // Read child folders (skip files).
        let children = std::fs::read_dir(path)
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let entry_path = entry.path();

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

    /// Build a folder tree from a remote `cloud+pulsar://` path by fetching
    /// the full manifest in a single HTTP round-trip and constructing the
    /// in-memory tree from the flat entry list.
    pub fn from_cloud_path(cloud_root: &Path) -> Option<Self> {
        let entries = engine_fs::virtual_fs::manifest(cloud_root).ok()?;

        // Normalize to forward slashes once so all subsequent string operations
        // work correctly on Windows where PathBuf stores backslashes.
        let cloud_root_s = cloud_root.to_string_lossy().replace('\\', "/");

        let root_name = cloud_root_s
            .trim_start_matches("cloud+pulsar://")
            .splitn(3, '/')
            .nth(1)
            .unwrap_or("Remote Project")
            .to_string();

        // Build the tree from the flat manifest.
        // Only include directories (files are shown in the content panel).
        let mut root = FolderNode {
            path: cloud_root.to_path_buf(),
            name: root_name,
            children: Vec::new(),
            expanded: true,
        };

        for entry in entries.iter().filter(|e| e.is_dir) {
            // Build a cloud+pulsar:// path for this subdirectory using the
            // already-normalized string so no '\\' ever appears in the URI.
            let child_cloud = PathBuf::from(format!(
                "{}/{}",
                cloud_root_s.trim_end_matches('/'),
                entry.path.trim_start_matches('/')
            ));
            let name = entry.path
                .split('/')
                .last()
                .unwrap_or(&entry.path)
                .to_string();
            Self::insert_at_depth(
                &mut root,
                &cloud_root_s,
                &entry.path,
                child_cloud,
                name,
            );
        }

        Some(root)
    }

    /// Recursively insert a directory node at the correct position.
    ///
    /// `cloud_root_s` is the cloud root URI as a forward-slash-normalized
    /// string (never a Windows `PathBuf` representation).
    fn insert_at_depth(
        node: &mut FolderNode,
        cloud_root_s: &str,
        rel_path: &str,
        abs_cloud: PathBuf,
        name: String,
    ) {
        let parts: Vec<&str> = rel_path.splitn(2, '/').collect();
        if parts.len() == 1 {
            // Direct child of this node.
            node.children.push(FolderNode {
                path: abs_cloud,
                name,
                children: Vec::new(),
                expanded: false,
            });
        } else {
            // Find the intermediate child.
            let first = parts[0];
            let rest = parts[1];
            let parent_cloud = PathBuf::from(format!(
                "{}/{}",
                cloud_root_s.trim_end_matches('/'),
                first
            ));
            let parent_cloud_s = format!("{}/{}", cloud_root_s.trim_end_matches('/'), first);
            if let Some(child) = node.children.iter_mut().find(|c| c.path == parent_cloud) {
                Self::insert_at_depth(child, &parent_cloud_s, rest, abs_cloud, name);
            }
        }
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

