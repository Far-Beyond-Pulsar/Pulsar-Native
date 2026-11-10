//! Read-only mounts composed over the active provider.
//!
//! A project sees two kinds of content it did not author and cannot change:
//!
//! - `<project>/pulsar/`: the engine's own project-facing files (its trait
//!   definitions, ...);
//! - `<project>/plugins/<plugin id>/`: each enabled plugin's (its traits and
//!   other project-facing data). Every enabled installed plugin has one,
//!   empty or not, so the folder lists exactly the project's plugins.
//!
//! The files are embedded in the binary that links their owner and are
//! registered as [`MountedContent`] (an origin-gated contribution): the
//! registration's owner picks the mount, the engine's
//! ([`pulsar_plugins::engine_origin!`]) going to `pulsar/` and a plugin
//! root's to `plugins/<its id>/`, so no plugin can put files in another's
//! folder. A disabled plugin's mount is gone, and whatever referenced its
//! files sees them missing.
//!
//! [`MountedFs`] is the layer every [`virtual_fs`](super) call goes through: it
//! answers for paths inside a mount and hands everything else to the active
//! provider (local, remote or peer-to-peer), so mounts look the same whatever
//! backs the project. Writing, creating, deleting or renaming anything inside
//! a mount fails with [`ReadOnlyMount`]. A project is mounted with
//! [`mount_project`] (opening it in the editor, a game loop's project root,
//! the trait index); a folder named `plugins/` or `pulsar/` that the project
//! has on disk is shadowed by the mount and reported ([`mount_problems`]).
//!
//! How others do it: Unreal stacks `IPlatformFile` layers (the pak layer over
//! the physical one) and registers a mount point per plugin's content
//! (`FPackageName::RegisterMountPoint("/<Plugin>/", ...)`); Unity shows
//! immutable packages under `Packages/<name>` and refuses edits to them. Ours
//! is Unreal's layering with Unity's layout: one layer over whichever provider
//! is active, read-only folders under a fixed project path, a named error on
//! write.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use parking_lot::RwLock;

use crate::providers::{FsEntry, FsMetadata, FsProvider, ManifestEntry};

/// The engine's mount, at `<project>/pulsar/`.
pub const ENGINE_MOUNT: &str = "pulsar";

/// The folder holding one mount per enabled plugin, `<project>/plugins/`.
pub const PLUGINS_FOLDER: &str = "plugins";

/// One embedded file of a [`MountedContent`].
#[derive(Clone, Copy, Debug)]
pub struct MountedFile {
    /// Path inside the owner's mount, forward slashes (`traits/x.trait.json`).
    pub path: &'static str,
    pub bytes: &'static [u8],
}

/// Files the engine or a plugin serves read-only into every project. The
/// origin decides the mount (see the module docs).
///
/// ```ignore
/// inventory::submit! {
///     engine_fs::MountedContent {
///         origin: pulsar_plugins::origin!(),
///         id: "pulsar.lights.traits",
///         files: engine_fs::mounted_files!("content": "traits/light_source.trait.json"),
///     }
/// }
/// ```
pub struct MountedContent {
    pub origin: &'static str,
    /// Stable within its owner (`"pulsar.traits"`).
    pub id: &'static str,
    pub files: &'static [MountedFile],
}

impl pulsar_plugins::Contribution for MountedContent {
    const CATEGORY: &'static str = "fs.mounted_content";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_owned()
    }

    fn label(&self) -> String {
        format!("{} ({} files)", self.id, self.files.len())
    }
}

pulsar_plugins::registration!(MountedContent);

/// Embed files under `dir` (relative to the calling crate's manifest) as
/// [`MountedFile`]s at their path inside the mount.
#[macro_export]
macro_rules! mounted_files {
    ($dir:literal: $($path:literal),* $(,)?) => {
        &[$($crate::MountedFile {
            path: $path,
            bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $path)),
        }),*]
    };
}

