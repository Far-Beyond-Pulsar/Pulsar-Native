//! Where a running game's content comes from.
//!
//! [`ContentRoot::discover`] decides at startup, from the executable's
//! location (never from a path baked in at compile time):
//!
//! 1. `PULSAR_CONTENT_DIR` (env): that directory, as shipped content.
//! 2. `PULSAR_PROJECT_ROOT` (env): that project, as loose dev files.
//! 3. `<exe dir>/Content` holding cooked content (`game.pak`, or a loose
//!    cook with `Pulsar/class_index.json`): a **packaged** game.
//! 4. The nearest ancestor of the executable that is a project (has a
//!    `Pulsar/` or `.pulsar/` directory): a **dev** build run from its
//!    project's `target/` (`cargo run`).
//! 5. The working directory, if it is a project.
//!
//! Play-in-Editor does not discover: the editor passes the project root
//! ([`ContentRoot::project`]).
//!
//! Every content path is `root.join(<content-relative path>)`. With a pak,
//! [`install`](ContentRoot::install) routes `engine_fs::virtual_fs` reads
//! under the root to the pak ([`ContentFsProvider`](crate::ContentFsProvider)),
//! so loaders that take paths (levels, classes, modules, settings) read
//! from it unchanged.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::pak::{PakReader, PAK_FILE_NAME};
use crate::settings::{ProjectSettings, PROJECT_SETTINGS_FILE};

/// The content directory next to a packaged game's executable.
pub const CONTENT_DIR_NAME: &str = "Content";
/// Environment override: use this directory as shipped content.
pub const CONTENT_DIR_ENV: &str = "PULSAR_CONTENT_DIR";
/// Environment override: use this project root (loose dev files).
pub const PROJECT_ROOT_ENV: &str = "PULSAR_PROJECT_ROOT";
/// Environment override: where a pak's assets are unpacked for loaders
/// that only read files (see [`ContentRoot::asset_root`]).
pub const ASSET_CACHE_ENV: &str = "PULSAR_ASSET_CACHE";

#[derive(Clone, Debug)]
enum Source {
    /// A project directory (editor, Play-in-Editor, `cargo run`).
    Project,
    /// Cooked content: from `game.pak` when present, else loose files.
    Packaged { pak: Option<Arc<PakReader>> },
}

/// A game's content: a directory, and maybe a pak inside it.
#[derive(Clone, Debug)]
pub struct ContentRoot {
    root: PathBuf,
    source: Source,
}

