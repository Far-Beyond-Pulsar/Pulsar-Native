//! Plugin folders: `<project>/plugins/<owner>/`, composed read-only over the
//! active provider.
//!
//! A plugin that provides files says so with a [`FileProvider`] registration
//! (an origin-gated contribution, from its root crate), and every project
//! then shows a folder named after it under a virtual `plugins/` folder. The
//! engine's own files (its trait definitions) are provided the same way, by
//! the engine, at `plugins/pulsar/`. A disabled or absent plugin's folder is
//! gone, and whatever referenced its files sees them missing.
//!
//! **The plugin owns its folder.** When the folder is first mounted, the
//! engine calls the registration's `open` with a [`PluginFolder`]: the
//! capability to change that folder, which only this module can make and
//! which is handed to the owner alone. `open` returns the folder's
//! [`FolderSource`], which the plugin implements over anything it likes:
//! files it embeds (one option), data it generates or downloads, its own
//! cache. Two convenience sources come with it: [`MemoryFolder`] (embedded
//! files to start with, then [`MemoryFolder::write`] / [`MemoryFolder::delete`]
//! at runtime; gone at exit) and [`CacheFolder`] (kept per project in
//! `<project>/.pulsar/plugins/<owner>/`, which version control ignores).
//! Whenever the content changes, the owner reports it through its
//! `PluginFolder` ([`PluginFolder::changed`]; both conveniences do it
//! themselves), which emits the file event the file drawer and the project
//! watcher follow, like any other file change.
//!
//! **A plugin folder is the plugin's content, not the project's.** Nothing
//! else writes there: [`MountedFs`] is the layer every
//! [`virtual_fs`](super) call goes through, and every write, create, delete
//! or rename under `plugins/` through it fails with [`ReadOnlyMount`],
//! whoever calls it (the editor, another plugin, a script). Only the owner's
//! capability changes a folder, never a name a caller types. A user who wants
//! to own or change a file copies it into the project. (The capability stops
//! accidental writes; it is no sandbox: a native plugin runs in-process.)
//!
//! **A packaged game sees the same files.** By default the packager takes a
//! snapshot of every plugin folder ([`plugin_snapshot`]) into the package,
//! and a game serves each folder from that snapshot, read-only, without
//! calling the plugin ([`serve_packaged_folders`]): a shipped game does not
//! depend on where the editor's plugin got its files. A plugin that builds its
//! folder at game time too says so ([`InGame::Live`]).
//!
//! A project is mounted with [`mount_project`] (opening it in the editor, a
//! game loop's project root, the trait index). A `plugins/` folder the
//! project has on disk is shadowed and reported ([`mount_problems`]).
//!
//! How others do it: Unreal stacks `IPlatformFile` layers (the pak layer over
//! the physical one) and registers a mount point per plugin's content
//! (`FPackageName::RegisterMountPoint("/<Plugin>/", ...)`), opt-in through the
//! plugin's `CanContainContent`, and cooks plugin content into the game's
//! paks; Unity shows packages under `Packages/<name>` and refuses edits to
//! immutable ones. Ours is Unreal's opt-in mount points layered over
//! whichever provider is active, with the plugin, not a directory, as the
//! source of its folder.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use parking_lot::RwLock;

use crate::providers::{FsEntry, FsMetadata, FsProvider, ManifestEntry};
use crate::{events, FsChangeKind};

/// The virtual folder holding one folder per providing plugin,
/// `<project>/plugins/`.
pub const PLUGINS_FOLDER: &str = "plugins";

/// The engine's own folder's name, `<project>/plugins/pulsar/`: the engine's
/// id and trait namespace.
pub const ENGINE_FOLDER: &str = "pulsar";

/// Where [`CacheFolder`]s keep their files in a project.
pub const CACHE_FOLDER: &str = ".pulsar/plugins";

// ── What a plugin implements ─────────────────────────────────────────────────