/// A write into a read-only mount: what every changing operation on a path
/// inside `<project>/pulsar/` or `<project>/plugins/` fails with (inside the
/// `anyhow::Error`; test with [`is_read_only`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadOnlyMount {
    /// The mount, as a project path (`plugins/pulsar.lights`, `pulsar`,
    /// `plugins`).
    pub mount: String,
    pub path: PathBuf,
}

impl std::fmt::Display for ReadOnlyMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "'{}' is inside the read-only mount '{}': the engine and plugins serve it, a project cannot change it",
            self.path.display(),
            self.mount
        )
    }
}

impl std::error::Error for ReadOnlyMount {}

/// Whether `error` is a write refused by a read-only mount.
pub fn is_read_only(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ReadOnlyMount>().is_some()
}

// ── The mounted tree ─────────────────────────────────────────────────────────

/// One mount's files and the folders they imply, by path inside the mount.
#[derive(Default)]
struct Mount {
    files: BTreeMap<&'static str, &'static [u8]>,
    dirs: BTreeSet<String>,
}

impl Mount {
    fn add(&mut self, file: &MountedFile) {
        let path = file.path.trim_matches('/');
        let mut at = 0;
        while let Some(slash) = path[at..].find('/') {
            self.dirs.insert(path[..at + slash].to_owned());
            at += slash + 1;
        }
        self.files.insert(path, file.bytes);
    }

    fn is_dir(&self, inner: &str) -> bool {
        inner.is_empty() || self.dirs.contains(inner)
    }

    fn children(&self, inner: &str) -> Vec<FsEntry> {
        let prefix = if inner.is_empty() {
            String::new()
        } else {
            format!("{inner}/")
        };
        let direct = |path: &str| {
            path.strip_prefix(prefix.as_str())
                .filter(|rest| !rest.is_empty() && !rest.contains('/'))
                .map(str::to_owned)
        };
        let mut entries: Vec<FsEntry> = self
            .dirs
            .iter()
            .filter_map(|dir| direct(dir))
            .map(|name| dir_entry(&name))
            .collect();
        entries.extend(self.files.iter().filter_map(|(path, bytes)| {
            direct(path).map(|name| FsEntry {
                name,
                is_dir: false,
                size: bytes.len() as u64,
                modified: None,
            })
        }));
        entries
    }

    /// Every entry under `inner`, as paths relative to it.
    fn manifest(&self, inner: &str) -> Vec<ManifestEntry> {
        let prefix = if inner.is_empty() {
            String::new()
        } else {
            format!("{inner}/")
        };
        let mut out: Vec<ManifestEntry> = self
            .dirs
            .iter()
            .filter_map(|dir| dir.strip_prefix(prefix.as_str()))
            .map(|rest| ManifestEntry {
                path: rest.to_owned(),
                is_dir: true,
                size: 0,
                modified: None,
            })
            .collect();
        out.extend(self.files.iter().filter_map(|(path, bytes)| {
            path.strip_prefix(prefix.as_str()).map(|rest| ManifestEntry {
                path: rest.to_owned(),
                is_dir: false,
                size: bytes.len() as u64,
                modified: None,
            })
        }));
        out
    }
}

fn dir_entry(name: &str) -> FsEntry {
    FsEntry {
        name: name.to_owned(),
        is_dir: true,
        size: 0,
        modified: None,
    }
}

/// Every mount, by its project path (`pulsar`, `plugins/<id>`), for one
/// plugin generation.
struct Tree {
    generation: u64,
    mounts: BTreeMap<String, Arc<Mount>>,
}

