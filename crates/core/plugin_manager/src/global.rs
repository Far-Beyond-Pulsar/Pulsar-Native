//! The process-wide plugin manager and its lock-free file-type lookup.
//!
//! Entry points: [`initialize_global`] (once at startup), [`global`] and
//! [`file_type_for_path`].

use crate::{FileTypeLookup, PluginManager};
use once_cell::sync::OnceCell;
use parking_lot::RwLock;

/// Global plugin manager instance.
static GLOBAL_PLUGIN_MANAGER: OnceCell<RwLock<PluginManager>> = OnceCell::new();

/// Initialize the global plugin manager.
/// This should be called once at application startup.
pub fn initialize_global(manager: PluginManager) {
    let lookup = manager.file_type_registry().lookup_handle();
    if GLOBAL_PLUGIN_MANAGER.set(RwLock::new(manager)).is_err() {
        tracing::warn!("Global plugin manager already initialized");
    } else {
        // Only the manager that won the race publishes its lookup, so the
        // handle always belongs to the global manager.
        let _ = GLOBAL_FILE_TYPE_LOOKUP.set(lookup);
    }
}

/// The global manager's file-type lookup, kept current by its registry.
static GLOBAL_FILE_TYPE_LOOKUP: OnceCell<std::sync::Arc<arc_swap::ArcSwap<FileTypeLookup>>> =
    OnceCell::new();

/// The file type registered for `path`, without touching the plugin manager's
/// lock.
///
/// Prefer this over `global()?.read().file_type_registry()` anywhere that can
/// run while an editor is being created: `ui_core` holds the manager's write
/// lock for that whole time, and these locks are not reentrant, so a blocking
/// `read()` from inside the editor's own thread waits on itself forever.
/// Returns `None` until the manager is initialized.
pub fn file_type_for_path(path: &std::path::Path) -> Option<plugin_editor_api::FileTypeId> {
    GLOBAL_FILE_TYPE_LOOKUP
        .get()?
        .load()
        .file_type_for_path(path)
}

/// Get a handle to the global plugin manager.
///
/// `parking_lot`-backed — `.read()` / `.write()` return guards directly,
/// no `.unwrap()` needed.
pub fn global() -> Option<&'static RwLock<PluginManager>> {
    GLOBAL_PLUGIN_MANAGER.get()
}
