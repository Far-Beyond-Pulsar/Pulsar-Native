//! File System Watchers
//!
//! Monitors file changes and automatically updates indexes
//!
//! Note: Currently disabled registry-based type detection in watchers due to Send trait limitations.
//! Type detection is still handled during project scans and manual operations.

use anyhow::Result;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;

use crate::asset_index::AssetIndex;
use crate::user_types::UserTypeRegistry;

/// Start watching the project directory for changes
///
/// Note: Currently only handles file removal events. File creation/modification detection
/// requires plugin registry access which isn't thread-safe yet.
pub fn start_watcher(
    project_root: PathBuf,
    asset_index: Arc<AssetIndex>,
    user_types: Arc<UserTypeRegistry>,
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
                handle_fs_event(&event, &asset_index, &user_types);
            }
            // Keep watcher alive
            drop(watcher);
        })?;

    Ok(())
}

fn handle_fs_event(event: &Event, asset_index: &AssetIndex, user_types: &UserTypeRegistry) {
    profiling::profile_scope!("handle_fs_event");
    tracing::debug!("Filesystem event: {:?}", event);

    match &event.kind {
        EventKind::Remove(_) => {
            // File removed - we can safely unregister
            for path in &event.paths {
                asset_index.unregister_by_path(path);
                user_types.unregister_by_path(path);
            }
        }
        EventKind::Create(_) | EventKind::Modify(_) => {
            // File created/modified - log for now, user needs to rescan
            // TODO: Once PluginManager is Send-safe, integrate registry-based detection here
            for path in &event.paths {
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