fn build_tree(generation: u64) -> Tree {
    let mut mounts: BTreeMap<String, Mount> = BTreeMap::new();
    mounts.insert(ENGINE_MOUNT.to_owned(), Mount::default());
    for plugin in pulsar_plugins::installed() {
        if pulsar_plugins::is_enabled(&plugin.id) {
            mounts.insert(format!("{PLUGINS_FOLDER}/{}", plugin.id), Mount::default());
        }
    }
    for content in pulsar_plugins::registered::<MountedContent>() {
        let mount = match pulsar_plugins::owner_of(content.origin) {
            pulsar_plugins::Owner::Engine(_) => ENGINE_MOUNT.to_owned(),
            pulsar_plugins::Owner::Plugin(plugin) => format!("{PLUGINS_FOLDER}/{}", plugin.id),
            pulsar_plugins::Owner::Unknown(origin) => {
                tracing::error!(
                    "mounted content '{}' comes from `{origin}`, which is neither the engine nor an installed plugin's root; it is not mounted",
                    content.id
                );
                continue;
            }
        };
        let Some(target) = mounts.get_mut(&mount) else {
            continue;
        };
        for file in content.files {
            target.add(file);
        }
    }
    Tree {
        generation,
        mounts: mounts
            .into_iter()
            .map(|(name, mount)| (name, Arc::new(mount)))
            .collect(),
    }
}

static TREE: OnceLock<RwLock<Option<Arc<Tree>>>> = OnceLock::new();

/// The mounts for the plugin set in effect now (rebuilt when it changes).
fn tree() -> Arc<Tree> {
    let cell = TREE.get_or_init(|| RwLock::new(None));
    let generation = pulsar_plugins::generation();
    if let Some(tree) = cell.read().as_ref() {
        if tree.generation == generation {
            return Arc::clone(tree);
        }
    }
    let fresh = Arc::new(build_tree(generation));
    *cell.write() = Some(Arc::clone(&fresh));
    fresh
}

// ── Mounted projects ─────────────────────────────────────────────────────────

/// A project root as compared against paths: forward slashes, no trailing
/// slash, lower case on Windows (paths are case-insensitive there).
fn comparable(path: &str) -> String {
    let text = path.replace('\\', "/");
    let text = text.trim_end_matches('/');
    if cfg!(windows) {
        text.to_ascii_lowercase()
    } else {
        text.to_owned()
    }
}

fn projects() -> &'static RwLock<Vec<String>> {
    static PROJECTS: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    PROJECTS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Give the project at `root` its mounts. Idempotent and cheap after the
/// first call; reports the on-disk folders the mounts shadow once.
pub fn mount_project(root: &Path) {
    let key = comparable(&root.to_string_lossy());
    if key.is_empty() {
        return;
    }
    if projects().read().contains(&key) {
        return;
    }
    {
        let mut list = projects().write();
        if list.contains(&key) {
            return;
        }
        list.push(key);
    }
    for problem in mount_problems(root) {
        tracing::error!("{problem}");
    }
}

/// Take the mounts away from the project at `root` (closing it).
pub fn unmount_project(root: &Path) {
    let key = comparable(&root.to_string_lossy());
    projects().write().retain(|known| *known != key);
}

/// What is wrong with how the project at `root` meets its mounts: an
/// on-disk folder at a mount's path, which the mount shadows.
pub fn mount_problems(root: &Path) -> Vec<String> {
    let base = super::base_provider();
    [PLUGINS_FOLDER, ENGINE_MOUNT]
        .into_iter()
        .filter(|folder| base.exists(&root.join(folder)).unwrap_or(false))
        .map(|folder| {
            format!(
                "The project '{}' has a '{folder}/' folder on disk. '{folder}/' is where the {} files are mounted, read-only, so the folder on disk is hidden: move what it holds elsewhere in the project and delete it",
                root.display(),
                if folder == ENGINE_MOUNT { "engine's" } else { "plugins'" }
            )
        })
        .collect()
}

/// Where a path falls relative to the mounts.
enum Located {
    /// Not in any mount: the active provider's.
    Outside,
    /// A mounted project's root folder (its listing gains the mounts).
    ProjectRoot,
    /// `<project>/plugins`: one folder per plugin mount.
    PluginsFolder,
    /// Inside the mount named `mount` (`pulsar`, `plugins/<id>`), at `inner`
    /// (empty for the mount's own folder).
    Mount {
        mount: String,
        inner: String,
        files: Option<Arc<Mount>>,
    },
}