/// The read side of a plugin's folder, implemented by its owner. `project`
/// is the mounted project asking (a source may hold the same files for every
/// project, or its own per project); paths are inside the folder,
/// `/`-separated, `""` for the folder itself.
///
/// **Never block.** Every method answers from what the source already holds:
/// the VFS calls it from any thread, the UI's included. A slow fetch (a
/// download, a generator) runs on the plugin's own task and announces what it
/// produced through [`PluginFolder::changed`]. The VFS holds no lock while it
/// calls a source.
pub trait FolderSource: Send + Sync + 'static {
    /// A file's bytes; an error for a missing file or a folder.
    fn read_file(&self, project: &Path, path: &str) -> Result<Vec<u8>>;

    /// A folder's direct children; an error for a missing folder or a file.
    fn list_dir(&self, project: &Path, path: &str) -> Result<Vec<FsEntry>>;

    /// What is at `path`; `None` when nothing is.
    fn metadata(&self, project: &Path, path: &str) -> Option<FsMetadata>;

    /// Every entry under `path`, relative to it. By default a walk over
    /// [`list_dir`](Self::list_dir).
    fn manifest(&self, project: &Path, path: &str) -> Vec<ManifestEntry> {
        let mut out = Vec::new();
        walk(self, project, path, "", &mut out);
        out
    }
}

fn walk<S: FolderSource + ?Sized>(
    source: &S,
    project: &Path,
    base: &str,
    rel: &str,
    out: &mut Vec<ManifestEntry>,
) {
    let Ok(entries) = source.list_dir(project, &join_inner(base, rel)) else {
        return;
    };
    for entry in entries {
        let child = join_inner(rel, &entry.name);
        out.push(ManifestEntry {
            path: child.clone(),
            is_dir: entry.is_dir,
            size: entry.size,
            modified: entry.modified,
        });
        if entry.is_dir {
            walk(source, project, base, &child, out);
        }
    }
}

fn join_inner(base: &str, name: &str) -> String {
    match (base.is_empty(), name.is_empty()) {
        (true, _) => name.to_owned(),
        (_, true) => base.to_owned(),
        _ => format!("{base}/{name}"),
    }
}

fn clean(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_owned()
}

/// The capability to change one plugin folder: made only here, handed only
/// to the folder's owner (its [`FileProvider::open`]). Cheap to clone; the
/// owner keeps it for as long as it changes its folder.
#[derive(Clone, Debug)]
pub struct PluginFolder {
    owner: Arc<str>,
}

impl PluginFolder {
    /// The folder's name: the owning plugin's id (`pulsar` for the engine).
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Report that `path` (inside the folder) was created, changed or
    /// removed for every mounted project: each emits the file event, which
    /// the file drawer and the project's watchers follow.
    pub fn changed(&self, path: &str, kind: FsChangeKind) {
        // Out of the lock first: the event's handlers read the VFS.
        let roots: Vec<PathBuf> = projects()
            .read()
            .iter()
            .map(|root| root.path.clone())
            .collect();
        for root in roots {
            self.changed_in(&root, path, kind.clone());
        }
    }

    /// [`changed`](Self::changed) for one project.
    pub fn changed_in(&self, project: &Path, path: &str, kind: FsChangeKind) {
        let inner = clean(path);
        let mut full = project.join(PLUGINS_FOLDER).join(&*self.owner);
        if !inner.is_empty() {
            full = full.join(inner);
        }
        events::emit(full, kind);
    }
}

/// What a packaged game serves for a plugin's folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InGame {
    /// The packager's snapshot of the folder, read-only; the plugin is not
    /// asked. The default: a shipped game does not depend on where the
    /// editor's source got its files.
    Snapshot,
    /// The plugin builds the folder in the game too (`open` is called there).
    Live,
}

/// A plugin that provides files (see the module docs).
///
/// ```ignore
/// fn open(folder: engine_fs::PluginFolder) -> std::sync::Arc<dyn engine_fs::FolderSource> {
///     std::sync::Arc::new(engine_fs::MemoryFolder::with_files(folder, TEMPLATES))
/// }
/// inventory::submit! {
///     engine_fs::FileProvider {
///         origin: pulsar_plugins::origin!(),
///         open,
///         in_game: engine_fs::InGame::Snapshot,
///     }
/// }
/// ```
pub struct FileProvider {
    pub origin: &'static str,
    /// Called once per process, when the folder is first mounted, with the
    /// capability to change it; returns the folder's content.
    pub open: fn(PluginFolder) -> Arc<dyn FolderSource>,
    pub in_game: InGame,
}

impl pulsar_plugins::Contribution for FileProvider {
    const CATEGORY: &'static str = "fs.file_providers";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        owner_folder(self.origin).unwrap_or_else(|| self.origin.to_owned())
    }

    fn label(&self) -> String {
        format!("{PLUGINS_FOLDER}/{}", self.id())
    }
}

pulsar_plugins::registration!(FileProvider);

