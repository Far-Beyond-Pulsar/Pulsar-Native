//! File System Watchers
//!
//! Monitors file changes and automatically updates indexes

use anyhow::Result;
use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::path::PathBuf;
use std::sync::Arc;
use super::{AssetRegistry, TypeAliasIndex};

/// Start watching the project directory for changes
pub fn start_watcher(
    project_root: PathBuf,
    _registry: Arc<AssetRegistry>,
    type_index: Arc<TypeAliasIndex>,
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
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            handle_fs_event(&event, &type_index);
        }
        // Keep watcher alive
        drop(watcher);
    });
    
    Ok(())
}

fn handle_fs_event(event: &Event, type_index: &TypeAliasIndex) {
    match &event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            // File created or modified
            for path in &event.paths {
                if is_alias_file(path) {
                    if let Err(e) = type_index.register(path) {
                        eprintln!("Failed to register alias {}: {}", path.display(), e);
                    }
                }
            }
        }
        EventKind::Remove(_) => {
            // File removed
            for path in &event.paths {
                if is_alias_file(path) {
                    type_index.unregister_by_path(path);
                }
            }
        }
        _ => {}
    }
}

fn is_alias_file(path: &PathBuf) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "alias" || e == "json")
        .unwrap_or(false)
        && path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.contains("alias"))
            .unwrap_or(false)
}
