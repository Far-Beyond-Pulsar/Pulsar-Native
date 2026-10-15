//! File System Watchers
//!
//! Monitors file changes and automatically updates indexes
//!
//! Note: Currently disabled registry-based type detection in watchers due to Send trait limitations.
//! Type detection is still handled during project scans and manual operations.

use anyhow::Result;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::asset_index::AssetIndex;
use crate::file_index::ProjectFileIndex;
use crate::user_types::UserTypeRegistry;

static FILE_INDEX_WATCHERS: OnceLock<parking_lot::Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// Start one local watcher that keeps a persistent file index current even
/// when the broader AssetIndex/EngineFs service is not active.
pub fn start_file_index_watcher(
    project_root: PathBuf,
    file_index: Arc<ProjectFileIndex>,
) -> Result<()> {
    let watched = FILE_INDEX_WATCHERS.get_or_init(|| parking_lot::Mutex::new(HashSet::new()));
    {
        let mut watched = watched.lock();
        if watched.contains(&project_root) {
            return Ok(());
        }
        watched.insert(project_root.clone());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match notify::recommended_watcher(move |res: Result<Event, _>| {
        if let Ok(event) = res {
            let _ = tx.send(event);
        }
    }) {
        Ok(watcher) => watcher,
        Err(error) => {
            watched.lock().remove(&project_root);
            return Err(error.into());
        }
    };
    if let Err(error) = watcher.watch(&project_root, RecursiveMode::Recursive) {
        watched.lock().remove(&project_root);
        return Err(error.into());
    }

    let thread = std::thread::Builder::new()
        .name("Project File Index Watcher".to_owned())
        .spawn(move || {
            while let Ok(event) = rx.recv() {
                if matches!(
                    event.kind,
                    EventKind::Modify(ModifyKind::Name(RenameMode::Both))
                ) && event.paths.len() >= 2
                {
                    if let Err(error) = file_index.move_path(&event.paths[0], &event.paths[1]) {
                        tracing::warn!(%error, from = %event.paths[0].display(), to = %event.paths[1].display(), "Could not preserve file-index IDs across external rename");
                    }
                    continue;
                }
                for path in &event.paths {
                    let result = match &event.kind {
                        EventKind::Remove(_) => file_index.remove_path(path),
                        _ => file_index.refresh_path(path),
                    };
                    if let Err(error) = result {
                        tracing::warn!(%error, path = %path.display(), "Could not update project file index from filesystem event");
                    }
                }
            }
            drop(watcher);
        });
    if let Err(error) = thread {
        watched.lock().remove(&project_root);
        return Err(error.into());
    }
    Ok(())
}

/// Start watching the project directory for changes
///
/// Note: Currently only handles file removal events. File creation/modification detection
/// requires plugin registry access which isn't thread-safe yet.
pub fn start_watcher(
    project_root: PathBuf,
    asset_index: Arc<AssetIndex>,
    user_types: Arc<UserTypeRegistry>,
    file_index: Option<Arc<ProjectFileIndex>>,
) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
        if let Ok(event) = res {
            let _ = tx.send(event);
        }
    })?;

    // Watch the project root
    watcher.watch(&project_root, RecursiveMode::Recursive)?;

    // Spawn thread to handle events
    std::thread::Builder::new()
        .name("FS Watcher".to_string())
        .spawn(move || {
            profiling::set_thread_name("FS Watcher");
            while let Ok(event) = rx.recv() {
                profiling::profile_scope!("fs_event_handle");
                handle_fs_event(&event, &asset_index, &user_types, file_index.as_deref());
            }
            // Keep watcher alive
            drop(watcher);
        })?;

    Ok(())
}

fn handle_fs_event(
    event: &Event,
    asset_index: &AssetIndex,
    user_types: &UserTypeRegistry,
    file_index: Option<&ProjectFileIndex>,
) {
    profiling::profile_scope!("handle_fs_event");
    tracing::debug!("Filesystem event: {:?}", event);

    match &event.kind {
        EventKind::Remove(_) => {
            // File removed - we can safely unregister
            for path in &event.paths {
                asset_index.unregister_by_path(path);
                user_types.unregister_by_path(path);
                if let Some(index) = file_index {
                    if let Err(error) = index.remove_path(path) {
                        tracing::warn!(%error, path = %path.display(), "Failed to remove path from project file index");
                    }
                }
            }
        }
        EventKind::Create(_) | EventKind::Modify(_) => {
            // File created/modified - log for now, user needs to rescan
            // TODO: Once PluginManager is Send-safe, integrate registry-based detection here
            for path in &event.paths {
                if let Some(index) = file_index {
                    if let Err(error) = index.refresh_path(path) {
                        tracing::warn!(%error, path = %path.display(), "Failed to refresh project file index entry");
                    }
                }
                tracing::info!("File changed: {:?} - rescan project to update index", path);
                publish_asset_update(path);
            }
        }
        _ => {}
    }
}

/// An image or material written on disk is an asset update: every world
/// showing it reloads it (SceneDB#84). A shader graph folder's save is its
/// material's update. Other kinds publish their own updates when they are
/// written (a re-imported mesh, a saved class).
fn publish_asset_update(path: &std::path::Path) {
    use ui_types_common::AssetKind;
    let extension = path.extension().and_then(|ext| ext.to_str());
    // Surface materials are saved as `.mat` (mesh slots' extracted
    // materials), which the file-type table does not list.
    let kind = if path
        .file_name()
        .is_some_and(|name| name == "shader_graph_save.json")
        || extension.is_some_and(|ext| ext.eq_ignore_ascii_case("mat"))
    {
        AssetKind::Material
    } else {
        match extension.map(AssetKind::from_extension) {
            Some(kind @ (AssetKind::Texture | AssetKind::Material)) => kind,
            _ => return,
        }
    };
    if path.is_file() {
        let mut update = plugin_editor_api::AssetUpdated::new(kind);
        update.path = Some(path.to_path_buf());
        plugin_editor_api::publish_asset_updated(update);
    }
}
