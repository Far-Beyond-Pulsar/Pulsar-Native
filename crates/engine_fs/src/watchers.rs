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
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            handle_fs_event(&event, &type_database);
        }
        // Keep watcher alive
        drop(watcher);
    });

    Ok(())
}

fn handle_fs_event(event: &Event, type_database: &TypeDatabase) {
    println!("Filesystem event: {:?}", event);

    match &event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            // File created or modified
            for path in &event.paths {
                if let Some((name, kind)) = get_type_info(path) {
                    type_database.register_with_path(
                        name.clone(),
                        path.clone(),
                        kind,
                        None,
                        Some(format!("{:?}: {}", kind, name)),
                        None,
                    );
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
    let name = path.file_stem()?.to_str()?.to_string();

    let kind = if extension == "alias" || (extension == "json" && file_name.contains("alias")) {
        TypeKind::Alias
    } else if extension == "struct" {
        TypeKind::Struct
    } else if extension == "enum" {
        TypeKind::Enum
    } else if extension == "trait" {
        TypeKind::Trait
    } else {
        return None;
    };

    Some((name, kind))
}
