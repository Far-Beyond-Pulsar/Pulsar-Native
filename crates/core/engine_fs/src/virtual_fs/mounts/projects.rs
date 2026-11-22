//! Mounted projects, and locating a path inside the plugin folders.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use parking_lot::RwLock;

use super::*;

/// A project root: as compared against paths (forward slashes, no trailing
/// slash, lower case on Windows) and as given.
pub(super) struct ProjectRoot {
    pub(super) key: String,
    pub(super) path: PathBuf,
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

pub(super) fn projects() -> &'static RwLock<Vec<ProjectRoot>> {
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
    let base = super::super::base_provider();
    if !base.exists(&root.join(PLUGINS_FOLDER)).unwrap_or(false) {
        return Vec::new();
    }
    vec![format!(
        "The project '{}' has a '{PLUGINS_FOLDER}/' folder on disk. '{PLUGINS_FOLDER}/' is where the engine and the plugins provide their files, read-only, so the folder on disk is hidden: move what it holds elsewhere in the project and delete it",
        root.display()
    )]
}

/// Where a path falls relative to the plugin folders.
pub(super) enum Located {
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

/// The folder `segment` names: by its visible name, else by the plugin's id
/// (the legacy alias); either without regard to case.
fn resolve_segment<'a>(tree: &'a Tree, segment: &str) -> Option<(&'a String, &'a Folder)> {
    tree.folders
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(segment))
        .or_else(|| {
            tree.folders
                .iter()
                .find(|(_, folder)| folder.id.eq_ignore_ascii_case(segment))
        })
}

/// A project-relative path with its plugin folder spelled as shown:
/// `plugins/pulsar.default_pipelines/X` (the id, a legacy alias) becomes
/// `plugins/Default Pipelines/X`; any other path, and a folder no enabled
/// plugin provides, is returned as it was.
pub fn canonical_mount_path(relative: &str) -> String {
    let text = relative.replace('\\', "/");
    let Some(rest) = text
        .strip_prefix(PLUGINS_FOLDER)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return relative.to_owned();
    };
    let (segment, inner) = rest.split_once('/').unwrap_or((rest, ""));
    let tree = tree();
    match resolve_segment(&tree, segment) {
        Some((name, _)) if inner.is_empty() => format!("{PLUGINS_FOLDER}/{name}"),
        Some((name, _)) => format!("{PLUGINS_FOLDER}/{name}/{inner}"),
        None => relative.to_owned(),
    }
}

/// The id of the plugin (`pulsar` for the engine) whose folder `segment` names,
/// by visible name or id.
pub fn mount_owner_id(segment: &str) -> Option<String> {
    let tree = tree();
    resolve_segment(&tree, segment).map(|(_, folder)| folder.id.clone())
}

pub(super) fn locate(path: &Path) -> Located {
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
    let (owner, source) = match resolve_segment(&tree, owner) {
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

pub(super) fn refuse(path: &Path) -> Result<()> {
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

pub(super) fn not_found(path: &Path) -> anyhow::Error {
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
