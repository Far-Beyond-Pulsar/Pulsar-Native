//! The mounted plugin folders: opening each provider's source once, and the
//! tree of folders (live or packaged) the layer answers from.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

use super::*;

/// Serving packaged snapshots ([`serve_packaged_folders`]).
pub(super) static PACKAGED: AtomicBool = AtomicBool::new(false);

/// Serve every [`InGame::Snapshot`] folder from the packaged game's content
/// (the snapshot the packager wrote at `plugins/<visible name>/`) instead of
/// opening its plugin. The content root is separate from the project's path,
/// which can point at an extracted asset cache.
pub fn serve_packaged_folders(content_root: Option<PathBuf>) {
    *packaged_content_root().write() = content_root.clone();
    PACKAGED.store(content_root.is_some(), Ordering::Release);
}

fn packaged_content_root() -> &'static RwLock<Option<PathBuf>> {
    static ROOT: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();
    ROOT.get_or_init(|| RwLock::new(None))
}

/// Every source opened so far, by folder: each provider is opened once per
/// process and keeps its content across a plugin being disabled and enabled.
fn opened() -> &'static RwLock<HashMap<String, Arc<dyn FolderSource>>> {
    static OPENED: OnceLock<RwLock<HashMap<String, Arc<dyn FolderSource>>>> = OnceLock::new();
    OPENED.get_or_init(|| RwLock::new(HashMap::new()))
}

/// A mounted folder, in [`Tree::folders`] under its visible name.
pub(super) struct Folder {
    /// The owning plugin's id (the legacy alias).
    pub(super) id: String,
    pub(super) source: Arc<dyn FolderSource>,
    pub(super) in_game: InGame,
}

/// The folders mounted for one plugin generation, by visible name.
pub(super) struct Tree {
    pub(super) generation: u64,
    pub(super) packaged: bool,
    pub(super) packaged_root: Option<PathBuf>,
    pub(super) folders: BTreeMap<String, Folder>,
}

fn build_tree(generation: u64, packaged: bool, packaged_root: Option<PathBuf>) -> Tree {
    let mut folders: BTreeMap<String, Folder> = BTreeMap::new();
    let mut owners: BTreeMap<String, &'static str> = BTreeMap::new();
    let segments = folder_segments();
    for provider in pulsar_plugins::registered::<FileProvider>() {
        let Some(owner) = owner_folder(provider.origin) else {
            tracing::error!(
                "a file provider comes from `{}`, which is neither the engine nor an installed plugin's root; it is not mounted",
                provider.origin
            );
            continue;
        };
        if let Some(first) = owners.get(&owner) {
            tracing::error!(
                "`{}` registers a second file provider for plugins/{owner}/ (the first is `{first}`); it is not mounted",
                provider.origin
            );
            continue;
        }
        owners.insert(owner.clone(), provider.origin);
        let segment = segments
            .get(&owner)
            .cloned()
            .unwrap_or_else(|| owner.clone());
        let source: Arc<dyn FolderSource> = if packaged && provider.in_game == InGame::Snapshot {
            Arc::new(PackagedFolder {
                owner: owner.clone(),
                segment: segment.clone(),
                content_root: packaged_root.clone(),
            })
        } else {
            source_of(provider, &owner, &segment)
        };
        folders.insert(
            segment,
            Folder {
                id: owner,
                source,
                in_game: provider.in_game,
            },
        );
    }
    Tree {
        generation,
        packaged,
        packaged_root,
        folders,
    }
}

/// `provider`'s source for the folder `owner`: opened the first time (with
/// the capability to change it), the same one after.
fn source_of(provider: &FileProvider, owner: &str, segment: &str) -> Arc<dyn FolderSource> {
    if let Some(source) = opened().read().get(owner).cloned() {
        return source;
    }
    let source = (provider.open)(PluginFolder {
        owner: Arc::from(owner),
        segment: Arc::from(segment),
    });
    opened()
        .write()
        .entry(owner.to_owned())
        .or_insert(source)
        .clone()
}

/// The files `provider` serves in every project (its embedded content),
/// as `(path inside its folder, bytes)`, whether or not its plugin is
/// enabled: what the plugin catalog lists of a plugin's files (its traits).
/// A source whose files are per project lists none.
pub fn provided_files(
    provider: &FileProvider,
    wanted: impl Fn(&str) -> bool,
) -> Vec<(String, Vec<u8>)> {
    let Some(owner) = owner_folder(provider.origin) else {
        return Vec::new();
    };
    let segment = folder_segments()
        .get(&owner)
        .cloned()
        .unwrap_or_else(|| owner.clone());
    let source = source_of(provider, &owner, &segment);
    if !source.project_independent() {
        return Vec::new();
    }
    let project = Path::new("");
    source
        .manifest(project, "")
        .into_iter()
        .filter(|entry| !entry.is_dir && wanted(&entry.path))
        .filter_map(|entry| {
            let bytes = source.read_file(project, &entry.path).ok()?;
            Some((entry.path, bytes))
        })
        .collect()
}

static TREE: OnceLock<RwLock<Option<Arc<Tree>>>> = OnceLock::new();

/// The folders mounted for the plugin set in effect now (rebuilt when it
/// changes).
pub(super) fn tree() -> Arc<Tree> {
    let cell = TREE.get_or_init(|| RwLock::new(None));
    let generation = pulsar_plugins::generation();
    let packaged = PACKAGED.load(Ordering::Acquire);
    let packaged_root = packaged_content_root().read().clone();
    let current = |tree: &Tree| {
        tree.generation == generation
            && tree.packaged == packaged
            && tree.packaged_root == packaged_root
    };
    if let Some(tree) = cell.read().as_ref() {
        if current(tree) {
            return Arc::clone(tree);
        }
    }
    // One build at a time, so each provider is opened once.
    static BUILDING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _building = BUILDING.lock();
    if let Some(tree) = cell.read().as_ref() {
        if current(tree) {
            return Arc::clone(tree);
        }
    }
    let fresh = Arc::new(build_tree(generation, packaged, packaged_root));
    *cell.write() = Some(Arc::clone(&fresh));
    fresh
}

/// Every file of every folder the packager snapshots ([`InGame::Snapshot`])
/// in the project at `project`, as `(project-relative path, bytes)`
/// (`plugins/<visible name>/...`): what a packaged game serves for them.
pub fn plugin_snapshot(project: &Path) -> Vec<(String, Vec<u8>)> {
    mount_project(project);
    let tree = tree();
    let mut out = Vec::new();
    for (owner, folder) in &tree.folders {
        if folder.in_game != InGame::Snapshot {
            continue;
        }
        for entry in folder.source.manifest(project, "") {
            if entry.is_dir {
                continue;
            }
            match folder.source.read_file(project, &entry.path) {
                Ok(bytes) => out.push((format!("{PLUGINS_FOLDER}/{owner}/{}", entry.path), bytes)),
                Err(error) => tracing::warn!(
                    "plugins/{owner}/{} is listed but unreadable, not packaged: {error:#}",
                    entry.path
                ),
            }
        }
    }
    out
}