/// The folder a registration's origin owns: the plugin's id, `pulsar` for
/// the engine, `None` for an origin that is neither.
fn owner_folder(origin: &'static str) -> Option<String> {
    match pulsar_plugins::owner_of(origin) {
        pulsar_plugins::Owner::Engine(_) => Some(ENGINE_FOLDER.to_owned()),
        pulsar_plugins::Owner::Plugin(plugin) => Some(plugin.id.clone()),
        pulsar_plugins::Owner::Unknown(_) => None,
    }
}

// ── The in-memory source ─────────────────────────────────────────────────────

/// One file embedded in a binary, for [`MemoryFolder::with_files`].
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedFile {
    /// Path inside the folder, `/`-separated (`traits/x.trait.json`).
    pub path: &'static str,
    pub bytes: &'static [u8],
}

/// Embed files under `dir` (relative to the calling crate's manifest) as
/// [`EmbeddedFile`]s at their path inside the folder.
#[macro_export]
macro_rules! embedded_files {
    ($dir:literal: $($path:literal),* $(,)?) => {
        &[$($crate::EmbeddedFile {
            path: $path,
            bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $path)),
        }),*]
    };
}

#[derive(Default)]
struct MemoryTree {
    files: BTreeMap<String, Arc<[u8]>>,
    dirs: BTreeSet<String>,
}

impl MemoryTree {
    fn add_parents(&mut self, path: &str) {
        let mut at = 0;
        while let Some(slash) = path[at..].find('/') {
            self.dirs.insert(path[..at + slash].to_owned());
            at += slash + 1;
        }
    }

    fn is_dir(&self, path: &str) -> bool {
        path.is_empty() || self.dirs.contains(path)
    }
}

/// A plugin folder held in memory, the same for every project. It starts
/// with whatever the plugin puts in (embedded files, generated ones) and
/// changes at runtime through [`write`](Self::write),
/// [`create_dir_all`](Self::create_dir_all) and [`delete`](Self::delete),
/// each reported through the folder's capability, which it holds. Gone at
/// exit: what must last goes in a [`CacheFolder`].
pub struct MemoryFolder {
    folder: PluginFolder,
    tree: RwLock<MemoryTree>,
}

impl MemoryFolder {
    /// An empty folder.
    pub fn new(folder: PluginFolder) -> Self {
        Self {
            folder,
            tree: RwLock::new(MemoryTree::default()),
        }
    }

    /// A folder holding `files` (embedded with [`embedded_files!`](crate::embedded_files)).
    pub fn with_files(folder: PluginFolder, files: &[EmbeddedFile]) -> Self {
        let this = Self::new(folder);
        {
            let mut tree = this.tree.write();
            for file in files {
                let path = clean(file.path);
                tree.add_parents(&path);
                tree.files.insert(path, Arc::from(file.bytes));
            }
        }
        this
    }

    /// The folder's capability.
    pub fn folder(&self) -> &PluginFolder {
        &self.folder
    }

    /// Put `bytes` at `path`, creating its parent folders.
    pub fn write(&self, path: &str, bytes: impl Into<Arc<[u8]>>) {
        let path = clean(path);
        let existed = {
            let mut tree = self.tree.write();
            tree.add_parents(&path);
            tree.files.insert(path.clone(), bytes.into()).is_some()
        };
        let kind = if existed {
            FsChangeKind::Modified
        } else {
            FsChangeKind::Created
        };
        self.folder.changed(&path, kind);
    }

    /// Create the folder `path` and its parents.
    pub fn create_dir_all(&self, path: &str) {
        let path = clean(path);
        if path.is_empty() {
            return;
        }
        let created = {
            let mut tree = self.tree.write();
            tree.add_parents(&format!("{path}/"));
            tree.dirs.insert(path.clone())
        };
        if created {
            self.folder.changed(&path, FsChangeKind::Created);
        }
    }

    /// Remove the file or folder (with everything in it) at `path`.
    pub fn delete(&self, path: &str) {
        let path = clean(path);
        let prefix = format!("{path}/");
        let removed = {
            let mut tree = self.tree.write();
            let file = tree.files.remove(&path).is_some();
            let before = tree.files.len() + tree.dirs.len();
            tree.files.retain(|name, _| !name.starts_with(&prefix));
            tree.dirs
                .retain(|name| *name != path && !name.starts_with(&prefix));
            file || before != tree.files.len() + tree.dirs.len()
        };
        if removed {
            self.folder.changed(&path, FsChangeKind::Deleted);
        }
    }
}

