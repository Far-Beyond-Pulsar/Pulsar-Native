//! The process-shared index per project, and applying VFS operations to every
//! open index.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use super::*;
use crate::events::{FsChangeKind, FsEventSource};

pub(super) static SHARED_FILE_INDEXES: OnceLock<
    parking_lot::Mutex<HashMap<PathBuf, Arc<ProjectFileIndex>>>,
> = OnceLock::new();

/// Projects whose index last failed to open: when, and the error.
static FAILED_FILE_INDEX_OPENS: OnceLock<
    parking_lot::Mutex<HashMap<PathBuf, (std::time::Instant, String)>>,
> = OnceLock::new();

/// How long a failed open is answered from [`FAILED_FILE_INDEX_OPENS`]
/// before the next caller tries again.
const FAILED_OPEN_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// Get the process-shared index for a local project, creating and populating
/// the cache the first time it is requested. Remote and P2P projects return
/// `Ok(None)` so callers can use their provider's native listing path.
pub fn open_project_file_index(project_root: &Path) -> Result<Option<Arc<ProjectFileIndex>>> {
    // The index lists the project as the VFS shows it, its `plugins/` folder
    // included, whoever opens it first (the editor's loading screen opens it
    // before the project is published).
    crate::virtual_fs::mount_project(project_root);
    if crate::virtual_fs::is_remote() || crate::is_cloud_path(project_root) {
        return Ok(None);
    }
    let key = normalize_absolute_path(
        crate::virtual_fs::canonicalize(project_root)
            .unwrap_or_else(|_| project_root.to_path_buf()),
    );
    let indexes = SHARED_FILE_INDEXES.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    let existing = { indexes.lock().get(&key).cloned() };
    if let Some(index) = existing {
        if let Err(error) = crate::watchers::start_file_index_watcher(key, index.clone()) {
            tracing::warn!(%error, root = %index.project_root().display(), "Could not watch project file index for live changes");
        }
        return Ok(Some(index.clone()));
    }

    // A failed open is a whole-project scan that failed. Many callers ask for
    // the index from render paths, so retrying on every call turns one bad
    // index into a scan per caller per frame; retry only after a pause.
    let failures = FAILED_FILE_INDEX_OPENS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    if let Some((at, error)) = failures.lock().get(&key) {
        if at.elapsed() < FAILED_OPEN_RETRY {
            anyhow::bail!("{error}");
        }
    }

    // Opening creates `.pulsar/cache` through VFS, which emits a local event.
    // Never hold the shared registry lock across provider calls or that event
    // handler would re-enter this registry and deadlock.
    let opened = (|| -> Result<Arc<ProjectFileIndex>> {
        let index = Arc::new(ProjectFileIndex::open(key.clone())?);
        let manifest =
            crate::virtual_fs::manifest(&key).context("read project manifest for file index")?;
        index.reconcile(&manifest)?;
        index.restore_durable_asset_ids()?;
        Ok(index)
    })();
    let index = match opened {
        Ok(index) => {
            failures.lock().remove(&key);
            index
        }
        Err(error) => {
            failures.lock().insert(
                key.clone(),
                (std::time::Instant::now(), format!("{error:#}")),
            );
            return Err(error);
        }
    };
    let index = {
        let mut indexes = indexes.lock();
        if let Some(existing) = indexes.get(&key) {
            existing.clone()
        } else {
            indexes.insert(key.clone(), index.clone());
            index
        }
    };
    if let Err(error) = crate::watchers::start_file_index_watcher(key.clone(), index.clone()) {
        tracing::warn!(%error, root = %key.display(), "Could not watch project file index for live changes");
    }
    Ok(Some(index))
}

/// Return an already-open project index containing an absolute filesystem path.
/// This never opens a database, so callers can use it after the project index
/// loading task has completed without accidentally triggering a scan.
pub fn project_file_index_for_path(path: &Path) -> Option<Arc<ProjectFileIndex>> {
    if !path.is_absolute() {
        return None;
    }
    let path = crate::virtual_fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let indexes = SHARED_FILE_INDEXES.get()?.lock();
    indexes
        .values()
        .filter(|index| index.relative_path(&path).is_some())
        .max_by_key(|index| index.project_root.components().count())
        .cloned()
}

/// Synchronously apply a local VFS operation to every already-open index that
/// owns its path. Local notify watchers remain responsible for external disk
/// changes; remote VFS operations must not update this local-only cache.
pub fn update_open_project_file_indexes(path: &Path, kind: &FsChangeKind, source: &FsEventSource) {
    if !matches!(source, FsEventSource::Local) {
        return;
    }
    let indexes = SHARED_FILE_INDEXES
        .get()
        .map(|indexes| indexes.lock().values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for index in indexes {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            index.project_root.join(path)
        };
        if index.relative_path(&path).is_none() {
            continue;
        }
        let result = match kind {
            FsChangeKind::Deleted => index.remove_path(&path),
            FsChangeKind::Created | FsChangeKind::Modified => index.refresh_path(&path),
        };
        if let Err(error) = result {
            tracing::warn!(%error, path = %path.display(), "Could not update project file index from local VFS event");
        }
    }
}

/// Move indexed paths in every open local project that contains either side.
/// If a move crosses a project boundary, remove it from the old index and
/// refresh it in the new one.
pub fn move_open_project_file_indexes(from: &Path, to: &Path, source: &FsEventSource) {
    if !matches!(source, FsEventSource::Local) {
        return;
    }
    let indexes = SHARED_FILE_INDEXES
        .get()
        .map(|indexes| indexes.lock().values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for index in indexes {
        let from_path = if from.is_absolute() {
            from.to_path_buf()
        } else {
            index.project_root.join(from)
        };
        let to_path = if to.is_absolute() {
            to.to_path_buf()
        } else {
            index.project_root.join(to)
        };
        let from_is_in_project = index.relative_path(&from_path).is_some();
        let to_is_in_project = index.relative_path(&to_path).is_some();
        let result = match (from_is_in_project, to_is_in_project) {
            (true, true) => index.move_path(&from_path, &to_path),
            (true, false) => index.remove_path(&from_path),
            (false, true) => index.refresh_path(&to_path),
            (false, false) => continue,
        };
        if let Err(error) = result {
            tracing::warn!(%error, from = %from_path.display(), to = %to_path.display(), "Could not move paths in project file index");
        }
    }
}
