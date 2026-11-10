//! The local project watcher: keeps the persistent file index current when
//! files change outside the engine (version control, another program) and
//! publishes the asset updates plugins ask for ([`AssetChangeKind`]).

use anyhow::Result;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::file_index::ProjectFileIndex;

static FILE_INDEX_WATCHERS: OnceLock<parking_lot::Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// Start the one local watcher of `project_root`, keeping its file index
/// current (the index opens it, once per project).
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
                        EventKind::Create(_) | EventKind::Modify(_) => {
                            let refreshed = file_index.refresh_path(path);
                            publish_asset_update(path);
                            refreshed
                        }
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

/// What kind of asset update a changed file is, for the plugins whose files
/// reload when they change on disk (an image, a material, an animation): the
/// file index watcher publishes an [`AssetUpdated`](pulsar_events::AssetUpdated)
/// of the first kind a registration names. A kind that publishes its own
/// updates when it writes (a re-imported mesh, a saved class) registers none,
/// so nothing reloads twice.
pub struct AssetChangeKind {
    pub origin: &'static str,
    /// Stable within its plugin (`"animation.files"`).
    pub id: &'static str,
    pub kind_of: fn(&Path) -> Option<ui_types_common::AssetKind>,
}

impl pulsar_plugins::Contribution for AssetChangeKind {
    const CATEGORY: &'static str = "fs.asset_changes";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_owned()
    }
}

pulsar_plugins::registration!(AssetChangeKind);

/// Publish the asset update a file changed on disk is, if a plugin names one.
fn publish_asset_update(path: &Path) {
    let Some(kind) =
        pulsar_plugins::registered::<AssetChangeKind>().find_map(|change| (change.kind_of)(path))
    else {
        return;
    };
    if crate::virtual_fs::is_file(path) {
        pulsar_events::publish_asset_updated(pulsar_events::AssetUpdated::new(kind).with_path(path));
    }
}
