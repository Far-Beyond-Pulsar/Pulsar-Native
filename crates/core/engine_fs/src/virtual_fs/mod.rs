//! Virtual filesystem global: routes all file I/O to the correct backend.
//!
//! Call [`set_provider`] once (e.g. when opening a cloud project) to switch
//! from the default local-disk provider to a remote one. The rest of the
//! engine, the editor and every plugin use the free functions in this module
//! and never need to care which backend is active.
//!
//! Every call goes through the plugin folders first ([`mounts`]): a mounted
//! project's `plugins/<owner>/` folders are served by the plugins that
//! provide them (and the engine, `plugins/pulsar/`), whatever the provider,
//! and refuse every writer but their owner with [`ReadOnlyMount`].
//!
//! This is the one place engine and plugin code touches files: `std::fs` is
//! a disallowed method across the workspace (the root `clippy.toml`), allowed
//! only in the providers, build scripts and tests
//! (`architecture/std-fs-allowed.txt`).

use anyhow::Result;
use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::providers::{
    FsEntry, FsMetadata, FsProvider, FsWriter, LocalFsProvider, ManifestEntry, ReadSeek,
};
use crate::{events, FsChangeKind};

pub mod mounts;
pub mod path_utils;

pub use mounts::{
    canonical_mount_path, folder_segments, is_read_only, mount_of, mount_owner_id, mount_problems,
    mount_project, plugin_snapshot, provided_files, serve_packaged_folders, unmount_project,
    CacheFolder, EmbeddedFile, FileProvider, FolderSource, InGame, MemoryFolder, MountedFs,
    PluginFolder, ReadOnlyMount, CACHE_FOLDER, ENGINE_FOLDER, ENGINE_FOLDER_NAME, PLUGINS_FOLDER,
};
// Re-export commonly used path utilities
pub use path_utils::{cloud_join, is_cloud_path, normalize_path};

// ── Singleton ─────────────────────────────────────────────────────────────────

/// The active provider, already wrapped in the mount layer.
static VIRTUAL_FS: OnceLock<RwLock<Arc<MountedFs>>> = OnceLock::new();

fn global() -> &'static RwLock<Arc<MountedFs>> {
    VIRTUAL_FS
        .get_or_init(|| RwLock::new(Arc::new(MountedFs::new(Arc::new(LocalFsProvider::new())))))
}

/// The active layer, taken out of the lock: a provider call may emit events
/// whose handlers call back into this module (the file index refreshing a
/// path), and the lock is not reentrant.
fn layer() -> Arc<MountedFs> {
    global().read().clone()
}

// ── Configuration ─────────────────────────────────────────────────────────────

/// Replace the active filesystem provider. The mounts stay composed over it.
///
/// Call this before opening the editor when connecting to a cloud project.
/// The change is process-wide and takes effect immediately.
pub fn set_provider(provider: Arc<dyn FsProvider>) {
    *global().write() = Arc::new(MountedFs::new(provider));
}

/// Restore the default local-disk provider (e.g. when disconnecting).
pub fn reset_to_local() {
    set_provider(Arc::new(LocalFsProvider::new()));
}

/// The active provider with the mounts composed over it (cloned `Arc`:
/// cheap). What bulk users call; the free functions below go through it too.
pub fn provider() -> Arc<dyn FsProvider> {
    layer()
}

/// The active provider itself, which knows no mounts (for the mounts' own
/// checks of what the project has on disk).
pub fn base_provider() -> Arc<dyn FsProvider> {
    Arc::clone(layer().base())
}

// ── Introspection ─────────────────────────────────────────────────────────────

/// `true` when the active provider serves a remote filesystem.
pub fn is_remote() -> bool {
    layer().is_remote()
}

/// Human-readable label for the current provider ("Local" or "Remote").
pub fn current_label() -> String {
    layer().label().to_string()
}

// ── Convenience pass-throughs ─────────────────────────────────────────────────
//
// Every wrapper clones the `Arc` out of the lock (`provider()`) and calls the
// provider through the clone, never while holding the read guard: providers
// emit filesystem events synchronously, and the event handlers (file index
// updates) call back into these wrappers. A re-entrant `read()` deadlocks as
// soon as a writer (`set_provider`) is queued, because the lock is fair.

/// Read the full contents of a file.
#[track_caller]
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    editor_task_queue::blocking_call("virtual_fs::read_file");
    layer().read_file(path)
}

/// Read up to `len` bytes from the start of a file.
#[track_caller]
pub fn read_file_prefix(path: &Path, len: usize) -> Result<Vec<u8>> {
    editor_task_queue::blocking_call("virtual_fs::read_file_prefix");
    layer().read_file_prefix(path, len)
}

/// Overwrite (or create) a file with `content`.
#[track_caller]
pub fn write_file(path: &Path, content: &[u8]) -> Result<()> {
    editor_task_queue::blocking_call("virtual_fs::write_file");
    let result = layer().write_file(path, content);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Modified);
    }
    result
}

/// Atomically replace a file when the active provider supports it.
///
/// Providers that cannot guarantee atomic replacement return an error rather
/// than silently falling back to a partial write.
#[track_caller]
pub fn write_file_atomically(path: &Path, content: &[u8]) -> Result<()> {
    editor_task_queue::blocking_call("virtual_fs::write_file_atomically");
    let result = layer().write_file_atomically(path, content);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Modified);
    }
    result
}