impl FolderSource for MemoryFolder {
    fn read_file(&self, _project: &Path, path: &str) -> Result<Vec<u8>> {
        self.tree
            .read()
            .files
            .get(&clean(path))
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| not_found_inner(path))
    }

    fn list_dir(&self, _project: &Path, path: &str) -> Result<Vec<FsEntry>> {
        let path = clean(path);
        let tree = self.tree.read();
        if !tree.is_dir(&path) {
            return Err(not_found_inner(&path));
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let direct = |name: &str| {
            name.strip_prefix(prefix.as_str())
                .filter(|rest| !rest.is_empty() && !rest.contains('/'))
                .map(str::to_owned)
        };
        let mut entries: Vec<FsEntry> = tree
            .dirs
            .iter()
            .filter_map(|dir| direct(dir))
            .map(|name| dir_entry(&name))
            .collect();
        entries.extend(tree.files.iter().filter_map(|(name, bytes)| {
            direct(name).map(|name| FsEntry {
                name,
                is_dir: false,
                size: bytes.len() as u64,
                modified: None,
            })
        }));
        Ok(entries)
    }

    fn metadata(&self, _project: &Path, path: &str) -> Option<FsMetadata> {
        let path = clean(path);
        let tree = self.tree.read();
        if tree.is_dir(&path) {
            return Some(dir_metadata());
        }
        tree.files.get(&path).map(|bytes| FsMetadata {
            is_dir: false,
            size: bytes.len() as u64,
            modified: None,
            modified_ns: None,
        })
    }
}

// ── The per-project cache source ─────────────────────────────────────────────

/// A plugin folder kept per project in `<project>/.pulsar/plugins/<owner>/`
/// (derived data version control ignores, like the rest of `.pulsar/`), so
/// what the plugin wrote at runtime is there next time. Writes need the
/// project they are for and are reported through the folder's capability,
/// which it holds. The files are written through the project's own provider
/// (`virtual_fs`, not `std::fs`), so a remote project keeps its cache too.
pub struct CacheFolder {
    folder: PluginFolder,
}

impl CacheFolder {
    pub fn new(folder: PluginFolder) -> Self {
        Self { folder }
    }

    /// The folder's capability.
    pub fn folder(&self) -> &PluginFolder {
        &self.folder
    }

    /// Where the folder's files are kept for `project`.
    pub fn dir(&self, project: &Path) -> PathBuf {
        project.join(CACHE_FOLDER).join(self.folder.owner())
    }

    fn real(&self, project: &Path, path: &str) -> PathBuf {
        let inner = clean(path);
        let dir = self.dir(project);
        if inner.is_empty() {
            dir
        } else {
            dir.join(inner)
        }
    }

    /// Put `bytes` at `path` in `project`'s copy, creating its parents.
    pub fn write(&self, project: &Path, path: &str, bytes: &[u8]) -> Result<()> {
        let real = self.real(project, path);
        let existed = super::exists(&real).unwrap_or(false);
        super::write_file(&real, bytes)?;
        let kind = if existed {
            FsChangeKind::Modified
        } else {
            FsChangeKind::Created
        };
        self.folder.changed_in(project, path, kind);
        Ok(())
    }

    /// Create the folder `path` (and its parents) in `project`'s copy.
    pub fn create_dir_all(&self, project: &Path, path: &str) -> Result<()> {
        super::create_dir_all(&self.real(project, path))?;
        self.folder.changed_in(project, path, FsChangeKind::Created);
        Ok(())
    }

    /// Remove the file or folder at `path` from `project`'s copy.
    pub fn delete(&self, project: &Path, path: &str) -> Result<()> {
        super::delete_path(&self.real(project, path))?;
        self.folder.changed_in(project, path, FsChangeKind::Deleted);
        Ok(())
    }
}

impl FolderSource for CacheFolder {
    fn read_file(&self, project: &Path, path: &str) -> Result<Vec<u8>> {
        super::base_provider().read_file(&self.real(project, path))
    }

    fn list_dir(&self, project: &Path, path: &str) -> Result<Vec<FsEntry>> {
        let real = self.real(project, path);
        let base = super::base_provider();
        if clean(path).is_empty() && !base.exists(&real).unwrap_or(false) {
            // Nothing written for this project yet: an empty folder.
            return Ok(Vec::new());
        }
        base.list_dir(&real)
    }

    fn metadata(&self, project: &Path, path: &str) -> Option<FsMetadata> {
        if clean(path).is_empty() {
            return Some(dir_metadata());
        }
        super::base_provider()
            .metadata(&self.real(project, path))
            .ok()
    }
}

