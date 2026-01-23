//! File System Watchers
//!
//! Monitors file changes and automatically updates indexes

use anyhow::Result;
use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::path::PathBuf;
use std::sync::Arc;
use type_db::{TypeDatabase, TypeKind};

/// Start watching the project directory for changes
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
        EventKind::Create(_) | EventKind::Modify(_) => {
            // File created or modified
            for path in &event.paths {
                if let Some((name, kind)) = get_type_info(path) {
                    if let Err(e) = type_database.register_with_path(
                        name.clone(),
                        path.clone(),
                        kind,
                        None,
                        Some(format!("{:?}: {}", kind, name)),
                        None,
                    ) {
                        tracing::warn!("Failed to register type '{}': {:?}", name, e);
                    }
                }
            }
        }
        EventKind::Remove(_) => {
            // File removed
            for path in &event.paths {
                type_database.unregister_by_path(path);
            }
        }
        _ => {}
    }
}

/// Extract type information from a file path
/// Returns (name, TypeKind) if the file is a recognized type file
fn get_type_info(path: &PathBuf) -> Option<(String, TypeKind)> {
    let extension = path.extension()?.to_str()?;
    let file_name = path.file_name()?.to_str()?;

    // For JSON files, check the filename to determine type
    if extension == "json" {
        // Get parent folder name as the type name
        let type_name = path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())?
            .to_string();

        let kind = match file_name {
            "struct.json" => TypeKind::Struct,
            "enum.json" => TypeKind::Enum,
            "trait.json" => TypeKind::Trait,
            _ if file_name.contains("alias") => TypeKind::Alias,
            _ => return None,
        };

        return Some((type_name, kind));
    }

    None
}
