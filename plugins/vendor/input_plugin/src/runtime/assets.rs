//! Finding the project's input assets while the game runs, and noticing when
//! they change on disk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::library::{project_root, Library, MappingEntry};
use crate::model::AssetId;

/// Look again for a mapping that isn't found at most this often.
const RESCAN_MISSING: Duration = Duration::from_secs(1);

static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Use `root` as the project, instead of finding it. For hosts that know
/// better than the environment does.
pub fn set_project_root(root: Option<PathBuf>) {
    *ROOT_OVERRIDE.lock().unwrap_or_else(|p| p.into_inner()) = root;
}

/// The project the running game belongs to: the root set with
/// [`set_project_root`], else `PULSAR_PROJECT_ROOT` (which Play-in-Editor
/// sets), else the nearest project around the working directory or the
/// executable.
pub fn find_project_root() -> Option<PathBuf> {
    if let Some(root) = ROOT_OVERRIDE.lock().unwrap_or_else(|p| p.into_inner()).clone() {
        return Some(root);
    }
    if let Some(root) = std::env::var_os("PULSAR_PROJECT_ROOT").map(PathBuf::from).filter(|p| p.is_dir()) {
        return Some(root);
    }
    let is_project = |dir: &Path| dir.join("Pulsar.toml").is_file() || dir.join(".pulsar").is_dir();
    let from_cwd = std::env::current_dir().ok().and_then(|cwd| cwd.ancestors().find(|d| is_project(d)).map(Path::to_path_buf));
    from_cwd.or_else(|| {
        let exe = std::env::current_exe().ok()?;
        exe.ancestors().skip(1).find(|d| is_project(d)).map(Path::to_path_buf)
    })
}

/// Files and when they were last written, to tell when one changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamps(Vec<(PathBuf, Option<SystemTime>)>);

impl Stamps {
    pub fn of(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self(paths.into_iter().map(|p| {
            let at = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            (p, at)
        }).collect())
    }

    /// Whether any of the files was written, removed or created since.
    pub fn changed(&self) -> bool {
        self.0.iter().any(|(p, at)| std::fs::metadata(p).and_then(|m| m.modified()).ok() != *at)
    }
}

/// The project's input assets, scanned on first use.
pub struct Assets {
    root: Option<PathBuf>,
    library: Option<Arc<Library>>,
    scanned: Option<Instant>,
}

impl Assets {
    /// `root` is the project; `None` finds it.
    pub fn new(root: Option<PathBuf>) -> Self {
        Self { root: root.or_else(find_project_root), library: None, scanned: None }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The library, scanning the project the first time.
    pub fn library(&mut self) -> Arc<Library> {
        if self.library.is_none() {
            self.rescan();
        }
        self.library.clone().unwrap_or_else(|| Arc::new(Library::empty(PathBuf::new())))
    }

    /// Read the project again.
    pub fn rescan(&mut self) {
        self.scanned = Some(Instant::now());
        self.library = Some(Arc::new(match &self.root {
            Some(root) => Library::scan(root),
            None => Library::empty(PathBuf::new()),
        }));
    }

    /// The mapping `key` names: its project-relative path, a path to the
    /// file, or its id. A mapping that isn't found triggers a fresh scan, at
    /// most once a second, in case it was just made.
    pub fn mapping(&mut self, key: &str) -> Option<MappingEntry> {
        let root = self.root.clone();
        let find = |lib: &Library| -> Option<MappingEntry> {
            if let Some(id) = AssetId::parse(key) {
                return lib.mapping_entry(&id).cloned();
            }
            let relative = match &root {
                Some(root) => crate::library::relative_path(root, Path::new(key)),
                None => key.to_string(),
            };
            lib.mapping_at_relative(&relative).cloned()
        };
        let library = self.library();
        if let Some(found) = find(&library) {
            return Some(found);
        }
        if self.scanned.is_some_and(|at| at.elapsed() < RESCAN_MISSING) {
            return None;
        }
        self.rescan();
        let library = self.library();
        find(&library)
    }

    /// The project root a mapping's library paths are relative to.
    pub fn resolve_root(file: &Path) -> PathBuf {
        project_root(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Asset, MappingAsset};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("input_plugin_assets_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_mapping_is_found_by_relative_path_absolute_path_or_id() {
        let root = dir("find");
        let map = MappingAsset::new();
        std::fs::create_dir_all(root.join("input")).unwrap();
        std::fs::write(root.join("input/Game.inputmap"), map.to_text()).unwrap();
        let mut assets = Assets::new(Some(root.clone()));
        assert_eq!(assets.mapping("input/Game.inputmap").unwrap().id, map.id);
        assert_eq!(assets.mapping(r"input\Game.inputmap").unwrap().id, map.id);
        assert_eq!(assets.mapping(root.join("input/Game.inputmap").to_str().unwrap()).unwrap().id, map.id);
        assert_eq!(assets.mapping(map.id.as_str()).unwrap().id, map.id);
        assert!(assets.mapping("nope.inputmap").is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_mapping_made_after_the_first_scan_is_found_on_a_later_try() {
        let root = dir("late");
        let mut assets = Assets::new(Some(root.clone()));
        assert!(assets.mapping("Late.inputmap").is_none());
        let map = MappingAsset::new();
        std::fs::write(root.join("Late.inputmap"), map.to_text()).unwrap();
        assert!(assets.mapping("Late.inputmap").is_none(), "scans aren't repeated every frame");
        assets.scanned = Some(Instant::now() - RESCAN_MISSING * 2);
        assert_eq!(assets.mapping("Late.inputmap").unwrap().id, map.id);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn stamps_notice_a_write_a_removal_and_a_creation() {
        let root = dir("stamps");
        let file = root.join("a.txt");
        let stamps = Stamps::of([file.clone()]);
        assert!(!stamps.changed());
        std::fs::write(&file, "x").unwrap();
        assert!(stamps.changed(), "created");
        let stamps = Stamps::of([file.clone()]);
        assert!(!stamps.changed());
        std::fs::remove_file(&file).unwrap();
        assert!(stamps.changed(), "removed");
        std::fs::remove_dir_all(&root).ok();
    }
}