// ── The packaged source ──────────────────────────────────────────────────────

/// A packaged game's copy of a plugin folder: the snapshot the packager wrote
/// at `plugins/<owner>/` in the game's content, read from the game's
/// provider (the pak, or loose files).
struct PackagedFolder {
    owner: String,
}

impl PackagedFolder {
    fn real(&self, project: &Path, path: &str) -> PathBuf {
        let inner = clean(path);
        let dir = project.join(PLUGINS_FOLDER).join(&self.owner);
        if inner.is_empty() {
            dir
        } else {
            dir.join(inner)
        }
    }
}

impl FolderSource for PackagedFolder {
    fn read_file(&self, project: &Path, path: &str) -> Result<Vec<u8>> {
        super::base_provider().read_file(&self.real(project, path))
    }

    fn list_dir(&self, project: &Path, path: &str) -> Result<Vec<FsEntry>> {
        super::base_provider()
            .list_dir(&self.real(project, path))
            .or_else(|error| {
                if clean(path).is_empty() {
                    Ok(Vec::new())
                } else {
                    Err(error)
                }
            })
    }

    fn metadata(&self, project: &Path, path: &str) -> Option<FsMetadata> {
        if clean(path).is_empty() {
            return Some(dir_metadata());
        }
        super::base_provider()
            .metadata(&self.real(project, path))
            .ok()
    }
}

fn not_found_inner(path: &str) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("'{path}' is not in this plugin folder"),
    ))
}

fn dir_entry(name: &str) -> FsEntry {
    FsEntry {
        name: name.to_owned(),
        is_dir: true,
        size: 0,
        modified: None,
    }
}

fn dir_metadata() -> FsMetadata {
    FsMetadata {
        is_dir: true,
        size: 0,
        modified: None,
        modified_ns: None,
    }
}

// ── The mounted folders ──────────────────────────────────────────────────────

/// Serving packaged snapshots ([`serve_packaged_folders`]).
static PACKAGED: AtomicBool = AtomicBool::new(false);

/// Serve every [`InGame::Snapshot`] folder from the packaged game's content
/// (the snapshot the packager wrote at `plugins/<owner>/`) instead of
/// opening its plugin. The game's content root calls it when it installs a
/// packaged game; an on-disk `plugins/` folder is then the snapshot, not a
/// problem.
pub fn serve_packaged_folders(on: bool) {
    PACKAGED.store(on, Ordering::Release);
}

/// Every source opened so far, by folder: each provider is opened once per
/// process and keeps its content across a plugin being disabled and enabled.
fn opened() -> &'static RwLock<HashMap<String, Arc<dyn FolderSource>>> {
    static OPENED: OnceLock<RwLock<HashMap<String, Arc<dyn FolderSource>>>> = OnceLock::new();
    OPENED.get_or_init(|| RwLock::new(HashMap::new()))
}

/// A mounted folder.
struct Folder {
    source: Arc<dyn FolderSource>,
    in_game: InGame,
}

/// The folders mounted for one plugin generation, by name.
struct Tree {
    generation: u64,
    packaged: bool,
    folders: BTreeMap<String, Folder>,
}

fn build_tree(generation: u64, packaged: bool) -> Tree {
    let mut folders: BTreeMap<String, Folder> = BTreeMap::new();
    let mut owners: BTreeMap<String, &'static str> = BTreeMap::new();
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
        let source: Arc<dyn FolderSource> = if packaged && provider.in_game == InGame::Snapshot {
            Arc::new(PackagedFolder {
                owner: owner.clone(),
            })
        } else {
            let existing = opened().read().get(&owner).cloned();
            match existing {
                Some(source) => source,
                None => {
                    let source = (provider.open)(PluginFolder {
                        owner: Arc::from(owner.as_str()),
                    });
                    opened()
                        .write()
                        .entry(owner.clone())
                        .or_insert(source)
                        .clone()
                }
            }
        };
        folders.insert(
            owner,
            Folder {
                source,
                in_game: provider.in_game,
            },
        );
    }
    Tree {
        generation,
        packaged,
        folders,
    }
}

static TREE: OnceLock<RwLock<Option<Arc<Tree>>>> = OnceLock::new();