impl ContentRoot {
    /// A project's own files, loose (dev builds).
    pub fn project(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), source: Source::Project }
    }

    /// Cooked content in `dir`: `dir/game.pak` when it exists, loose files
    /// otherwise.
    pub fn packaged(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let root = dir.into();
        let pak_path = root.join(PAK_FILE_NAME);
        let pak = if pak_path.is_file() {
            Some(Arc::new(PakReader::open(&pak_path).map_err(io::Error::from)?))
        } else if root.join(crate::CLASS_INDEX_FILE).is_file() || root.join(PROJECT_SETTINGS_FILE).is_file() {
            None
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} holds no cooked content ({PAK_FILE_NAME} or loose files)", root.display()),
            ));
        };
        Ok(Self { root, source: Source::Packaged { pak } })
    }

    /// Open `dir` as whatever it is: cooked content, or a project.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        if is_cooked(&dir) {
            Self::packaged(dir)
        } else if dir.is_dir() {
            Ok(Self::project(dir))
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, format!("{} does not exist", dir.display())))
        }
    }

    /// Find this process's content. See the module doc for the order.
    pub fn discover() -> Result<Self, String> {
        if let Some(dir) = std::env::var_os(CONTENT_DIR_ENV).filter(|v| !v.is_empty()) {
            return Self::packaged(PathBuf::from(dir)).map_err(|e| format!("{CONTENT_DIR_ENV}: {e}"));
        }
        if let Some(dir) = std::env::var_os(PROJECT_ROOT_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::project(PathBuf::from(dir)));
        }
        let exe = std::env::current_exe().map_err(|e| format!("cannot locate the executable: {e}"))?;
        let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        Self::discover_from(&exe_dir, std::env::current_dir().ok().as_deref())
    }

    /// [`discover`](Self::discover) for an executable in `exe_dir`, without
    /// the environment overrides.
    pub fn discover_from(exe_dir: &Path, cwd: Option<&Path>) -> Result<Self, String> {
        let content = exe_dir.join(CONTENT_DIR_NAME);
        if is_cooked(&content) {
            return Self::packaged(content).map_err(|e| e.to_string());
        }
        if let Some(project) = exe_dir.ancestors().find(|dir| is_project_root(dir)) {
            return Ok(Self::project(project));
        }
        if let Some(cwd) = cwd.filter(|dir| is_project_root(dir)) {
            return Ok(Self::project(cwd));
        }
        Err(format!(
            "no game content found: expected {} next to the executable, or a project around it \
             (set {CONTENT_DIR_ENV} or {PROJECT_ROOT_ENV} to override)",
            content.display()
        ))
    }

    /// The content directory (packaged) or project root (dev).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether this is cooked, shipped content.
    pub fn is_packaged(&self) -> bool {
        matches!(self.source, Source::Packaged { .. })
    }

    /// The pak, when content comes from one.
    pub fn pak(&self) -> Option<&Arc<PakReader>> {
        match &self.source {
            Source::Packaged { pak } => pak.as_ref(),
            Source::Project => None,
        }
    }

    /// The path content-relative `rel` has under the root. With a pak it
    /// may exist only virtually: read it through [`read`](Self::read) or
    /// `engine_fs::virtual_fs` after [`install`](Self::install).
    pub fn path(&self, rel: &str) -> PathBuf {
        match crate::normalize_rel(rel) {
            Some(rel) => self.root.join(rel),
            None => self.root.clone(),
        }
    }

    /// Content-relative form of `path`, when it is under the root.
    pub fn relative(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?;
        crate::normalize_rel(&rel.to_string_lossy())
    }

    /// Read content-relative `rel`: from the pak when it has it, else the
    /// loose file.
    pub fn read(&self, rel: &str) -> io::Result<Vec<u8>> {
        if let Some(pak) = self.pak() {
            if pak.contains(rel) {
                return pak.read(rel).map_err(io::Error::from);
            }
        }
        std::fs::read(self.path(rel))
    }

    /// Whether content-relative `rel` exists (in the pak or loose).
    pub fn exists(&self, rel: &str) -> bool {
        self.pak().is_some_and(|pak| pak.contains(rel)) || self.path(rel).is_file()
    }

    /// `Pulsar/project.json`, or the defaults when it is missing or
    /// unreadable (with a warning).
    pub fn settings(&self) -> ProjectSettings {
        match self.read(PROJECT_SETTINGS_FILE) {
            Ok(bytes) => ProjectSettings::from_json(&bytes).unwrap_or_else(|error| {
                tracing::warn!("{error}; using default project settings");
                ProjectSettings::default()
            }),
            Err(_) => ProjectSettings::default(),
        }
    }

    /// Make this the process's content: [`current`] returns it and, for a
    /// pak, `engine_fs::virtual_fs` reads under the root come from it.
    pub fn install(&self) {
        if let Some(pak) = self.pak() {
            engine_fs::virtual_fs::set_provider(Arc::new(crate::ContentFsProvider::new(
                self.root.clone(),
                Arc::clone(pak),
            )));
        }
        set_current(Some(self.clone()));
        tracing::info!(
            root = %self.root.display(),
            packaged = self.is_packaged(),
            pak = self.pak().is_some(),
            "Game content"
        );
    }

    /// A real directory where `assets/...` files exist on disk, for loaders
    /// that only open files by path (Helio's mesh loader). Loose content
    /// and projects: the root itself. A pak: its `assets/` entries,
    /// unpacked once into a cache directory keyed by the pak's contents
    /// (`PULSAR_ASSET_CACHE`, or the system temp directory).
    ///
    /// Stopgap until those loaders read through the content root: they
    /// then read the pak directly and nothing is unpacked.
    pub fn asset_root(&self) -> io::Result<PathBuf> {
        let base = std::env::var_os(ASSET_CACHE_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("pulsar-asset-cache"));
        self.asset_root_in(&base)
    }

    /// [`asset_root`](Self::asset_root) with an explicit cache directory.
    pub fn asset_root_in(&self, base: &Path) -> io::Result<PathBuf> {
        let Some(pak) = self.pak() else {
            return Ok(self.root.clone());
        };
        let dir = base.join(format!("{:032x}", pak.content_id()));
        let complete = dir.join(".complete");
        if complete.is_file() {
            return Ok(dir);
        }
        let mut unpacked = 0usize;
        for path in pak.entries().keys().filter(|p| p.starts_with("assets/")) {
            let out = dir.join(path);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Write-then-rename: a concurrent reader never sees half a file.
            let tmp = out.with_extension("unpacking");
            std::fs::write(&tmp, pak.read(path).map_err(io::Error::from)?)?;
            std::fs::rename(&tmp, &out)?;
            unpacked += 1;
        }
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&complete, b"")?;
        tracing::info!(dir = %dir.display(), files = unpacked, "Unpacked pak assets for file-based loaders");
        Ok(dir)
    }
}

