//! The per-project cache folder source, kept on disk under `.pulsar/plugins/`.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::providers::{FsEntry, FsMetadata};
use crate::FsChangeKind;

use super::*;

/// A plugin folder kept per project in `<project>/.pulsar/plugins/<owner>/`
/// (derived data version control ignores, like the rest of `.pulsar/`), so
/// what the plugin wrote at runtime is there next time. Writes need the
/// project they are for and are reported through the folder's capability,
/// which it holds. The files are written through the project's own provider
/// (`virtual_fs`, not `std::fs`), so a remote project keeps its cache too.
pub struct CacheFolder {
    folder: PluginFolder,
}

impl CacheFolder {
    pub fn new(folder: PluginFolder) -> Self {
        Self { folder }
    }

    /// The folder's capability.
    pub fn folder(&self) -> &PluginFolder {
        &self.folder
    }

    /// Where the folder's files are kept for `project`.
    pub fn dir(&self, project: &Path) -> PathBuf {
        project.join(CACHE_FOLDER).join(self.folder.owner())
    }

    fn real(&self, project: &Path, path: &str) -> PathBuf {
        let inner = clean(path);
        let dir = self.dir(project);
        if inner.is_empty() {
            dir
        } else {
            dir.join(inner)
        }
    }

    /// Put `bytes` at `path` in `project`'s copy, creating its parents.
    pub fn write(&self, project: &Path, path: &str, bytes: &[u8]) -> Result<()> {
        let real = self.real(project, path);
        let existed = super::super::exists(&real).unwrap_or(false);
        super::super::write_file(&real, bytes)?;
        let kind = if existed {
            FsChangeKind::Modified
        } else {
            FsChangeKind::Created
        };
        self.folder.changed_in(project, path, kind);
        Ok(())
    }

    /// Create the folder `path` (and its parents) in `project`'s copy.
    pub fn create_dir_all(&self, project: &Path, path: &str) -> Result<()> {
        super::super::create_dir_all(&self.real(project, path))?;
        self.folder.changed_in(project, path, FsChangeKind::Created);
        Ok(())
    }

    /// Remove the file or folder at `path` from `project`'s copy.
    pub fn delete(&self, project: &Path, path: &str) -> Result<()> {
        super::super::delete_path(&self.real(project, path))?;
        self.folder.changed_in(project, path, FsChangeKind::Deleted);
        Ok(())
    }
}

impl FolderSource for CacheFolder {
    fn read_file(&self, project: &Path, path: &str) -> Result<Vec<u8>> {
        super::super::base_provider().read_file(&self.real(project, path))
    }

    fn list_dir(&self, project: &Path, path: &str) -> Result<Vec<FsEntry>> {
        let real = self.real(project, path);
        let base = super::super::base_provider();
        if clean(path).is_empty() && !base.exists(&real).unwrap_or(false) {
            // Nothing written for this project yet: an empty folder.
            return Ok(Vec::new());
        }
        base.list_dir(&real)
    }

    fn metadata(&self, project: &Path, path: &str) -> Option<FsMetadata> {
        if clean(path).is_empty() {
            return Some(dir_metadata());
        }
        super::super::base_provider()
            .metadata(&self.real(project, path))
            .ok()
    }
}
