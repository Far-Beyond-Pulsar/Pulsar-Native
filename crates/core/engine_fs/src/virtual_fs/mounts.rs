//! Plugin folders: `<project>/plugins/<owner>/`, composed read-only over the
//! active provider.
//!
//! A plugin that provides files says so with a [`FileProvider`] registration
//! (an origin-gated contribution, from its root crate), and every project
//! then shows a folder named after it under a virtual `plugins/` folder. The
//! engine's own files (its trait definitions) are provided the same way, by
//! the engine, at `plugins/Pulsar/`. A disabled or absent plugin's folder is
//! gone, and whatever referenced its files sees them missing.
//!
//! **A folder is named for what a user reads, and answers to its id.** The
//! visible folder is the plugin's display name (`plugins/Default Pipelines/`,
//! `plugins/Blueprints/`; the engine's is `plugins/Pulsar/`): listings,
//! manifests, the file index, [`mount_of`], file events and the packaged
//! snapshot all use it. The plugin's id (`plugins/pulsar.default_pipelines/`,
//! `plugins/pulsar/`) is a legacy alias that still resolves to the same
//! folder, so a path stored before the rename (a level's pipeline id, a trait
//! reference) keeps working; [`canonical_mount_path`] turns either spelling
//! into the visible one. A name that is not a valid path segment, or that two
//! installed plugins (or the engine) would share, falls back to the id
//! ([`folder_segments`]); the id is what identifies the owner everywhere else
//! (the [`PluginFolder::owner`], the cache directory, the trait namespace).
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

mod cache;
mod layer;
mod memory;
mod packaged;
mod projects;
mod tree;

#[cfg(test)]
mod tests;

pub use cache::CacheFolder;
pub use layer::MountedFs;
pub use memory::{EmbeddedFile, MemoryFolder};
pub use projects::{
    canonical_mount_path, is_read_only, mount_of, mount_owner_id, mount_problems, mount_project,
    unmount_project, ReadOnlyMount,
};
pub use tree::{plugin_snapshot, provided_files, serve_packaged_folders};

use packaged::{dir_entry, dir_metadata, not_found_inner, PackagedFolder};
use projects::{locate, not_found, projects, refuse, Located};
use tree::{tree, Folder, Tree, PACKAGED};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::providers::{FsEntry, FsMetadata, ManifestEntry};
use crate::{events, FsChangeKind};

/// The virtual folder holding one folder per providing plugin,
/// `<project>/plugins/`.
pub const PLUGINS_FOLDER: &str = "plugins";

/// The engine's own folder's id, `pulsar` (also its trait namespace): the
/// legacy alias of `<project>/plugins/Pulsar/`.
pub const ENGINE_FOLDER: &str = "pulsar";

/// The engine's own folder as shown, `<project>/plugins/Pulsar/`.
pub const ENGINE_FOLDER_NAME: &str = "Pulsar";

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

    /// Whether its content is the same in every project (embedded files):
    /// then the catalog can list it without one ([`provided_files`]). A
    /// source whose files are per project (a cache) says no.
    fn project_independent(&self) -> bool {
        false
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
    /// The folder as shown (`Default Pipelines`), which paths and file events
    /// use.
    segment: Arc<str>,
}

impl PluginFolder {
    /// The owning plugin's id (`pulsar` for the engine): its identity, not
    /// the folder's visible name ([`segment`](Self::segment)).
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The folder's visible name under `plugins/` (the plugin's display name,
    /// or its id when the name cannot be one).
    pub fn segment(&self) -> &str {
        &self.segment
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
        let mut full = project.join(PLUGINS_FOLDER).join(&*self.segment);
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
        let id = self.id();
        let segment = folder_segments().get(&id).cloned().unwrap_or(id);
        format!("{PLUGINS_FOLDER}/{segment}")
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

/// The visible folder name of every installed plugin (and the engine), by id.
/// Computed over the installed set, not the enabled one, so a folder keeps its
/// name when another plugin is enabled or disabled.
///
/// The display name is used unless it is not a valid path segment (empty, a
/// path separator or a character Windows refuses, a leading or trailing dot,
/// a Windows device name, over 80 characters) or another installed plugin's
/// name or id, or the engine's name, is the same (compared without regard to
/// case): then the plugin's id is. The engine always keeps its own name.
pub fn folder_segments() -> HashMap<String, String> {
    let installed = pulsar_plugins::installed();
    let mut out = HashMap::new();
    out.insert(ENGINE_FOLDER.to_owned(), ENGINE_FOLDER_NAME.to_owned());
    let key = |text: &str| text.to_ascii_lowercase();
    let mut wanted: HashMap<String, usize> = HashMap::new();
    for plugin in installed {
        *wanted.entry(key(plugin.name.trim())).or_default() += 1;
    }
    for plugin in installed {
        let name = plugin.name.trim();
        let shared = wanted.get(&key(name)).copied().unwrap_or(0) > 1
            || key(name) == key(ENGINE_FOLDER_NAME)
            || key(name) == key(ENGINE_FOLDER)
            || installed
                .iter()
                .any(|other| other.id != plugin.id && key(&other.id) == key(name));
        let segment = if valid_segment(name) && !shared {
            name.to_owned()
        } else {
            plugin.id.clone()
        };
        out.insert(plugin.id.clone(), segment);
    }
    out
}

fn valid_segment(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 80
        || name.ends_with('.')
        || name.starts_with('.')
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").trim_end();
    let stem = stem.to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| n.len() == 1 && n.chars().all(|c| c.is_ascii_digit()))
        });
    !reserved
}
