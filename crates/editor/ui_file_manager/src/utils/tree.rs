use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FolderNode {
    pub path: PathBuf,
    pub name: String,
    pub children: Vec<FolderNode>,
    pub expanded: bool,
}

impl FolderNode {
    pub fn from_path(path: &Path) -> Option<Self> {
        if engine_fs::is_cloud_path(path) {
            return Self::from_cloud_path(path);
        }
        if !path.is_dir() {
            return None;
        }
        let name = path.file_name()?.to_str()?.to_string();
        if [
            "graph_save.json",
            "struct.json",
            "enum.json",
            "trait.json",
            "alias.json",
        ]
        .iter()
        .any(|m| path.join(m).exists())
        {
            return None;
        }
        let children = engine_fs::virtual_fs::list_dir(path)
            .ok()?
            .into_iter()
            .filter_map(|e| {
                if e.is_dir && !e.name.starts_with('.') {
                    FolderNode::from_path(&path.join(&e.name))
                } else {
                    None
                }
            })
            .collect();
        Some(FolderNode {
            path: path.to_path_buf(),
            name,
            children,
            expanded: false,
        })
    }

    pub fn from_cloud_path(cloud_root: &Path) -> Option<Self> {
        let entries = engine_fs::virtual_fs::manifest(cloud_root).ok()?;
        let root_s = cloud_root.to_string_lossy().replace('\\', "/");
        let root_name = root_s
            .trim_start_matches("cloud+pulsar://")
            .split('/')
            .nth(1)
            .unwrap_or("Remote Project")
            .to_string();
        let mut root = FolderNode {
            path: cloud_root.to_path_buf(),
            name: root_name,
            children: Vec::new(),
            expanded: true,
        };
        for entry in entries.iter().filter(|e| e.is_dir) {
            let child = PathBuf::from(format!(
                "{}/{}",
                root_s.trim_end_matches('/'),
                entry.path.trim_start_matches('/')
            ));
            let name = entry
                .path
                .split('/')
                .next_back()
                .unwrap_or(&entry.path)
                .to_string();
            Self::insert_at_depth(&mut root, &root_s, &entry.path, child, name);
        }
        Some(root)
    }

    fn insert_at_depth(node: &mut FolderNode, root_s: &str, rel: &str, abs: PathBuf, name: String) {
        let parts: Vec<&str> = rel.splitn(2, '/').collect();
        if parts.len() == 1 {
            node.children.push(FolderNode {
                path: abs,
                name,
                children: Vec::new(),
                expanded: false,
            });
        } else {
            let parent_path =
                PathBuf::from(format!("{}/{}", root_s.trim_end_matches('/'), parts[0]));
            let parent_s = format!("{}/{}", root_s.trim_end_matches('/'), parts[0]);
            if let Some(child) = node.children.iter_mut().find(|c| c.path == parent_path) {
                Self::insert_at_depth(child, &parent_s, parts[1], abs, name);
            }
        }
    }

    pub fn toggle_expanded(&mut self, target: &Path) -> bool {
        if self.path == target {
            self.expanded = !self.expanded;
            return true;
        }
        self.children.iter_mut().any(|c| c.toggle_expanded(target))
    }

    pub fn collapse_all(&mut self) {
        self.expanded = false;
        for c in &mut self.children {
            c.collapse_all();
        }
    }
    pub fn expand_all(&mut self) {
        self.expanded = true;
        for c in &mut self.children {
            c.expand_all();
        }
    }
}
