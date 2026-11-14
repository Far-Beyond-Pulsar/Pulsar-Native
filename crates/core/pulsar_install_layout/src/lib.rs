//! The contract between the Hub (which installs engines, plugins and themes)
//! and the engine (which loads them).
//!
//! Both sides name every path through this crate, so a plugin the Hub installs
//! is where the engine looks for it, and a change to the layout is one change.
//!
//! ```text
//! <app data>/                       per user, shared by every engine install
//!   themes/                         one themes directory for all installs
//!   registries/                     cloned plugin and theme registries
//! <install root>/                   one engine install (a version)
//!   pulsar[.exe]                    the engine
//!   .pulsar-install.json            what the Hub installed (version, date)
//!   sdk/                            the engine SDK games build against
//!     pulsar-sdk.json               its index (pulsar_project::sdk)
//!   plugins/editor/                 this install's plugin libraries
//!     <plugin>.dll|so|dylib
//!     plugin_integrity.json         { file name: sha-256 hex } the engine requires
//!     plugins.lock.json             what the Hub installed here and from where
//! ```
//!
//! Themes are shared by all installs; plugins belong to one install, because a
//! plugin library only loads into the engine build it was compiled against.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The per-user app-data directory (`ProjectDirs` "com", "Pulsar", "Pulsar_Engine").
pub fn appdata_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The themes every engine install shares.
pub fn themes_dir() -> PathBuf {
    appdata_dir().join("themes")
}

/// What the Hub installed into [`themes_dir`] and from where. Kept beside the
/// directory, not in it: the engine's theme watcher parses every `*.json`
/// there as a theme.
pub fn themes_lock_path() -> PathBuf {
    appdata_dir().join("themes.lock.json")
}

/// Where plugin and theme registries are cloned.
pub fn registries_dir() -> PathBuf {
    appdata_dir().join("registries")
}

/// The file extension of a native plugin library on this platform.
pub const fn plugin_library_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// The file names that make a directory an engine install.
pub const ENGINE_BINARY_NAMES: &[&str] =
    &["pulsar.exe", "pulsar", "pulsar_engine.exe", "pulsar_engine"];

/// One engine install and the paths inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallLayout {
    root: PathBuf,
}

