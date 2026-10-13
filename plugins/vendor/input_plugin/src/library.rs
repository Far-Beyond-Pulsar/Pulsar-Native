//! The project's input actions and mappings, found by id.
//!
//! Mappings refer to actions by [`AssetId`] only. The library is what turns an
//! id back into a file: it scans the project for `.inputaction` and
//! `.inputmap` files, reads each one's id, and answers lookups. Because the id
//! lives inside the file, renaming or moving an action (in the editor, in the
//! file manager, or in git) never breaks a mapping; the next scan just finds
//! it at its new path.
//!
//! Two files can end up with the same id (a copied file). The library does not
//! guess which is "the" asset: it reports the id as ambiguous and every
//! reference to it as unresolved until one of the copies gets a new id.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::model::action::ACTION_EXTENSION;
use crate::model::mapping::MAPPING_EXTENSION;
use crate::model::validate::{ActionInfo, ActionSource};
use crate::model::{peek_id, ActionAsset, AssetId, MappingAsset};
use crate::model::Asset;

/// Directories never scanned.
const SKIPPED_DIRS: [&str; 5] = ["target", "node_modules", ".git", ".pulsar", ".cargo"];

/// Stop scanning after this many files, so a huge directory can't stall the editor.
const MAX_FILES: usize = 200_000;

/// The project directory `file` belongs to: the nearest ancestor with a
/// `Pulsar.toml`, else one with a `.pulsar` directory, else the
/// file's own directory.
pub fn project_root(file: &Path) -> PathBuf {
    let start = file.parent().unwrap_or(file);
    let ancestors = || start.ancestors();
    ancestors()
        .find(|d| d.join("Pulsar.toml").is_file())
        .or_else(|| ancestors().find(|d| d.join(".pulsar").is_dir()))
        .unwrap_or(start)
        .to_path_buf()
}

/// `path` relative to `root`, with `/` separators; `path` itself if it isn't
/// inside `root`.
pub fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// One action file in the project.
#[derive(Clone, Debug)]
pub struct ActionEntry {
    pub id: AssetId,
    pub path: PathBuf,
    /// Project-relative, `/` separated.
    pub relative: String,
    /// The file name without its extension.
    pub stem: String,
    pub asset: Arc<ActionAsset>,
}

impl ActionEntry {
    /// The name its events go by.
    pub fn event_name(&self) -> String {
        self.asset.event_name(&self.stem)
    }
}

/// One mapping file in the project.
#[derive(Clone, Debug)]
pub struct MappingEntry {
    pub id: AssetId,
    pub path: PathBuf,
    pub relative: String,
    pub stem: String,
    pub asset: Arc<MappingAsset>,
}

/// A file the library could not index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    pub kind: ProblemKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// The file has no id yet. Opening it in the editor assigns one.
    NoId,
    Unreadable(String),
}

/// An index of the project's input assets at one moment.
#[derive(Clone, Debug, Default)]
pub struct Library {
    root: PathBuf,
    actions: HashMap<AssetId, Vec<ActionEntry>>,
    mappings: HashMap<AssetId, Vec<MappingEntry>>,
    problems: Vec<Problem>,
}

impl Library {
    /// An empty library, for before the first scan finishes.
    pub fn empty(root: PathBuf) -> Self {
        Self { root, ..Default::default() }
    }

    /// Scan `root` for input assets. Reads every `.inputaction` and
    /// `.inputmap` file, so call it off the UI thread for large projects.
    pub fn scan(root: &Path) -> Self {
        let mut library = Self::empty(root.to_path_buf());
        let mut files = Vec::new();
        collect(root, &mut files);
        for (path, is_action) in files {
            library.add_file(&path, is_action);
        }
        library.problems.sort_by(|a, b| a.path.cmp(&b.path));
        library
    }