/// Create a new file with `content`, failing if it already exists.
#[track_caller]
pub fn create_file(path: &Path, content: &[u8]) -> Result<()> {
    editor_task_queue::blocking_call("virtual_fs::create_file");
    let result = layer().create_file(path, content);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Created);
    }
    result
}

/// Delete a file or recursively remove a directory.
#[track_caller]
pub fn delete_path(path: &Path) -> Result<()> {
    editor_task_queue::blocking_call("virtual_fs::delete_path");
    let result = layer().delete_path(path);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Deleted);
    }
    result
}

/// Rename / move a path.
#[track_caller]
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    editor_task_queue::blocking_call("virtual_fs::rename");
    layer().rename(from, to)
}

/// Read a file as UTF-8 text.
#[track_caller]
pub fn read_to_string(path: &Path) -> Result<String> {
    let bytes = read_file(path)?;
    String::from_utf8(bytes)
        .map_err(|error| anyhow::anyhow!("'{}' is not UTF-8 text: {error}", path.display()))
}

/// Add `content` to the end of a file, creating it if needed.
#[track_caller]
pub fn append_file(path: &Path, content: &[u8]) -> Result<()> {
    editor_task_queue::blocking_call("virtual_fs::append_file");
    layer().append_file(path, content)
}

/// Copy the file `from` to `to`, replacing `to`.
#[track_caller]
pub fn copy(from: &Path, to: &Path) -> Result<()> {
    editor_task_queue::blocking_call("virtual_fs::copy");
    let result = layer().copy(from, to);
    if result.is_ok() {
        events::emit(to.to_path_buf(), FsChangeKind::Modified);
    }
    result
}

/// Open a file for streamed reading.
#[track_caller]
pub fn open_read(path: &Path) -> Result<Box<dyn ReadSeek>> {
    editor_task_queue::blocking_call("virtual_fs::open_read");
    layer().open_read(path)
}

/// Open a file for streamed writing (created or truncated). The file holds
/// what was written once [`FsWriter::finish`] returns `Ok`.
#[track_caller]
pub fn open_write(path: &Path) -> Result<FsWriter> {
    editor_task_queue::blocking_call("virtual_fs::open_write");
    let layer = layer();
    if let Some(stream) = layer.open_write_stream(path)? {
        return Ok(FsWriter::direct(stream));
    }
    let path = path.to_path_buf();
    Ok(FsWriter::buffered(move |bytes| {
        layer.write_file(&path, &bytes)
    }))
}

/// Whether `path` is a directory (`false` when it does not exist).
#[track_caller]
pub fn is_dir(path: &Path) -> bool {
    metadata(path).map(|meta| meta.is_dir).unwrap_or(false)
}

/// Whether `path` is a file (`false` when it does not exist).
#[track_caller]
pub fn is_file(path: &Path) -> bool {
    metadata(path).map(|meta| !meta.is_dir).unwrap_or(false)
}

/// Resolve `path` to its canonical absolute form.
#[track_caller]
pub fn canonicalize(path: &Path) -> Result<PathBuf> {
    editor_task_queue::blocking_call("virtual_fs::canonicalize");
    layer().canonicalize(path)
}

/// Whether `path` is a symbolic link or another filesystem redirection.
#[track_caller]
pub fn is_symlink(path: &Path) -> Result<bool> {
    editor_task_queue::blocking_call("virtual_fs::is_symlink");
    layer().is_symlink(path)
}

/// List the immediate children of a directory.
#[track_caller]
pub fn list_dir(path: &Path) -> Result<Vec<FsEntry>> {
    editor_task_queue::blocking_call("virtual_fs::list_dir");
    layer().list_dir(path)
}

/// Recursively create a directory and all missing parents.
#[track_caller]
pub fn create_dir_all(path: &Path) -> Result<()> {
    editor_task_queue::blocking_call("virtual_fs::create_dir_all");
    let result = layer().create_dir_all(path);
    if result.is_ok() {
        events::emit(path.to_path_buf(), FsChangeKind::Created);
    }
    result
}

/// Return `true` if `path` exists.
#[track_caller]
pub fn exists(path: &Path) -> Result<bool> {
    editor_task_queue::blocking_call("virtual_fs::exists");
    layer().exists(path)
}

/// Return basic metadata for `path`.
#[track_caller]
pub fn metadata(path: &Path) -> Result<FsMetadata> {
    editor_task_queue::blocking_call("virtual_fs::metadata");
    layer().metadata(path)
}

/// Return a flat manifest of every entry under `path`.
#[track_caller]
pub fn manifest(path: &Path) -> Result<Vec<ManifestEntry>> {
    editor_task_queue::blocking_call("virtual_fs::manifest");
    layer().manifest(path)
}

/// Return paths of all files under `root` whose extension matches `ext`
/// (case-insensitive, without leading dot). Paths are relative to `root`.
#[track_caller]
pub fn find_by_extension(root: &Path, ext: &str) -> Vec<PathBuf> {
    editor_task_queue::blocking_call("virtual_fs::find_by_extension");
    let ext_lower = ext.trim_start_matches('.').to_ascii_lowercase();
    match layer().manifest(root) {
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
