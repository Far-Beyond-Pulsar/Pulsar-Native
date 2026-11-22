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
    let filter_root = project_root.clone();
    let mut watcher = match notify::recommended_watcher(move |res: Result<Event, _>| {
        let Ok(mut event) = res else {
            return;
        };
        // A build writes tens of thousands of files under `target/`; none of
        // them is indexed, so drop them here rather than queue them.
        event
            .paths
            .retain(|path| !crate::file_index::is_unindexed_path(&filter_root, path));
        if !event.paths.is_empty() {
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

    let thread = editor_task_queue::spawn_service(
        editor_task_queue::ServiceDescription::new(
            format!("Project file index watcher: {}", project_root.display()),
            "fs",
            "Project File Index Watcher",
            editor_task_queue::ThreadRole::Service,
        )
        .owner("engine_fs")
        .no_heartbeat(),
        move |ctx| {
            let _watcher = watcher;
            loop {
                ctx.set_idle(true);
                let Ok(first) = rx.recv() else {
                    return;
                };
                ctx.set_idle(false);
                ctx.set_phase("Applying file changes");
                // A save or a copy arrives as a burst; take what is queued and
                // apply each path once.
                let mut pending = vec![first];
                pending.extend(rx.try_iter());
                apply_events(&file_index, coalesce(pending));
                ctx.heartbeat();
            }
        },
    );
    if let Err(error) = thread {
        watched.lock().remove(&project_root);
        return Err(error.into());
    }
    Ok(())
}

/// One change the watcher applies to the index.
#[derive(Debug, PartialEq, Eq)]
enum IndexOp {
    Move(PathBuf, PathBuf),
    Remove(PathBuf),
    /// Created or modified: refresh, and publish the asset update.
    Changed(PathBuf),
    /// Anything else notify reports (access, metadata): refresh only.
    Refresh(PathBuf),
}

impl IndexOp {
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Move(..) => None,
            Self::Remove(path) | Self::Changed(path) | Self::Refresh(path) => Some(path),
        }
    }
}

/// The burst's events as index operations, each path once: its last
/// operation wins. A move is a barrier (operations are not merged across
/// it), so a rename between two writes keeps its order.
fn coalesce(events: Vec<Event>) -> Vec<IndexOp> {
    let mut ops = Vec::new();
    for event in events {
        if matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Name(RenameMode::Both))
        ) && event.paths.len() >= 2
        {
            let mut paths = event.paths.into_iter();
            if let (Some(from), Some(to)) = (paths.next(), paths.next()) {
                ops.push(IndexOp::Move(from, to));
            }
            continue;
        }
        for path in event.paths {
            ops.push(match event.kind {
                EventKind::Remove(_) => IndexOp::Remove(path),
                EventKind::Create(_) | EventKind::Modify(_) => IndexOp::Changed(path),
                _ => IndexOp::Refresh(path),
            });
        }
    }
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut kept = Vec::with_capacity(ops.len());
    for op in ops.into_iter().rev() {
        match op.path() {
            None => seen.clear(),
            Some(path) if !seen.insert(path.to_path_buf()) => continue,
            Some(_) => {}
        }
        kept.push(op);
    }
    kept.reverse();
    kept
}

fn apply_events(file_index: &ProjectFileIndex, ops: Vec<IndexOp>) {
    for op in ops {
        match op {
            IndexOp::Move(from, to) => {
                if let Err(error) = file_index.move_path(&from, &to) {
                    tracing::warn!(%error, from = %from.display(), to = %to.display(), "Could not preserve file-index IDs across external rename");
                }
            }
            IndexOp::Remove(path) => {
                if let Err(error) = file_index.remove_path(&path) {
                    tracing::warn!(%error, path = %path.display(), "Could not update project file index from filesystem event");
                }
            }
            IndexOp::Changed(path) => {
                if let Err(error) = file_index.refresh_path(&path) {
                    tracing::warn!(%error, path = %path.display(), "Could not update project file index from filesystem event");
                }
                publish_asset_update(&path);
            }
            IndexOp::Refresh(path) => {
                if let Err(error) = file_index.refresh_path(&path) {
                    tracing::warn!(%error, path = %path.display(), "Could not update project file index from filesystem event");
                }
            }
        }
    }
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
        pulsar_events::publish_asset_updated(
            pulsar_events::AssetUpdated::new(kind).with_path(path),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RemoveKind};

    fn event(kind: EventKind, paths: &[&str]) -> Event {
        let mut event = Event::new(kind);
        event.paths = paths.iter().map(PathBuf::from).collect();
        event
    }

    #[test]
    fn build_output_is_never_indexed() {
        let root = Path::new("/project");
        for path in [
            "/project/target/debug/deps/libfoo.rlib",
            "/project/target",
            "/project/scripts/target/debug/x",
            "/project/.git/index",
            "/project/node_modules/a/b.js",
        ] {
            assert!(
                crate::file_index::is_unindexed_path(root, Path::new(path)),
                "{path}"
            );
        }
        for path in [
            "/project/src/main.rs",
            "/project/Content/targets.png",
            "/elsewhere/target/x",
        ] {
            assert!(
                !crate::file_index::is_unindexed_path(root, Path::new(path)),
                "{path}"
            );
        }
    }

    #[test]
    fn a_burst_applies_each_path_once_with_its_last_change() {
        let ops = coalesce(vec![
            event(EventKind::Create(CreateKind::File), &["/p/a"]),
            event(EventKind::Modify(ModifyKind::Any), &["/p/a"]),
            event(EventKind::Modify(ModifyKind::Any), &["/p/b"]),
            event(EventKind::Remove(RemoveKind::File), &["/p/a"]),
            event(EventKind::Modify(ModifyKind::Any), &["/p/b"]),
        ]);
        assert_eq!(
            ops,
            vec![
                IndexOp::Remove(PathBuf::from("/p/a")),
                IndexOp::Changed(PathBuf::from("/p/b")),
            ]
        );
    }

    #[test]
    fn a_rename_keeps_its_place_in_the_burst() {
        let ops = coalesce(vec![
            event(EventKind::Modify(ModifyKind::Any), &["/p/a"]),
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &["/p/a", "/p/b"],
            ),
            event(EventKind::Modify(ModifyKind::Any), &["/p/a"]),
        ]);
        assert_eq!(
            ops,
            vec![
                IndexOp::Changed(PathBuf::from("/p/a")),
                IndexOp::Move(PathBuf::from("/p/a"), PathBuf::from("/p/b")),
                IndexOp::Changed(PathBuf::from("/p/a")),
            ]
        );
    }
}
