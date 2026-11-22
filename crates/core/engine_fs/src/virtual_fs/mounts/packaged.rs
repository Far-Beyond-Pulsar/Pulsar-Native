//! The folder source a packaged game serves from the snapshot in its package.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::providers::{FsEntry, FsMetadata, FsProvider};

use super::*;

/// A packaged game's copy of a plugin folder: the snapshot the packager wrote
/// at `plugins/<segment>/` in the game's content, read from the game's
/// provider (the pak, or loose files). A package made before folders took the
/// plugin's display name holds it at `plugins/<owner id>/`, which is read too.
pub(super) struct PackagedFolder {
    /// The plugin's id.
    pub(super) owner: String,
    /// The folder's visible name.
    pub(super) segment: String,
    /// The package's Content root, distinct from the project's asset cache.
    pub(super) content_root: Option<PathBuf>,
}

impl PackagedFolder {
    /// Where `path` is in the content: under the visible name, then under the
    /// id (an older package).
    fn candidates(&self, project: &Path, path: &str) -> Vec<PathBuf> {
        let inner = clean(path);
        let root = self.content_root.as_deref().unwrap_or(project);
        let mut names = vec![self.segment.as_str()];
        if self.owner != self.segment {
            names.push(self.owner.as_str());
        }
        names
            .into_iter()
            .map(|name| {
                let dir = root.join(PLUGINS_FOLDER).join(name);
                if inner.is_empty() {
                    dir
                } else {
                    dir.join(&inner)
                }
            })
            .collect()
    }

    fn first<T>(
        &self,
        project: &Path,
        path: &str,
        read: impl Fn(&Arc<dyn FsProvider>, &Path) -> Result<T>,
    ) -> Result<T> {
        let base = super::super::base_provider();
        let mut failure = None;
        for real in self.candidates(project, path) {
            match read(&base, &real) {
                Ok(found) => return Ok(found),
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        Err(failure.unwrap_or_else(|| not_found_inner(path)))
    }
}

impl FolderSource for PackagedFolder {
    fn read_file(&self, project: &Path, path: &str) -> Result<Vec<u8>> {
        self.first(project, path, |base, real| base.read_file(real))
    }

    fn list_dir(&self, project: &Path, path: &str) -> Result<Vec<FsEntry>> {
        self.first(project, path, |base, real| base.list_dir(real))
            .or_else(|error| {
                if clean(path).is_empty() {
                    Ok(Vec::new())
                } else {
                    Err(error)
                }
            })
    }

    fn metadata(&self, project: &Path, path: &str) -> Option<FsMetadata> {
        if clean(path).is_empty() {
            return Some(dir_metadata());
        }
        self.first(project, path, |base, real| base.metadata(real))
            .ok()
    }
}

pub(super) fn not_found_inner(path: &str) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("'{path}' is not in this plugin folder"),
    ))
}

pub(super) fn dir_entry(name: &str) -> FsEntry {
    FsEntry {
        name: name.to_owned(),
        is_dir: true,
        size: 0,
        modified: None,
    }
}

pub(super) fn dir_metadata() -> FsMetadata {
    FsMetadata {
        is_dir: true,
        size: 0,
        modified: None,
        modified_ns: None,
    }
}