/// The folders mounted for the plugin set in effect now (rebuilt when it
/// changes).
fn tree() -> Arc<Tree> {
    let cell = TREE.get_or_init(|| RwLock::new(None));
    let generation = pulsar_plugins::generation();
    let packaged = PACKAGED.load(Ordering::Acquire);
    let current = |tree: &Tree| tree.generation == generation && tree.packaged == packaged;
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
    let fresh = Arc::new(build_tree(generation, packaged));
    *cell.write() = Some(Arc::clone(&fresh));
    fresh
}

/// Every file of every folder the packager snapshots ([`InGame::Snapshot`])
/// in the project at `project`, as `(project-relative path, bytes)`
/// (`plugins/<owner>/...`): what a packaged game serves for them.
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

// ── Mounted projects ─────────────────────────────────────────────────────────

/// A project root: as compared against paths (forward slashes, no trailing
/// slash, lower case on Windows) and as given.
struct ProjectRoot {
    key: String,
    path: PathBuf,
}

fn comparable(path: &str) -> String {
    let text = path.replace('\\', "/");
    // A canonical Windows path is verbatim (`\\?\C:\...`, `\\?\UNC\host\...`):
    // the same folder as the path without the prefix.
    let text = if let Some(rest) = text.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else {
        text.strip_prefix("//?/").map(str::to_owned).unwrap_or(text)
    };
    let text = text.trim_end_matches('/');
    if cfg!(windows) {
        text.to_ascii_lowercase()
    } else {
        text.to_owned()
    }
}

fn projects() -> &'static RwLock<Vec<ProjectRoot>> {
    static PROJECTS: OnceLock<RwLock<Vec<ProjectRoot>>> = OnceLock::new();
    PROJECTS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Give the project at `root` its `plugins/` folder. Idempotent and cheap
/// after the first call; reports an on-disk `plugins/` folder once.
pub fn mount_project(root: &Path) {
    let key = comparable(&root.to_string_lossy());
    if key.is_empty() || projects().read().iter().any(|p| p.key == key) {
        return;
    }
    {
        let mut list = projects().write();
        if list.iter().any(|p| p.key == key) {
            return;
        }
        list.push(ProjectRoot {
            key,
            path: root.to_path_buf(),
        });
    }
    for problem in mount_problems(root) {
        tracing::error!("{problem}");
    }
}

/// Take the `plugins/` folder away from the project at `root` (closing it).
pub fn unmount_project(root: &Path) {
    let key = comparable(&root.to_string_lossy());
    projects().write().retain(|known| known.key != key);
}

/// What is wrong with how the project at `root` meets the plugin folders: a
/// `plugins/` folder on disk, which the virtual one shadows (in a packaged
/// game it is the snapshot, and no problem).
pub fn mount_problems(root: &Path) -> Vec<String> {
    if PACKAGED.load(Ordering::Acquire) {
        return Vec::new();
    }
    let base = super::base_provider();
    if !base.exists(&root.join(PLUGINS_FOLDER)).unwrap_or(false) {
        return Vec::new();
    }
    vec![format!(
        "The project '{}' has a '{PLUGINS_FOLDER}/' folder on disk. '{PLUGINS_FOLDER}/' is where the engine and the plugins provide their files, read-only, so the folder on disk is hidden: move what it holds elsewhere in the project and delete it",
        root.display()
    )]
}

/// Where a path falls relative to the plugin folders.
enum Located {
    /// Not under any `plugins/`: the active provider's.
    Outside,
    /// A mounted project's root folder (its listing gains `plugins/`).
    ProjectRoot,
    /// `<project>/plugins`: one folder per providing plugin.
    PluginsFolder,
    /// Inside the folder `owner` of `project`, at `inner` (empty for the
    /// folder itself); `source` is `None` when no enabled plugin provides it.
    Folder {
        project: PathBuf,
        owner: String,
        inner: String,
        source: Option<Arc<dyn FolderSource>>,
    },
}