    fn add_file(&mut self, path: &Path, is_action: bool) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.problems.push(Problem { path: path.to_path_buf(), kind: ProblemKind::Unreadable(error.to_string()) });
                return;
            }
        };
        // A file with no id can't be referenced yet; it is not an error.
        if peek_id(&text).is_none() {
            let kind = match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(serde_json::Value::Object(_)) => ProblemKind::NoId,
                _ if text.trim().is_empty() => ProblemKind::NoId,
                _ => ProblemKind::Unreadable("not an input file".to_string()),
            };
            self.problems.push(Problem { path: path.to_path_buf(), kind });
            return;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let relative = relative_path(&self.root, path);
        if is_action {
            match ActionAsset::parse(&text) {
                Ok((asset, _)) => {
                    let id = asset.id.clone();
                    self.actions.entry(id.clone()).or_default().push(ActionEntry { id, path: path.to_path_buf(), relative, stem, asset: Arc::new(asset) });
                }
                Err(error) => self.problems.push(Problem { path: path.to_path_buf(), kind: ProblemKind::Unreadable(error) }),
            }
        } else {
            match MappingAsset::parse(&text) {
                Ok((asset, _)) => {
                    let id = asset.id.clone();
                    self.mappings.entry(id.clone()).or_default().push(MappingEntry { id, path: path.to_path_buf(), relative, stem, asset: Arc::new(asset) });
                }
                Err(error) => self.problems.push(Problem { path: path.to_path_buf(), kind: ProblemKind::Unreadable(error) }),
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    // ── Actions ────────────────────────────────────────────────────────────

    /// The action `id`, unless it is missing or ambiguous.
    pub fn action_entry(&self, id: &AssetId) -> Option<&ActionEntry> {
        match self.actions.get(id).map(Vec::as_slice) {
            Some([only]) => Some(only),
            _ => None,
        }
    }

    /// Every file that claims the action `id` (more than one: a copied file).
    pub fn action_claimants(&self, id: &AssetId) -> &[ActionEntry] {
        self.actions.get(id).map_or(&[], Vec::as_slice)
    }

    /// All actions, by name then path.
    pub fn actions(&self) -> Vec<&ActionEntry> {
        let mut all: Vec<&ActionEntry> = self.actions.values().flatten().collect();
        all.sort_by(|a, b| a.stem.to_lowercase().cmp(&b.stem.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
        all
    }

    /// The id of the action file at `path`, if the library has indexed it.
    pub fn action_id_at(&self, path: &Path) -> Option<&AssetId> {
        self.actions.values().flatten().find(|e| same_file(&e.path, path)).map(|e| &e.id)
    }

    // ── Mappings ───────────────────────────────────────────────────────────

    pub fn mapping_entry(&self, id: &AssetId) -> Option<&MappingEntry> {
        match self.mappings.get(id).map(Vec::as_slice) {
            Some([only]) => Some(only),
            _ => None,
        }
    }

    pub fn mapping_claimants(&self, id: &AssetId) -> &[MappingEntry] {
        self.mappings.get(id).map_or(&[], Vec::as_slice)
    }

    /// All mappings, by name then path.
    pub fn mappings(&self) -> Vec<&MappingEntry> {
        let mut all: Vec<&MappingEntry> = self.mappings.values().flatten().collect();
        all.sort_by(|a, b| a.stem.to_lowercase().cmp(&b.stem.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
        all
    }

    /// The mapping at a project-relative `path` (any separators, any case of
    /// the extension).
    pub fn mapping_at_relative(&self, path: &str) -> Option<&MappingEntry> {
        let wanted = path.replace('\\', "/");
        self.mappings.values().flatten().find(|e| e.relative.eq_ignore_ascii_case(&wanted))
    }

    // ── The whole ──────────────────────────────────────────────────────────

    pub fn len(&self) -> usize {
        self.actions.values().map(Vec::len).sum::<usize>() + self.mappings.values().map(Vec::len).sum::<usize>()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && self.mappings.is_empty()
    }

    /// Action ids claimed by more than one file.
    pub fn action_conflicts(&self) -> Vec<(&AssetId, &[ActionEntry])> {
        let mut out: Vec<_> = self.actions.iter().filter(|(_, v)| v.len() > 1).map(|(k, v)| (k, v.as_slice())).collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }

    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The mappings that bind the action `id`.
    pub fn mappings_using(&self, id: &AssetId) -> Vec<&MappingEntry> {
        self.mappings().into_iter().filter(|m| m.asset.used_actions().contains(id)).collect()
    }

    /// For each action, how many mappings bind it.
    pub fn usage_counts(&self) -> BTreeMap<AssetId, usize> {
        let mut counts = BTreeMap::new();
        for mapping in self.mappings.values().flatten() {
            for id in mapping.asset.used_actions() {
                *counts.entry(id).or_insert(0) += 1;
            }
        }
        counts
    }

    /// A copy of the library in which the action at `path` is `action`
    /// instead of what is on disk, so checks see unsaved edits.
    pub fn with_current_action(&self, path: &Path, action: &ActionAsset) -> Library {
        let mut copy = self.clone();
        for entries in copy.actions.values_mut() {
            entries.retain(|e| !same_file(&e.path, path));
        }
        copy.actions.retain(|_, v| !v.is_empty());
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        copy.actions.entry(action.id.clone()).or_default().push(ActionEntry {
            id: action.id.clone(),
            path: path.to_path_buf(),
            relative: relative_path(&self.root, path),
            stem,
            asset: Arc::new(action.clone()),
        });
        copy
    }
}

impl ActionSource for Library {
    fn action(&self, id: &AssetId) -> Option<ActionInfo> {
        self.action_entry(id).map(|e| ActionInfo {
            name: e.event_name(),
            value_type: e.asset.value_type,
            path: e.relative.clone(),
            consume_input: e.asset.consume_input,
        })
    }

    fn is_ambiguous(&self, id: &AssetId) -> bool {
        self.action_claimants(id).len() > 1
    }

    fn name_clash(&self, name: &str, me: &AssetId) -> Option<String> {
        self.actions
            .values()
            .flatten()
            .find(|e| &e.id != me && e.event_name().eq_ignore_ascii_case(name))
            .map(|e| e.relative.clone())
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn collect(dir: &Path, out: &mut Vec<(PathBuf, bool)>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if kind.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_ref()) && !name.starts_with('.') {
                collect(&path, out);
            }
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case(ACTION_EXTENSION) {
                out.push((path, true));
            } else if ext.eq_ignore_ascii_case(MAPPING_EXTENSION) {
                out.push((path, false));
            }
        }
    }
}

/// Write an id into the file at `path` if it has none, changing nothing
/// else. Gives older or hand-made files their stable id.
pub fn stamp_file(path: &Path) -> Result<AssetId, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let is_action = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ACTION_EXTENSION));
    if is_action {
        let (mut asset, _) = ActionAsset::parse(&text)?;
        if asset.stamp() {
            std::fs::write(path, asset.to_text()).map_err(|e| e.to_string())?;
        }
        Ok(asset.id)
    } else {
        let (mut asset, _) = MappingAsset::parse(&text)?;
        if asset.stamp() {
            std::fs::write(path, asset.to_text()).map_err(|e| e.to_string())?;
        }
        Ok(asset.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ActionRef;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("input_plugin_lib_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: String) -> PathBuf {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn actions_and_mappings_are_found_by_id_wherever_they_are() {
        let dir = project("find");
        let jump = ActionAsset::new();
        let map = MappingAsset::new();
        write(&dir, "input/Jump.inputaction", jump.to_text());
        write(&dir, "deep/er/Gameplay.INPUTMAP", map.to_text());
        write(&dir, "target/Skipped.inputaction", ActionAsset::new().to_text());
        write(&dir, "notes.txt", "x".into());

        let lib = Library::scan(&dir);
        assert_eq!(lib.len(), 2);
        let found = lib.action_entry(&jump.id).unwrap();
        assert_eq!(found.stem, "Jump");
        assert_eq!(found.relative, "input/Jump.inputaction");
        assert_eq!(lib.mapping_entry(&map.id).unwrap().stem, "Gameplay");
        assert!(lib.problems().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn moving_an_action_keeps_the_mapping_working() {
        let dir = project("move");
        let jump = ActionAsset::new();
        let from = write(&dir, "input/Jump.inputaction", jump.to_text());
        let mut map = MappingAsset::new();
        let cat = map.categories[0].id;
        map.add_entry(cat, "Keyboard.Space", Some(ActionRef::new(jump.id.clone(), "input/Jump.inputaction")));
        write(&dir, "Map.inputmap", map.to_text());

        std::fs::create_dir_all(dir.join("renamed")).unwrap();
        std::fs::rename(&from, dir.join("renamed/Leap.inputaction")).unwrap();
        let lib = Library::scan(&dir);
        let info = lib.action(&jump.id).expect("still found by id");
        assert_eq!(info.path, "renamed/Leap.inputaction");
        let issues = map.validate(&lib);
        assert_eq!(issues.len(), 1, "only the stale hint is reported, not a missing action");
        assert!(issues[0].fix.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_copied_action_makes_its_id_ambiguous() {
        let dir = project("copy");
        let jump = ActionAsset::new();
        write(&dir, "A.inputaction", jump.to_text());
        write(&dir, "B.inputaction", jump.to_text());
        let lib = Library::scan(&dir);
        assert!(lib.action_entry(&jump.id).is_none());
        assert!(lib.is_ambiguous(&jump.id));
        assert_eq!(lib.action_conflicts().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn files_without_an_id_or_unreadable_are_reported_not_indexed() {
        let dir = project("problems");
        write(&dir, "Empty.inputaction", String::new());
        write(&dir, "NoId.inputmap", "{}".into());
        write(&dir, "Broken.inputaction", "{ nope".into());
        let lib = Library::scan(&dir);
        assert!(lib.is_empty());
        let kinds: Vec<_> = lib.problems().iter().map(|p| p.kind.clone()).collect();
        assert_eq!(kinds.iter().filter(|k| **k == ProblemKind::NoId).count(), 2);
        assert!(kinds.iter().any(|k| matches!(k, ProblemKind::Unreadable(_))));

        let id = stamp_file(&dir.join("Empty.inputaction")).unwrap();
        assert_eq!(Library::scan(&dir).action_entry(&id).unwrap().stem, "Empty");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn usage_and_name_clashes_are_answered_from_the_scan() {
        let dir = project("usage");
        let jump = ActionAsset::new();
        let mut other = ActionAsset::new();
        other.identifier = "Jump".into();
        write(&dir, "Jump.inputaction", jump.to_text());
        write(&dir, "Hop.inputaction", other.to_text());
        let mut map = MappingAsset::new();
        let cat = map.categories[0].id;
        map.add_entry(cat, "Keyboard.Space", Some(ActionRef::new(jump.id.clone(), "")));
        write(&dir, "M.inputmap", map.to_text());
        let lib = Library::scan(&dir);
        assert_eq!(lib.mappings_using(&jump.id).len(), 1);
        assert_eq!(lib.usage_counts().get(&jump.id), Some(&1));
        assert_eq!(lib.name_clash("jump", &jump.id).as_deref(), Some("Hop.inputaction"));
        assert!(lib.name_clash("Unique", &jump.id).is_none());
        assert_eq!(lib.mapping_at_relative("M.inputmap").unwrap().id, map.id);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsaved_edits_replace_the_file_on_disk_in_a_checked_copy() {
        let dir = project("current");
        let mut jump = ActionAsset::new();
        let path = write(&dir, "Jump.inputaction", jump.to_text());
        let lib = Library::scan(&dir);
        jump.identifier = "Leap".into();
        let copy = lib.with_current_action(&path, &jump);
        assert_eq!(copy.action(&jump.id).unwrap().name, "Leap");
        assert_eq!(lib.action(&jump.id).unwrap().name, "Jump");
        std::fs::remove_dir_all(&dir).ok();
    }
}
