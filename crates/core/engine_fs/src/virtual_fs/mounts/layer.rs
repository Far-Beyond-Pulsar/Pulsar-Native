//! [`MountedFs`]: the filesystem layer that composes the plugin folders over
//! the active provider.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::providers::{FsEntry, FsMetadata, FsProvider, ManifestEntry};

use super::*;

/// The active provider with the plugin folders composed over it.
pub struct MountedFs {
    base: Arc<dyn FsProvider>,
}

impl MountedFs {
    pub fn new(base: Arc<dyn FsProvider>) -> Self {
        Self { base }
    }

    /// The provider underneath, which knows no plugin folders.
    pub fn base(&self) -> &Arc<dyn FsProvider> {
        &self.base
    }
}

fn has_folders() -> bool {
    !tree().folders.is_empty()
}

impl FsProvider for MountedFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        match locate(path) {
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } => source.read_file(&project, &inner),
            Located::Folder { .. } | Located::PluginsFolder => Err(not_found(path)),
            _ => self.base.read_file(path),
        }
    }

    fn read_file_prefix(&self, path: &Path, len: usize) -> Result<Vec<u8>> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.read_file_prefix(path, len),
            _ => {
                let mut bytes = self.read_file(path)?;
                bytes.truncate(len);
                Ok(bytes)
            }
        }
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.write_file(path, content)
    }

    fn write_file_atomically(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.write_file_atomically(path, content)
    }

    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.create_file(path, content)
    }

    fn create_executable_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.create_executable_file(path, content)
    }

    fn append_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.append_file(path, content)
    }

    fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        refuse(to)?;
        match locate(from) {
            Located::Outside | Located::ProjectRoot => self.base.copy(from, to),
            _ => self.base.write_file(to, &self.read_file(from)?),
        }
    }

    fn open_read(&self, path: &Path) -> Result<Box<dyn crate::providers::ReadSeek>> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.open_read(path),
            _ => Ok(Box::new(std::io::Cursor::new(self.read_file(path)?))),
        }
    }

    fn open_write_stream(&self, path: &Path) -> Result<Option<Box<dyn std::io::Write + Send>>> {
        refuse(path)?;
        self.base.open_write_stream(path)
    }

    fn delete_path(&self, path: &Path) -> Result<()> {
        refuse(path)?;
        self.base.delete_path(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        refuse(from)?;
        refuse(to)?;
        self.base.rename(from, to)
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>> {
        match locate(path) {
            Located::Outside => self.base.list_dir(path),
            Located::ProjectRoot => {
                let mut entries: Vec<FsEntry> = self
                    .base
                    .list_dir(path)?
                    .into_iter()
                    .filter(|e| !(e.is_dir && e.name.eq_ignore_ascii_case(PLUGINS_FOLDER)))
                    .collect();
                if has_folders() {
                    entries.push(dir_entry(PLUGINS_FOLDER));
                }
                Ok(entries)
            }
            Located::PluginsFolder => {
                Ok(tree().folders.keys().map(|name| dir_entry(name)).collect())
            }
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } => source.list_dir(&project, &inner),
            Located::Folder { .. } => Err(not_found(path)),
        }
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        match locate(path) {
            // What already exists needs no creating; anything new in a
            // plugin folder is its owner's to make.
            Located::PluginsFolder if has_folders() => Ok(()),
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } if source
                .metadata(&project, &inner)
                .is_some_and(|meta| meta.is_dir) =>
            {
                Ok(())
            }
            Located::PluginsFolder | Located::Folder { .. } => refuse(path),
            _ => self.base.create_dir_all(path),
        }
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.exists(path),
            Located::PluginsFolder => Ok(has_folders()),
            Located::Folder {
                project,
                inner,
                source,
                ..
            } => Ok(source.is_some_and(|source| source.metadata(&project, &inner).is_some())),
        }
    }

    fn metadata(&self, path: &Path) -> Result<FsMetadata> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.metadata(path),
            Located::PluginsFolder if has_folders() => Ok(dir_metadata()),
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } => source
                .metadata(&project, &inner)
                .ok_or_else(|| not_found(path)),
            _ => Err(not_found(path)),
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.canonicalize(path),
            // Plugin folders are virtual: their paths are their own
            // canonical form.
            _ if self.exists(path)? => Ok(path.to_path_buf()),
            _ => Err(not_found(path)),
        }
    }

    fn is_symlink(&self, path: &Path) -> Result<bool> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.is_symlink(path),
            _ => Ok(false),
        }
    }

    fn manifest(&self, path: &Path) -> Result<Vec<ManifestEntry>> {
        let folder_entries = |project: &Path, prefix: &str| -> Vec<ManifestEntry> {
            let tree = tree();
            let mut out = Vec::new();
            for (name, folder) in &tree.folders {
                let at = join_inner(prefix, name);
                out.push(ManifestEntry {
                    path: at.clone(),
                    is_dir: true,
                    size: 0,
                    modified: None,
                });
                out.extend(
                    folder
                        .source
                        .manifest(project, "")
                        .into_iter()
                        .map(|mut entry| {
                            entry.path = format!("{at}/{}", entry.path);
                            entry
                        }),
                );
            }
            out
        };
        match locate(path) {
            Located::Outside => self.base.manifest(path),
            Located::ProjectRoot => {
                let mut out: Vec<ManifestEntry> = self
                    .base
                    .manifest(path)?
                    .into_iter()
                    .filter(|entry| {
                        !entry
                            .path
                            .split('/')
                            .next()
                            .is_some_and(|first| first.eq_ignore_ascii_case(PLUGINS_FOLDER))
                    })
                    .collect();
                if has_folders() {
                    out.push(ManifestEntry {
                        path: PLUGINS_FOLDER.to_owned(),
                        is_dir: true,
                        size: 0,
                        modified: None,
                    });
                    out.extend(folder_entries(path, PLUGINS_FOLDER));
                }
                Ok(out)
            }
            Located::PluginsFolder => {
                let project = path.parent().unwrap_or(path);
                Ok(folder_entries(project, ""))
            }
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } => Ok(source.manifest(&project, &inner)),
            Located::Folder { .. } => Ok(Vec::new()),
        }
    }

    fn is_remote(&self) -> bool {
        self.base.is_remote()
    }

    fn permits_local_executable_writes(&self) -> bool {
        self.base.permits_local_executable_writes()
    }

    fn label(&self) -> &str {
        self.base.label()
    }
}