fn locate(path: &Path) -> Located {
    let text = comparable(&path.to_string_lossy());
    let original = path.to_string_lossy().replace('\\', "/");
    let original = original.trim_end_matches('/');
    // Match the project under the lock, then let go before asking the tree,
    // whose building calls plugins.
    let found = {
        let roots = projects().read();
        let mut found = None;
        for root in roots.iter() {
            if text == root.key {
                return Located::ProjectRoot;
            }
            if let Some(rest) = text
                .strip_prefix(root.key.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
            {
                found = Some((root.path.clone(), rest.to_owned()));
                break;
            }
        }
        found
    };
    let Some((project, rest)) = found else {
        return Located::Outside;
    };
    if rest == PLUGINS_FOLDER {
        return Located::PluginsFolder;
    }
    if !rest.starts_with("plugins/") {
        return Located::Outside;
    }
    // The same span of the path as written (case kept).
    let rest_original = &original[original.len() - rest.len()..];
    let after = &rest_original[PLUGINS_FOLDER.len() + 1..];
    let (owner, inner) = after.split_once('/').unwrap_or((after, ""));
    let tree = tree();
    let (owner, source) = match tree
        .folders
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(owner))
    {
        Some((name, folder)) => (name.clone(), Some(Arc::clone(&folder.source))),
        None => (owner.to_owned(), None),
    };
    Located::Folder {
        project,
        owner,
        inner: inner.to_owned(),
        source,
    }
}

/// A write into the plugin folders: what every changing VFS operation on a
/// path under `<project>/plugins/` fails with (inside the `anyhow::Error`;
/// test with [`is_read_only`]). Only the folder's owner changes it, through
/// its [`PluginFolder`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadOnlyMount {
    /// The folder, as a project path (`plugins/pulsar.lights`, `plugins`).
    pub mount: String,
    pub path: PathBuf,
}

impl std::fmt::Display for ReadOnlyMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "'{}' is inside '{}', which only its plugin can change: copy the file into the project to change it",
            self.path.display(),
            self.mount
        )
    }
}

impl std::error::Error for ReadOnlyMount {}

/// Whether `error` is a write refused by a plugin folder.
pub fn is_read_only(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ReadOnlyMount>().is_some()
}

fn refuse(path: &Path) -> Result<()> {
    let mount = match locate(path) {
        Located::Outside | Located::ProjectRoot => return Ok(()),
        Located::PluginsFolder => PLUGINS_FOLDER.to_owned(),
        Located::Folder { owner, .. } => format!("{PLUGINS_FOLDER}/{owner}"),
    };
    Err(anyhow::Error::new(ReadOnlyMount {
        mount,
        path: path.to_path_buf(),
    }))
}

fn not_found(path: &Path) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("'{}' is not in its plugin folder", path.display()),
    ))
}

/// The plugin folder a project path is in, if any: `Some("plugins/pulsar.lights")`.
/// For the file drawer's read-only state.
pub fn mount_of(path: &Path) -> Option<String> {
    match locate(path) {
        Located::Outside | Located::ProjectRoot => None,
        Located::PluginsFolder => Some(PLUGINS_FOLDER.to_owned()),
        Located::Folder { owner, .. } => Some(format!("{PLUGINS_FOLDER}/{owner}")),
    }
}

// ── The layer ────────────────────────────────────────────────────────────────

/// The active provider with the plugin folders composed over it.
pub struct MountedFs {
    base: Arc<dyn FsProvider>,
}

impl MountedFs {
    pub fn new(base: Arc<dyn FsProvider>) -> Self {
        Self { base }
    }

    /// The provider underneath, which knows no plugin folders.
    pub fn base(&self) -> &Arc<dyn FsProvider> {
        &self.base
    }
}

fn has_folders() -> bool {
    !tree().folders.is_empty()
}