/// Whether `dir` holds cooked content.
pub fn is_cooked(dir: &Path) -> bool {
    dir.join(PAK_FILE_NAME).is_file() || dir.join(crate::CLASS_INDEX_FILE).is_file()
}

/// Whether `dir` is a Pulsar project root.
pub fn is_project_root(dir: &Path) -> bool {
    (dir.join("Pulsar").is_dir() || dir.join(".pulsar").is_dir()) && !is_cooked(dir)
}

static CURRENT: RwLock<Option<ContentRoot>> = RwLock::new(None);

/// The process's content, once [`ContentRoot::install`]ed.
pub fn current() -> Option<ContentRoot> {
    CURRENT.read().clone()
}

/// Replace (or clear) the process's content without touching the virtual
/// filesystem.
pub fn set_current(root: Option<ContentRoot>) {
    *CURRENT.write() = root;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pak::PakWriter;

    #[test]
    fn discovery_prefers_content_next_to_the_exe_then_a_project_around_it() {
        let tmp = tempfile::tempdir().unwrap();
        // A dev build: <project>/target/debug/game.
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(project.join("Pulsar")).unwrap();
        let exe_dir = project.join("target").join("debug");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let found = ContentRoot::discover_from(&exe_dir, None).unwrap();
        assert!(!found.is_packaged());
        assert_eq!(found.root(), project);

        // A packaged game: <dir>/Content/game.pak next to the exe.
        let shipped = tmp.path().join("shipped");
        std::fs::create_dir_all(shipped.join(CONTENT_DIR_NAME)).unwrap();
        let mut pak = PakWriter::create(shipped.join(CONTENT_DIR_NAME).join(PAK_FILE_NAME)).unwrap();
        pak.add(PROJECT_SETTINGS_FILE, br#"{"startup_level":"scenes/a.level"}"#).unwrap();
        pak.add("assets/meshes/a.mesh", b"mesh").unwrap();
        pak.finish().unwrap();
        let found = ContentRoot::discover_from(&shipped, Some(&project)).unwrap();
        assert!(found.is_packaged());
        assert!(found.pak().is_some());
        assert_eq!(found.settings().startup_level.as_deref(), Some("scenes/a.level"));
        assert!(found.exists("assets/meshes/a.mesh"));
        assert_eq!(found.read("assets/meshes/a.mesh").unwrap(), b"mesh");

        // Nothing around: the working directory, then an error.
        let lost = tmp.path().join("lost");
        std::fs::create_dir_all(&lost).unwrap();
        assert_eq!(ContentRoot::discover_from(&lost, Some(&project)).unwrap().root(), project);
        assert!(ContentRoot::discover_from(&lost, None).is_err());
    }

    #[test]
    fn pak_assets_unpack_once_for_file_loaders() {
        let tmp = tempfile::tempdir().unwrap();
        let content = tmp.path().join(CONTENT_DIR_NAME);
        std::fs::create_dir_all(&content).unwrap();
        let mut pak = PakWriter::create(content.join(PAK_FILE_NAME)).unwrap();
        pak.add("assets/meshes/a.mesh", b"mesh").unwrap();
        pak.add("scenes/a.level", b"{}").unwrap();
        pak.finish().unwrap();
        let root = ContentRoot::packaged(&content).unwrap();

        let assets = root.asset_root_in(&tmp.path().join("cache")).unwrap();
        // Unpacked once: a second call finds the cache complete.
        assert_eq!(root.asset_root_in(&tmp.path().join("cache")).unwrap(), assets);
        assert!(assets.starts_with(tmp.path().join("cache")));
        assert_eq!(std::fs::read(assets.join("assets/meshes/a.mesh")).unwrap(), b"mesh");
        assert!(!assets.join("scenes/a.level").exists(), "only assets are unpacked");

        // Loose content is its own asset root.
        let loose = ContentRoot::project(tmp.path());
        assert_eq!(loose.asset_root().unwrap(), tmp.path());
    }
}
