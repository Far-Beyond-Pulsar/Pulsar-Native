//! Virtual filesystem global — routes all file I/O to the correct backend.
//!
//! Call [`set_provider`] once (e.g. when opening a cloud project) to switch
//! from the default local-disk provider to a remote one.  The rest of the
//! editor uses the free functions in this module and never needs to care
//! which backend is active.

use anyhow::Result;
use parking_lot::RwLock;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::providers::{FsEntry, FsMetadata, FsProvider, LocalFsProvider, ManifestEntry};
use crate::{events, FsChangeKind};

pub mod path_utils;

// Re-export commonly used path utilities
pub use path_utils::{cloud_join, is_cloud_path, normalize_path};

// ── Singleton ─────────────────────────────────────────────────────────────────

static VIRTUAL_FS: OnceLock<Arc<RwLock<Arc<dyn FsProvider>>>> = OnceLock::new();

fn global() -> &'static Arc<RwLock<Arc<dyn FsProvider>>> {
    VIRTUAL_FS.get_or_init(|| Arc::new(RwLock::new(Arc::new(LocalFsProvider::new()))))
}

// ── Configuration ─────────────────────────────────────────────────────────────

/// Replace the active filesystem provider.
///
/// Call this before opening the editor when connecting to a cloud project.
/// The change is process-wide and takes effect immediately.
pub fn set_provider(provider: Arc<dyn FsProvider>) {
    *global().write() = provider;
}

/// Restore the default local-disk provider (e.g. when disconnecting).
pub fn reset_to_local() {
    set_provider(Arc::new(LocalFsProvider::new()));
}

/// Return the currently active provider (cloned `Arc` — cheap).
pub fn provider() -> Arc<dyn FsProvider> {
    global().read().clone()
}

// ── Introspection ─────────────────────────────────────────────────────────────

/// `true` when the active provider serves a remote filesystem.
pub fn is_remote() -> bool {
    global().read().is_remote()
}

/// Human-readable label for the current provider ("Local" or "Remote").
pub fn current_label() -> String {
    global().read().label().to_string()
}

// ── Convenience pass-throughs ─────────────────────────────────────────────────

/// Read the full contents of a file.
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    global().read().read_file(path)
}

/// Overwrite (or create) a file with `content`.
pub fn write_file(path: &Path, content: &[u8]) -> Result<()> {
    let result = global().read().write_file(path, content);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Modified);
    }
    result
}

/// Create a new file with `content`, failing if it already exists.
pub fn create_file(path: &Path, content: &[u8]) -> Result<()> {
    let result = global().read().create_file(path, content);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Created);
    }
    result
}

/// Delete a file or recursively remove a directory.
pub fn delete_path(path: &Path) -> Result<()> {
    let result = global().read().delete_path(path);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Deleted);
    }
    result
}

/// Rename / move a path.
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    let result = global().read().rename(from, to);
    if result.is_ok() {
        events::emit(from.to_path_buf(), FsChangeKind::Deleted);
        events::emit(to.to_path_buf(), FsChangeKind::Created);
    }
    result
}

/// List the immediate children of a directory.
pub fn list_dir(path: &Path) -> Result<Vec<FsEntry>> {
    global().read().list_dir(path)
}

/// Recursively create a directory and all missing parents.
pub fn create_dir_all(path: &Path) -> Result<()> {
    let result = global().read().create_dir_all(path);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Created);
    }
    result
}

/// Return `true` if `path` exists.
pub fn exists(path: &Path) -> Result<bool> {
    global().read().exists(path)
}

/// Return basic metadata for `path`.
pub fn metadata(path: &Path) -> Result<FsMetadata> {
    global().read().metadata(path)
}

/// Return a flat manifest of every entry under `path`.
pub fn manifest(path: &Path) -> Result<Vec<ManifestEntry>> {
    global().read().manifest(path)
}

/// Return paths of all files under `root` whose extension matches `ext`
/// (case-insensitive, without leading dot). Paths are relative to `root`.
pub fn find_by_extension(root: &Path, ext: &str) -> Vec<std::path::PathBuf> {
    let ext_lower = ext.trim_start_matches('.').to_ascii_lowercase();
    match global().read().manifest(root) {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| !e.is_dir)
            .filter(|e| {
                std::path::Path::new(&e.path)
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.to_ascii_lowercase())
                    .as_deref()
                    == Some(ext_lower.as_str())
            })
            .map(|e| root.join(e.path))
            .collect(),
        Err(e) => {
            tracing::warn!("find_by_extension: manifest error for {:?}: {}", root, e);
            vec![]
        }
    }
}