impl FsProvider for MountedFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        match locate(path) {
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } => source.read_file(&project, &inner),
            Located::Folder { .. } | Located::PluginsFolder => Err(not_found(path)),
            _ => self.base.read_file(path),
        }
    }

    fn read_file_prefix(&self, path: &Path, len: usize) -> Result<Vec<u8>> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.read_file_prefix(path, len),
            _ => {
                let mut bytes = self.read_file(path)?;
                bytes.truncate(len);
                Ok(bytes)
            }
        }
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.write_file(path, content)
    }

    fn write_file_atomically(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.write_file_atomically(path, content)
    }

    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.create_file(path, content)
    }

    fn create_executable_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.create_executable_file(path, content)
    }

    fn append_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        refuse(path)?;
        self.base.append_file(path, content)
    }

    fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        refuse(to)?;
        match locate(from) {
            Located::Outside | Located::ProjectRoot => self.base.copy(from, to),
            _ => self.base.write_file(to, &self.read_file(from)?),
        }
    }

    fn open_read(&self, path: &Path) -> Result<Box<dyn crate::providers::ReadSeek>> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.open_read(path),
            _ => Ok(Box::new(std::io::Cursor::new(self.read_file(path)?))),
        }
    }

    fn open_write_stream(&self, path: &Path) -> Result<Option<Box<dyn std::io::Write + Send>>> {
        refuse(path)?;
        self.base.open_write_stream(path)
    }

    fn delete_path(&self, path: &Path) -> Result<()> {
        refuse(path)?;
        self.base.delete_path(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        refuse(from)?;
        refuse(to)?;
        self.base.rename(from, to)
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>> {
        match locate(path) {
            Located::Outside => self.base.list_dir(path),
            Located::ProjectRoot => {
                let mut entries: Vec<FsEntry> = self
                    .base
                    .list_dir(path)?
                    .into_iter()
                    .filter(|e| !(e.is_dir && e.name.eq_ignore_ascii_case(PLUGINS_FOLDER)))
                    .collect();
                if has_folders() {
                    entries.push(dir_entry(PLUGINS_FOLDER));
                }
                Ok(entries)
            }
            Located::PluginsFolder => {
                Ok(tree().folders.keys().map(|name| dir_entry(name)).collect())
            }
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } => source.list_dir(&project, &inner),
            Located::Folder { .. } => Err(not_found(path)),
        }
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        match locate(path) {
            // What already exists needs no creating; anything new in a
            // plugin folder is its owner's to make.
            Located::PluginsFolder if has_folders() => Ok(()),
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } if source
                .metadata(&project, &inner)
                .is_some_and(|meta| meta.is_dir) =>
            {
                Ok(())
            }
            Located::PluginsFolder | Located::Folder { .. } => refuse(path),
            _ => self.base.create_dir_all(path),
        }
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.exists(path),
            Located::PluginsFolder => Ok(has_folders()),
            Located::Folder {
                project,
                inner,
                source,
                ..
            } => Ok(source.is_some_and(|source| source.metadata(&project, &inner).is_some())),
        }
    }

    fn metadata(&self, path: &Path) -> Result<FsMetadata> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.metadata(path),
            Located::PluginsFolder if has_folders() => Ok(dir_metadata()),
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } => source
                .metadata(&project, &inner)
                .ok_or_else(|| not_found(path)),
            _ => Err(not_found(path)),
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.canonicalize(path),
            // Plugin folders are virtual: their paths are their own
            // canonical form.
            _ if self.exists(path)? => Ok(path.to_path_buf()),
            _ => Err(not_found(path)),
        }
    }

    fn is_symlink(&self, path: &Path) -> Result<bool> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.is_symlink(path),
            _ => Ok(false),
        }
    }

    fn manifest(&self, path: &Path) -> Result<Vec<ManifestEntry>> {
        let folder_entries = |project: &Path, prefix: &str| -> Vec<ManifestEntry> {
            let tree = tree();
            let mut out = Vec::new();
            for (name, folder) in &tree.folders {
                let at = join_inner(prefix, name);
                out.push(ManifestEntry {
                    path: at.clone(),
                    is_dir: true,
                    size: 0,
                    modified: None,
                });
                out.extend(
                    folder
                        .source
                        .manifest(project, "")
                        .into_iter()
                        .map(|mut entry| {
                            entry.path = format!("{at}/{}", entry.path);
                            entry
                        }),
                );
            }
            out
        };
        match locate(path) {
            Located::Outside => self.base.manifest(path),
            Located::ProjectRoot => {
                let mut out: Vec<ManifestEntry> = self
                    .base
                    .manifest(path)?
                    .into_iter()
                    .filter(|entry| {
                        !entry
                            .path
                            .split('/')
                            .next()
                            .is_some_and(|first| first.eq_ignore_ascii_case(PLUGINS_FOLDER))
                    })
                    .collect();
                if has_folders() {
                    out.push(ManifestEntry {
                        path: PLUGINS_FOLDER.to_owned(),
                        is_dir: true,
                        size: 0,
                        modified: None,
                    });
                    out.extend(folder_entries(path, PLUGINS_FOLDER));
                }
                Ok(out)
            }
            Located::PluginsFolder => {
                let project = path.parent().unwrap_or(path);
                Ok(folder_entries(project, ""))
            }
            Located::Folder {
                project,
                inner,
                source: Some(source),
                ..
            } => Ok(source.manifest(&project, &inner)),
            Located::Folder { .. } => Ok(Vec::new()),
        }
    }

    fn is_remote(&self) -> bool {
        self.base.is_remote()
    }

    fn permits_local_executable_writes(&self) -> bool {
        self.base.permits_local_executable_writes()
    }

    fn label(&self) -> &str {
        self.base.label()
    }
}

#[cfg(test)]
mod tests;