impl InstallLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The install the running engine belongs to: `PULSAR_INSTALL_DIR` if set,
    /// else the directory of the executable. A development build (run from
    /// `target/`, with `plugins/editor` in the working directory instead)
    /// falls back to the working directory so a checkout keeps working.
    pub fn of_running_engine() -> Self {
        if let Some(dir) = std::env::var_os("PULSAR_INSTALL_DIR").filter(|d| !d.is_empty()) {
            return Self::new(dir);
        }
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let cwd = std::env::current_dir().ok();
        match (exe_dir, cwd) {
            (Some(exe), Some(cwd))
                if !Self::new(&exe).plugins_dir().is_dir()
                    && Self::new(&cwd).plugins_dir().is_dir() =>
            {
                Self::new(cwd)
            }
            (Some(exe), _) => Self::new(exe),
            (None, Some(cwd)) => Self::new(cwd),
            (None, None) => Self::new("."),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The Hub's record of the install (`version`, `install_date`, `install_path`).
    pub fn metadata_path(&self) -> PathBuf {
        self.root.join(".pulsar-install.json")
    }

    /// The engine SDK this install builds games against: the normalized
    /// sources of the crates a game links and their index
    /// (`pulsar_project::sdk`), mirrored into each project's `.pulsar/sdk/`.
    pub fn sdk_dir(&self) -> PathBuf {
        self.root.join("sdk")
    }

    /// This install's plugin libraries.
    pub fn plugins_dir(&self) -> PathBuf {
        self.root.join("plugins").join("editor")
    }

    /// `{ file name: sha-256 hex }` of the libraries in [`Self::plugins_dir`];
    /// the engine loads no library it does not list.
    pub fn integrity_manifest_path(&self) -> PathBuf {
        self.plugins_dir().join("plugin_integrity.json")
    }

    /// What the Hub installed into [`Self::plugins_dir`] and from where.
    pub fn plugin_lock_path(&self) -> PathBuf {
        self.plugins_dir().join("plugins.lock.json")
    }

    pub fn is_engine_install(&self) -> bool {
        ENGINE_BINARY_NAMES
            .iter()
            .any(|name| self.root.join(name).is_file())
            || self.metadata_path().is_file()
            || self.root.join("Contents").join("Info.plist").exists()
    }

    pub fn load_plugin_lock(&self) -> PluginLock {
        read_json(&self.plugin_lock_path()).unwrap_or_default()
    }

    pub fn load_integrity_manifest(&self) -> IntegrityManifest {
        read_json(&self.integrity_manifest_path()).unwrap_or_default()
    }

    /// Record `record` (its library already in [`Self::plugins_dir`]): hash the
    /// file into the integrity manifest and list it in the lock, replacing an
    /// earlier record of the same repository (and deleting its old library).
    pub fn register_plugin(&self, mut record: PluginRecord) -> io::Result<PluginRecord> {
        let file = self.plugins_dir().join(&record.file);
        record.sha256 = hash_file(&file)?;
        let mut manifest = self.load_integrity_manifest();
        let mut lock = self.load_plugin_lock();
        if let Some(old) = lock.plugins.iter().find(|p| p.repo_url == record.repo_url) {
            if old.file != record.file {
                let _ = std::fs::remove_file(self.plugins_dir().join(&old.file));
                manifest.0.remove(&old.file);
            }
        }
        manifest
            .0
            .insert(record.file.clone(), record.sha256.clone());
        write_json(&self.integrity_manifest_path(), &manifest)?;
        lock.plugins
            .retain(|existing| existing.repo_url != record.repo_url);
        lock.plugins.push(record.clone());
        write_json(&self.plugin_lock_path(), &lock)?;
        Ok(record)
    }

    /// Delete the plugin's library and drop it from the manifest and the lock.
    pub fn unregister_plugin(&self, repo_url: &str) -> io::Result<()> {
        let mut lock = self.load_plugin_lock();
        let Some(index) = lock.plugins.iter().position(|p| p.repo_url == repo_url) else {
            return Ok(());
        };
        let record = lock.plugins.remove(index);
        match std::fs::remove_file(self.plugins_dir().join(&record.file)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut manifest = self.load_integrity_manifest();
        manifest.0.remove(&record.file);
        write_json(&self.integrity_manifest_path(), &manifest)?;
        write_json(&self.plugin_lock_path(), &lock)
    }
}

/// The flat `{ file name: sha-256 hex }` map the engine verifies libraries
/// against: a newtype over a map so it serializes exactly as that.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntegrityManifest(pub BTreeMap<String, String>);

/// A plugin the Hub installed into one install.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRecord {
    pub name: String,
    pub repo_url: String,
    pub version: String,
    /// The library's file name inside [`InstallLayout::plugins_dir`].
    pub file: String,
    #[serde(default)]
    pub sha256: String,
    /// The registry it came from (empty for a hand-installed one).
    #[serde(default)]
    pub registry_url: String,
    pub installed_at: String,
    /// `"binary"` (a release asset) or `"source"` (built by the Hub).
    pub method: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginLock {
    pub plugins: Vec<PluginRecord>,
}

/// A theme file the Hub installed into the shared themes directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeRecord {
    /// The theme file's display name (its `name` field).
    pub name: String,
    /// The file's name inside [`themes_dir`].
    pub file: String,
    pub sha256: String,
    /// The registry it came from.
    #[serde(default)]
    pub registry_url: String,
    pub installed_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeLock {
    pub themes: Vec<ThemeRecord>,
}

/// The themes the Hub installed, all engine installs sharing one directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeLayout {
    dir: PathBuf,
    lock: PathBuf,
}

impl ThemeLayout {
    /// The per-user shared themes directory.
    pub fn shared() -> Self {
        Self {
            dir: themes_dir(),
            lock: themes_lock_path(),
        }
    }

    /// A themes directory and lock of your choosing (tests).
    pub fn at(dir: impl Into<PathBuf>, lock: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            lock: lock.into(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn load_lock(&self) -> ThemeLock {
        read_json(&self.lock).unwrap_or_default()
    }

    /// Record a theme file already written to [`Self::dir`].
    pub fn register_theme(&self, mut record: ThemeRecord) -> io::Result<ThemeRecord> {
        record.sha256 = hash_file(&self.dir.join(&record.file))?;
        let mut lock = self.load_lock();
        lock.themes.retain(|existing| existing.file != record.file);
        lock.themes.push(record.clone());
        write_json(&self.lock, &lock)?;
        Ok(record)
    }

    /// Delete an installed theme file and drop it from the lock.
    pub fn unregister_theme(&self, file: &str) -> io::Result<()> {
        let mut lock = self.load_lock();
        lock.themes.retain(|existing| existing.file != file);
        match std::fs::remove_file(self.dir.join(file)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        write_json(&self.lock, &lock)
    }
}

/// The lower-case sha-256 hex digest of a file.
pub fn hash_file(path: &Path) -> io::Result<String> {
    use io::Read as _;
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Write through a temporary file so a crash never leaves a half-written
/// manifest, which the engine would reject every plugin over.
fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(file: &str, repo: &str) -> PluginRecord {
        PluginRecord {
            name: "Thing".into(),
            repo_url: repo.into(),
            version: "1.0.0".into(),
            file: file.into(),
            sha256: String::new(),
            registry_url: String::new(),
            installed_at: "now".into(),
            method: "binary".into(),
        }
    }

    #[test]
    fn register_hashes_the_library_into_the_flat_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let layout = InstallLayout::new(dir.path());
        std::fs::create_dir_all(layout.plugins_dir()).unwrap();
        std::fs::write(layout.plugins_dir().join("a.dll"), b"abc").unwrap();
        let recorded = layout.register_plugin(record("a.dll", "r/a")).unwrap();
        assert_eq!(
            recorded.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let manifest: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(layout.integrity_manifest_path()).unwrap())
                .unwrap();
        assert_eq!(manifest["a.dll"], recorded.sha256);
    }

    #[test]
    fn upgrading_replaces_the_old_library_and_unregistering_removes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let layout = InstallLayout::new(dir.path());
        std::fs::create_dir_all(layout.plugins_dir()).unwrap();
        std::fs::write(layout.plugins_dir().join("a_1.dll"), b"1").unwrap();
        layout.register_plugin(record("a_1.dll", "r/a")).unwrap();
        std::fs::write(layout.plugins_dir().join("a_2.dll"), b"2").unwrap();
        layout.register_plugin(record("a_2.dll", "r/a")).unwrap();
        assert_eq!(layout.load_plugin_lock().plugins.len(), 1);
        assert!(!layout.plugins_dir().join("a_1.dll").exists());
        assert!(!layout.load_integrity_manifest().0.contains_key("a_1.dll"));
        layout.unregister_plugin("r/a").unwrap();
        assert!(layout.load_plugin_lock().plugins.is_empty());
        assert!(layout.load_integrity_manifest().0.is_empty());
        assert!(!layout.plugins_dir().join("a_2.dll").exists());
    }

    #[test]
    fn a_theme_is_recorded_beside_not_inside_the_themes_directory() {
        let dir = tempfile::tempdir().unwrap();
        let themes = ThemeLayout::at(
            dir.path().join("themes"),
            dir.path().join("themes.lock.json"),
        );
        std::fs::create_dir_all(themes.dir()).unwrap();
        std::fs::write(themes.dir().join("nord.json"), b"{}").unwrap();
        let recorded = themes
            .register_theme(ThemeRecord {
                name: "Nord".into(),
                file: "nord.json".into(),
                sha256: String::new(),
                registry_url: String::new(),
                installed_at: "now".into(),
            })
            .unwrap();
        assert_eq!(recorded.sha256.len(), 64);
        assert_eq!(themes.load_lock().themes.len(), 1);
        assert!(!themes.dir().join("themes.lock.json").exists());
        themes.unregister_theme("nord.json").unwrap();
        assert!(themes.load_lock().themes.is_empty());
        assert!(!themes.dir().join("nord.json").exists());
    }
}
