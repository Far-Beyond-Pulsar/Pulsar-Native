//! Serving a pak through `engine_fs::virtual_fs`.
//!
//! Engine code reads project files by path through `virtual_fs` (levels,
//! class definitions, script modules, settings). A packaged game installs a
//! [`ContentFsProvider`]: paths under the content root are looked up in the
//! pak first and fall back to loose files, everything else is the local
//! disk. So one loading path serves the editor, dev builds and shipped
//! games.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use engine_fs::{FsEntry, FsMetadata, FsProvider, LocalFsProvider};

use crate::pak::PakReader;

/// A local filesystem with a pak mounted (read-only) at `root`.
pub struct ContentFsProvider {
    root: PathBuf,
    pak: Arc<PakReader>,
    local: LocalFsProvider,
}

impl ContentFsProvider {
    pub fn new(root: PathBuf, pak: Arc<PakReader>) -> Self {
        Self { root, pak, local: LocalFsProvider::new() }
    }

    /// The content-relative path of `path` when it is under the root
    /// (`Some("")` for the root itself).
    fn rel(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?;
        let text = rel.to_string_lossy();
        if text.is_empty() {
            return Some(String::new());
        }
        crate::normalize_rel(&text)
    }

    fn pak_file(&self, path: &Path) -> Option<String> {
        self.rel(path).filter(|rel| !rel.is_empty() && self.pak.contains(rel))
    }

    fn pak_dir(&self, path: &Path) -> Option<String> {
        self.rel(path).filter(|rel| self.pak.contains_dir(rel))
    }
}

impl FsProvider for ContentFsProvider {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        match self.pak_file(path) {
            Some(rel) => Ok(self.pak.read(&rel)?),
            None => self.local.read_file(path),
        }
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.local.write_file(path, content)
    }

    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.local.create_file(path, content)
    }

    fn delete_path(&self, path: &Path) -> Result<()> {
        if self.pak_file(path).is_some() {
            anyhow::bail!("{} is packed content and cannot be deleted", path.display());
        }
        self.local.delete_path(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.local.rename(from, to)
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>> {
        let mut entries = self.local.list_dir(path).unwrap_or_default();
        if let Some(rel) = self.pak_dir(path) {
            for (name, is_dir, size) in self.pak.list_dir(&rel) {
                if !entries.iter().any(|e| e.name == name) {
                    entries.push(FsEntry { name, is_dir, size, modified: None });
                }
            }
        } else if entries.is_empty() {
            return self.local.list_dir(path);
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.local.create_dir_all(path)
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        if self.pak_file(path).is_some() || self.pak_dir(path).is_some() {
            return Ok(true);
        }
        self.local.exists(path)
    }

    fn metadata(&self, path: &Path) -> Result<FsMetadata> {
        if let Some(rel) = self.pak_file(path) {
            let len = self.pak.entry(&rel).map_or(0, |e| e.len);
            return Ok(FsMetadata { is_dir: false, size: len, modified: None });
        }
        if self.pak_dir(path).is_some() {
            return Ok(FsMetadata { is_dir: true, size: 0, modified: None });
        }
        self.local.metadata(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        if self.pak_file(path).is_some() || self.pak_dir(path).is_some() {
            return Ok(path.to_path_buf());
        }
        self.local.canonicalize(path)
    }

    fn is_symlink(&self, path: &Path) -> Result<bool> {
        if self.pak_file(path).is_some() || self.pak_dir(path).is_some() {
            return Ok(false);
        }
        self.local.is_symlink(path)
    }

    fn label(&self) -> &str {
        "Game content"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pak::PakWriter;

    #[test]
    fn pak_paths_read_from_the_pak_and_others_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Content");
        std::fs::create_dir_all(&root).unwrap();
        let mut writer = PakWriter::create(root.join("game.pak")).unwrap();
        writer.add("src/classes/A/prefab.json", b"{}").unwrap();
        writer.finish().unwrap();
        std::fs::write(root.join("loose.txt"), b"loose").unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"out").unwrap();

        let fs = ContentFsProvider::new(root.clone(), Arc::new(PakReader::open(root.join("game.pak")).unwrap()));
        assert_eq!(fs.read_file(&root.join("src/classes/A/prefab.json")).unwrap(), b"{}");
        assert_eq!(fs.read_file(&root.join("loose.txt")).unwrap(), b"loose");
        assert_eq!(fs.read_file(&outside).unwrap(), b"out");
        assert!(fs.exists(&root.join("src/classes")).unwrap());
        assert!(fs.metadata(&root.join("src/classes/A")).unwrap().is_dir);
        assert!(!fs.exists(&root.join("src/nothing")).unwrap());
        let names: Vec<String> = fs.list_dir(&root).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["game.pak", "loose.txt", "src"]);
        let classes: Vec<(String, bool)> =
            fs.list_dir(&root.join("src/classes")).unwrap().into_iter().map(|e| (e.name, e.is_dir)).collect();
        assert_eq!(classes, [("A".to_string(), true)]);
    }
}