fn locate(path: &Path) -> Located {
    let text = comparable(&path.to_string_lossy());
    let original = path.to_string_lossy().replace('\\', "/");
    let original = original.trim_end_matches('/');
    for root in projects().read().iter() {
        if text == *root {
            return Located::ProjectRoot;
        }
        let Some(rest) = text
            .strip_prefix(root.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
        else {
            continue;
        };
        // The same span of the path as authored (case kept).
        let rest_original = &original[original.len() - rest.len()..];
        let tree = tree();
        if rest == PLUGINS_FOLDER {
            return Located::PluginsFolder;
        }
        let (mount, inner) = if rest == ENGINE_MOUNT {
            (ENGINE_MOUNT.to_owned(), String::new())
        } else if rest.starts_with("pulsar/") {
            (
                ENGINE_MOUNT.to_owned(),
                rest_original[ENGINE_MOUNT.len() + 1..].to_owned(),
            )
        } else if rest.starts_with("plugins/") {
            let after = &rest_original[PLUGINS_FOLDER.len() + 1..];
            let (id, inner) = after.split_once('/').unwrap_or((after, ""));
            let id = tree
                .mounts
                .keys()
                .filter_map(|name| name.strip_prefix("plugins/"))
                .find(|known| known.eq_ignore_ascii_case(id))
                .unwrap_or(id);
            (format!("{PLUGINS_FOLDER}/{id}"), inner.to_owned())
        } else {
            return Located::Outside;
        };
        let files = tree.mounts.get(&mount).cloned();
        return Located::Mount {
            mount,
            inner,
            files,
        };
    }
    Located::Outside
}

fn refuse(path: &Path) -> Result<()> {
    let mount = match locate(path) {
        Located::Outside | Located::ProjectRoot => return Ok(()),
        Located::PluginsFolder => PLUGINS_FOLDER.to_owned(),
        Located::Mount { mount, .. } => mount,
    };
    Err(anyhow::Error::new(ReadOnlyMount {
        mount,
        path: path.to_path_buf(),
    }))
}

fn not_found(path: &Path) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("'{}' is not in its mount", path.display()),
    ))
}

/// The mount folders a mounted project's root shows.
fn root_entries(tree: &Tree) -> Vec<FsEntry> {
    vec![dir_entry(ENGINE_MOUNT), dir_entry(PLUGINS_FOLDER)]
        .into_iter()
        .filter(|entry| {
            entry.name == ENGINE_MOUNT || tree.mounts.keys().any(|m| m.starts_with("plugins/"))
        })
        .collect()
}

/// The mount a project path is in, if any: `Some("plugins/pulsar.lights")`.
/// For the file drawer's read-only state.
pub fn mount_of(path: &Path) -> Option<String> {
    match locate(path) {
        Located::Outside | Located::ProjectRoot => None,
        Located::PluginsFolder => Some(PLUGINS_FOLDER.to_owned()),
        Located::Mount { mount, .. } => Some(mount),
    }
}

// ── The layer ────────────────────────────────────────────────────────────────

/// The active provider with the mounts composed over it.
pub struct MountedFs {
    base: Arc<dyn FsProvider>,
}

impl MountedFs {
    pub fn new(base: Arc<dyn FsProvider>) -> Self {
        Self { base }
    }

    /// The provider underneath, which knows no mounts.
    pub fn base(&self) -> &Arc<dyn FsProvider> {
        &self.base
    }
}

impl FsProvider for MountedFs {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        match locate(path) {
            Located::Mount { inner, files, .. } => files
                .and_then(|mount| mount.files.get(inner.as_str()).map(|b| b.to_vec()))
                .ok_or_else(|| not_found(path)),
            Located::PluginsFolder => Err(not_found(path)),
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

