//! File System Watchers
//!
//! Monitors file changes and automatically updates indexes
//! 
//! Note: Currently disabled registry-based type detection in watchers due to Send trait limitations.
//! Type detection is still handled during project scans and manual operations.

use anyhow::Result;
use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::path::PathBuf;
use std::sync::Arc;
use type_db::TypeDatabase;

/// Start watching the project directory for changes
/// 
/// Note: Currently only handles file removal events. File creation/modification detection
/// requires plugin registry access which isn't thread-safe yet.
pub fn start_watcher(
    project_root: PathBuf,
    type_database: Arc<TypeDatabase>,
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
                handle_fs_event(&event, &type_database);
            }
            // Keep watcher alive
            drop(watcher);
        })?;

    Ok(())
}

fn handle_fs_event(event: &Event, type_database: &TypeDatabase) {
    profiling::profile_scope!("handle_fs_event");
    tracing::debug!("Filesystem event: {:?}", event);

    match &event.kind {
        EventKind::Remove(_) => {
            // File removed - we can safely unregister
            for path in &event.paths {
                type_database.unregister_by_path(path);
            }
        }
        EventKind::Create(_) | EventKind::Modify(_) => {
            // File created/modified - log for now, user needs to rescan
            // TODO: Once PluginManager is Send-safe, integrate registry-based detection here
            for path in &event.paths {
                tracing::info!("File changed: {:?} - rescan project to update index", path);
            }
        }
        _ => {}
    }
}
