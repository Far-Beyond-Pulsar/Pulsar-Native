//! Virtual filesystem global — routes all file I/O to the correct backend.
//!
//! Call [`set_provider`] once (e.g. when opening a cloud project) to switch
//! from the default local-disk provider to a remote one.  The rest of the
//! editor uses the free functions in this module and never needs to care
//! which backend is active.

use std::path::Path;
use std::sync::{Arc, OnceLock};
use parking_lot::RwLock;
use anyhow::Result;

use crate::providers::{FsEntry, FsMetadata, FsProvider, LocalFsProvider, ManifestEntry};

pub mod path_utils;

// Re-export commonly used path utilities
pub use path_utils::{is_cloud_path, normalize_path, cloud_join};

// ── Singleton ─────────────────────────────────────────────────────────────────

static VIRTUAL_FS: OnceLock<Arc<RwLock<Arc<dyn FsProvider>>>> = OnceLock::new();

fn global() -> &'static Arc<RwLock<Arc<dyn FsProvider>>> {
    VIRTUAL_FS.get_or_init(|| {
        Arc::new(RwLock::new(Arc::new(LocalFsProvider)))
    })
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
    set_provider(Arc::new(LocalFsProvider));
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
    global().read().write_file(path, content)
}

/// Create a new file with `content`, failing if it already exists.
pub fn create_file(path: &Path, content: &[u8]) -> Result<()> {
    global().read().create_file(path, content)
}

/// Delete a file or recursively remove a directory.
pub fn delete_path(path: &Path) -> Result<()> {
    global().read().delete_path(path)
}

/// Rename / move a path.
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    global().read().rename(from, to)
}

/// List the immediate children of a directory.
pub fn list_dir(path: &Path) -> Result<Vec<FsEntry>> {
    global().read().list_dir(path)
}

/// Recursively create a directory and all missing parents.
pub fn create_dir_all(path: &Path) -> Result<()> {
    global().read().create_dir_all(path)
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