    fn open_write_stream(
        &self,
        path: &Path,
    ) -> Result<Option<Box<dyn std::io::Write + Send>>> {
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
                    .filter(|e| !(e.is_dir && (e.name == PLUGINS_FOLDER || e.name == ENGINE_MOUNT)))
                    .collect();
                entries.extend(root_entries(&tree()));
                Ok(entries)
            }
            Located::PluginsFolder => Ok(tree()
                .mounts
                .keys()
                .filter_map(|name| name.strip_prefix("plugins/"))
                .map(dir_entry)
                .collect()),
            Located::Mount { inner, files, .. } => match files {
                Some(mount) if mount.is_dir(&inner) => Ok(mount.children(&inner)),
                _ => Err(not_found(path)),
            },
        }
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        match locate(path) {
            // An existing mount folder already exists; anything else in a
            // mount cannot be created.
            Located::PluginsFolder => Ok(()),
            Located::Mount { inner, files, .. }
                if files.as_ref().is_some_and(|mount| mount.is_dir(&inner)) =>
            {
                Ok(())
            }
            Located::Mount { .. } => refuse(path),
            _ => self.base.create_dir_all(path),
        }
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.exists(path),
            Located::PluginsFolder => Ok(true),
            Located::Mount { inner, files, .. } => Ok(files.is_some_and(|mount| {
                mount.is_dir(&inner) || mount.files.contains_key(inner.as_str())
            })),
        }
    }

    fn metadata(&self, path: &Path) -> Result<FsMetadata> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.metadata(path),
            Located::PluginsFolder => Ok(FsMetadata {
                is_dir: true,
                size: 0,
                modified: None,
            }),
            Located::Mount { inner, files, .. } => {
                let mount = files.ok_or_else(|| not_found(path))?;
                if mount.is_dir(&inner) {
                    return Ok(FsMetadata {
                        is_dir: true,
                        size: 0,
                        modified: None,
                    });
                }
                let bytes = mount
                    .files
                    .get(inner.as_str())
                    .ok_or_else(|| not_found(path))?;
                Ok(FsMetadata {
                    is_dir: false,
                    size: bytes.len() as u64,
                    modified: None,
                })
            }
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        match locate(path) {
            Located::Outside | Located::ProjectRoot => self.base.canonicalize(path),
            // Mounted paths are virtual: they are their own canonical form.
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
        match locate(path) {
            Located::Outside => self.base.manifest(path),
            Located::ProjectRoot => {
                let shadowed = |entry: &ManifestEntry| {
                    let first = entry.path.split('/').next().unwrap_or_default();
                    first == PLUGINS_FOLDER || first == ENGINE_MOUNT
                };
                let mut out: Vec<ManifestEntry> = self
                    .base
                    .manifest(path)?
                    .into_iter()
                    .filter(|entry| !shadowed(entry))
                    .collect();
                let tree = tree();
                for entry in root_entries(&tree) {
                    out.push(ManifestEntry {
                        path: entry.name,
                        is_dir: true,
                        size: 0,
                        modified: None,
                    });
                }
                for (name, mount) in &tree.mounts {
                    if name != ENGINE_MOUNT {
                        out.push(ManifestEntry {
                            path: name.clone(),
                            is_dir: true,
                            size: 0,
                            modified: None,
                        });
                    }
                    out.extend(mount.manifest("").into_iter().map(|mut entry| {
                        entry.path = format!("{name}/{}", entry.path);
                        entry
                    }));
                }
                Ok(out)
            }
            Located::PluginsFolder => {
                let tree = tree();
                let mut out = Vec::new();
                for (name, mount) in &tree.mounts {
                    let Some(id) = name.strip_prefix("plugins/") else {
                        continue;
                    };
                    out.push(ManifestEntry {
                        path: id.to_owned(),
                        is_dir: true,
                        size: 0,
                        modified: None,
                    });
                    out.extend(mount.manifest("").into_iter().map(|mut entry| {
                        entry.path = format!("{id}/{}", entry.path);
                        entry
                    }));
                }
                Ok(out)
            }
            Located::Mount { inner, files, .. } => Ok(files
                .map(|mount| mount.manifest(&inner))
                .unwrap_or_default()),
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
mod tests {
    use super::*;
    use crate::virtual_fs;

    const FILES: &[MountedFile] = &[
        MountedFile {
            path: "mount_test/one.txt",
            bytes: b"one",
        },
        MountedFile {
            path: "mount_test/deep/two.txt",
            bytes: b"two!",
        },
    ];

    pulsar_plugins::inventory::submit! {
        MountedContent {
            origin: pulsar_plugins::engine_origin!(),
            id: "pulsar.mount_test",
            files: FILES,
        }
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        virtual_fs::write_file(&dir.path().join("own.txt"), b"mine").unwrap();
        mount_project(dir.path());
        dir
    }

    #[test]
    fn mounted_files_read_list_and_show_in_the_manifest() {
        let dir = project();
        let root = dir.path();
        assert_eq!(
            virtual_fs::read_file(&root.join("pulsar/mount_test/one.txt")).unwrap(),
            b"one"
        );
        assert_eq!(
            virtual_fs::read_to_string(&root.join("pulsar/mount_test/deep/two.txt")).unwrap(),
            "two!"
        );
        assert!(virtual_fs::is_dir(&root.join("pulsar/mount_test/deep")));
        assert!(!virtual_fs::exists(&root.join("pulsar/mount_test/none.txt")).unwrap());

        let names = |path: &Path| -> Vec<String> {
            let mut names: Vec<String> = virtual_fs::list_dir(path)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect();
            names.sort();
            names
        };
        assert!(names(root).contains(&"pulsar".to_owned()));
        assert!(names(root).contains(&"own.txt".to_owned()));
        assert_eq!(names(&root.join("pulsar/mount_test")), ["deep", "one.txt"]);

        let manifest: Vec<String> = virtual_fs::manifest(root)
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert!(manifest.contains(&"pulsar/mount_test/deep/two.txt".to_owned()));
        assert!(manifest.contains(&"own.txt".to_owned()));
        assert_eq!(
            mount_of(&root.join("pulsar/mount_test/one.txt")).as_deref(),
            Some("pulsar")
        );
        assert_eq!(mount_of(&root.join("own.txt")), None);
    }

    #[test]
    fn every_change_inside_a_mount_fails_with_the_named_error() {
        let dir = project();
        let root = dir.path();
        let file = root.join("pulsar/mount_test/one.txt");
        let refused = |result: Result<()>| {
            let error = result.unwrap_err();
            assert!(is_read_only(&error), "{error:#}");
            assert!(error.to_string().contains("read-only mount"), "{error}");
        };
        refused(virtual_fs::write_file(&file, b"x"));
        refused(virtual_fs::write_file_atomically(&file, b"x"));
        refused(virtual_fs::create_file(&root.join("pulsar/new.txt"), b"x"));
        refused(virtual_fs::append_file(&file, b"x"));
        refused(virtual_fs::delete_path(&file));
        refused(virtual_fs::delete_path(&root.join("pulsar")));
        refused(virtual_fs::rename(&file, &root.join("moved.txt")));
        refused(virtual_fs::rename(&root.join("own.txt"), &root.join("plugins/x/own.txt")));
        refused(virtual_fs::copy(&root.join("own.txt"), &file));
        refused(virtual_fs::create_dir_all(&root.join("pulsar/new_folder")));
        refused(virtual_fs::open_write(&file).map(|_| ()));
        // Copying out of a mount is reading it.
        virtual_fs::copy(&file, &root.join("copied.txt")).unwrap();
        assert_eq!(virtual_fs::read_file(&root.join("copied.txt")).unwrap(), b"one");
        // The project's own files are untouched by the layer.
        virtual_fs::write_file(&root.join("own.txt"), b"changed").unwrap();
    }

    #[test]
    fn an_on_disk_mount_folder_is_a_problem_and_is_shadowed() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("plugins/stale.txt");
        crate::LocalFsProvider::new().write_file(&disk, b"old").unwrap();
        let problems = mount_problems(dir.path());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("'plugins/'"), "{}", problems[0]);
        mount_project(dir.path());
        assert!(!virtual_fs::exists(&disk).unwrap(), "the mount shadows it");
        unmount_project(dir.path());
        assert!(virtual_fs::exists(&disk).unwrap());
    }

    #[test]
    fn an_unmounted_project_is_the_providers_alone() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!virtual_fs::exists(&dir.path().join("pulsar/mount_test/one.txt")).unwrap());
        virtual_fs::write_file(&dir.path().join("pulsar/real.txt"), b"x").unwrap();
    }
}
