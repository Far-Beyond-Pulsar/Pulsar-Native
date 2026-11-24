//! The in-memory folder source: files embedded in a binary, then edited at runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use parking_lot::RwLock;

use crate::providers::{FsEntry, FsMetadata};
use crate::FsChangeKind;

use super::*;

/// One file embedded in a binary, for [`MemoryFolder::with_files`].
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedFile {
    /// Path inside the folder, `/`-separated (`traits/x.trait.json`).
    pub path: &'static str,
    pub bytes: &'static [u8],
}

/// Embed files under `dir` (relative to the calling crate's manifest) as
/// [`EmbeddedFile`]s at their path inside the folder.
#[macro_export]
macro_rules! embedded_files {
    ($dir:literal: $($path:literal),* $(,)?) => {
        &[$($crate::EmbeddedFile {
            path: $path,
            bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $path)),
        }),*]
    };
}

#[derive(Default)]
struct MemoryTree {
    files: BTreeMap<String, Arc<[u8]>>,
    dirs: BTreeSet<String>,
}

impl MemoryTree {
    fn add_parents(&mut self, path: &str) {
        let mut at = 0;
        while let Some(slash) = path[at..].find('/') {
            self.dirs.insert(path[..at + slash].to_owned());
            at += slash + 1;
        }
    }

    fn is_dir(&self, path: &str) -> bool {
        path.is_empty() || self.dirs.contains(path)
    }
}

/// A plugin folder held in memory, the same for every project. It starts
/// with whatever the plugin puts in (embedded files, generated ones) and
/// changes at runtime through [`write`](Self::write),
/// [`create_dir_all`](Self::create_dir_all) and [`delete`](Self::delete),
/// each reported through the folder's capability, which it holds. Gone at
/// exit: what must last goes in a [`CacheFolder`].
pub struct MemoryFolder {
    folder: PluginFolder,
    tree: RwLock<MemoryTree>,
}

impl MemoryFolder {
    /// An empty folder.
    pub fn new(folder: PluginFolder) -> Self {
        Self {
            folder,
            tree: RwLock::new(MemoryTree::default()),
        }
    }

    /// A folder holding `files` (embedded with [`embedded_files!`](crate::embedded_files)).
    pub fn with_files(folder: PluginFolder, files: &[EmbeddedFile]) -> Self {
        let this = Self::new(folder);
        {
            let mut tree = this.tree.write();
            for file in files {
                let path = clean(file.path);
                tree.add_parents(&path);
                tree.files.insert(path, Arc::from(file.bytes));
            }
        }
        this
    }

    /// The folder's capability.
    pub fn folder(&self) -> &PluginFolder {
        &self.folder
    }

    /// Put `bytes` at `path`, creating its parent folders.
    pub fn write(&self, path: &str, bytes: impl Into<Arc<[u8]>>) {
        let path = clean(path);
        let existed = {
            let mut tree = self.tree.write();
            tree.add_parents(&path);
            tree.files.insert(path.clone(), bytes.into()).is_some()
        };
        let kind = if existed {
            FsChangeKind::Modified
        } else {
            FsChangeKind::Created
        };
        self.folder.changed(&path, kind);
    }

    /// Create the folder `path` and its parents.
    pub fn create_dir_all(&self, path: &str) {
        let path = clean(path);
        if path.is_empty() {
            return;
        }
        let created = {
            let mut tree = self.tree.write();
            tree.add_parents(&format!("{path}/"));
            tree.dirs.insert(path.clone())
        };
        if created {
            self.folder.changed(&path, FsChangeKind::Created);
        }
    }

    /// Remove the file or folder (with everything in it) at `path`.
    pub fn delete(&self, path: &str) {
        let path = clean(path);
        let prefix = format!("{path}/");
        let removed = {
            let mut tree = self.tree.write();
            let file = tree.files.remove(&path).is_some();
            let before = tree.files.len() + tree.dirs.len();
            tree.files.retain(|name, _| !name.starts_with(&prefix));
            tree.dirs
                .retain(|name| *name != path && !name.starts_with(&prefix));
            file || before != tree.files.len() + tree.dirs.len()
        };
        if removed {
            self.folder.changed(&path, FsChangeKind::Deleted);
        }
    }
}

impl FolderSource for MemoryFolder {
    fn project_independent(&self) -> bool {
        true
    }

    fn read_file(&self, _project: &Path, path: &str) -> Result<Vec<u8>> {
        self.tree
            .read()
            .files
            .get(&clean(path))
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| not_found_inner(path))
    }

    fn list_dir(&self, _project: &Path, path: &str) -> Result<Vec<FsEntry>> {
        let path = clean(path);
        let tree = self.tree.read();
        if !tree.is_dir(&path) {
            return Err(not_found_inner(&path));
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let direct = |name: &str| {
            name.strip_prefix(prefix.as_str())
                .filter(|rest| !rest.is_empty() && !rest.contains('/'))
                .map(str::to_owned)
        };
        let mut entries: Vec<FsEntry> = tree
            .dirs
            .iter()
            .filter_map(|dir| direct(dir))
            .map(|name| dir_entry(&name))
            .collect();
        entries.extend(tree.files.iter().filter_map(|(name, bytes)| {
            direct(name).map(|name| FsEntry {
                name,
                is_dir: false,
                size: bytes.len() as u64,
                modified: None,
            })
        }));
        Ok(entries)
    }

    fn metadata(&self, _project: &Path, path: &str) -> Option<FsMetadata> {
        let path = clean(path);
        let tree = self.tree.read();
        if tree.is_dir(&path) {
            return Some(dir_metadata());
        }
        tree.files.get(&path).map(|bytes| FsMetadata {
            is_dir: false,
            size: bytes.len() as u64,
            modified: None,
            modified_ns: None,
        })
    }
}
