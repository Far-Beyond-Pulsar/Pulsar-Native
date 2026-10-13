//! The project's widgets, found by id.
//!
//! Widgets refer to each other by [`WidgetId`] only. The library is what turns
//! an id back into a file: it scans the project for `.widget` files, reads
//! each one's id, and answers lookups. Because the id lives inside the file,
//! renaming or moving a widget (in the editor, in the file manager, or in git)
//! never breaks a reference; the next scan just finds it at its new path.
//!
//! Two files can end up with the same id (a copied file). The library does not
//! guess which is "the" widget: it reports the id as ambiguous and every
//! reference to it as unresolved until one of the copies gets a new id.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::model::validate::WidgetSource;
use crate::model::widget::WidgetAsset;
use crate::model::{Asset, WidgetId};

/// File extension of widget assets.
pub const WIDGET_EXTENSION: &str = "widget";

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

/// One widget file in the project.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: WidgetId,
    pub path: PathBuf,
    /// The file name without its extension.
    pub name: String,
    pub widget: Arc<WidgetAsset>,
}

/// A `.widget` file the library could not index.
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

/// An index of the widgets in a project at one moment.
#[derive(Clone, Debug, Default)]
pub struct Library {
    root: PathBuf,
    by_id: HashMap<WidgetId, Vec<Entry>>,
    problems: Vec<Problem>,
}

impl Library {
    /// An empty library, for before the first scan finishes.
    pub fn empty(root: PathBuf) -> Self {
        Self { root, ..Default::default() }
    }

    /// Scan `root` for widgets. Reads every `.widget` file, so call it off the
    /// UI thread for large projects.
    pub fn scan(root: &Path) -> Self {
        let mut library = Self::empty(root.to_path_buf());
        let mut files = Vec::new();
        collect(root, &mut files);
        for path in files {
            library.add_file(&path);
        }
        library.problems.sort_by(|a, b| a.path.cmp(&b.path));
        library
    }

    fn add_file(&mut self, path: &Path) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.problems.push(Problem { path: path.to_path_buf(), kind: ProblemKind::Unreadable(error.to_string()) });
                return;
            }
        };
        // A file with no id can't be referenced yet; it is not an error.
        if WidgetAsset::peek_id(&text).is_none() {
            let kind = match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(serde_json::Value::Object(_)) => ProblemKind::NoId,
                _ if text.trim().is_empty() => ProblemKind::NoId,
                _ => ProblemKind::Unreadable("not a widget file".to_string()),
            };
            self.problems.push(Problem { path: path.to_path_buf(), kind });
            return;
        }
        match WidgetAsset::parse(&text) {
            Ok((widget, _notes)) => {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let id = widget.id.clone();
                self.by_id.entry(id.clone()).or_default().push(Entry { id, path: path.to_path_buf(), name, widget: Arc::new(widget) });
            }
            Err(error) => self.problems.push(Problem { path: path.to_path_buf(), kind: ProblemKind::Unreadable(error) }),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The entry for `id`, unless it is missing or ambiguous.
    pub fn entry(&self, id: &WidgetId) -> Option<&Entry> {
        match self.by_id.get(id).map(Vec::as_slice) {
            Some([only]) => Some(only),
            _ => None,
        }
    }

    /// Every file that claims `id` (more than one means a copied file).
    pub fn claimants(&self, id: &WidgetId) -> &[Entry] {
        self.by_id.get(id).map_or(&[], Vec::as_slice)
    }

    pub fn path_of(&self, id: &WidgetId) -> Option<&Path> {
        self.entry(id).map(|e| e.path.as_path())
    }

    /// The id of the widget at `path`, if the library has indexed it.
    pub fn id_at(&self, path: &Path) -> Option<&WidgetId> {
        self.by_id.values().flatten().find(|e| same_file(&e.path, path)).map(|e| &e.id)
    }

    /// All widgets, by name then path.
    pub fn entries(&self) -> Vec<&Entry> {
        let mut all: Vec<&Entry> = self.by_id.values().flatten().collect();
        all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
        all
    }

    /// Whether any file has this id (even if several do).
    pub fn widget_exists(&self, id: &WidgetId) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.by_id.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Ids claimed by more than one file.
    pub fn conflicts(&self) -> Vec<(&WidgetId, &[Entry])> {
        let mut out: Vec<_> = self.by_id.iter().filter(|(_, v)| v.len() > 1).map(|(k, v)| (k, v.as_slice())).collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }

    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The widgets that contain an instance of `id`.
    pub fn used_by(&self, id: &WidgetId) -> Vec<&Entry> {
        self.entries().into_iter().filter(|e| e.widget.dependencies().contains(id)).collect()
    }

    /// For each widget, how many widgets instance it.
    pub fn usage_counts(&self) -> BTreeMap<WidgetId, usize> {
        let mut counts = BTreeMap::new();
        for entry in self.by_id.values().flatten() {
            for dep in entry.widget.dependencies() {
                *counts.entry(dep).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Everything `id` needs, directly or through other widgets (not
    /// including itself), in the order first reached.
    pub fn transitive_dependencies(&self, id: &WidgetId) -> Vec<WidgetId> {
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        let mut stack = vec![id.clone()];
        while let Some(current) = stack.pop() {
            let Some(entry) = self.entry(&current) else { continue };
            for dep in entry.widget.dependencies() {
                if &dep != id && seen.insert(dep.clone()) {
                    order.push(dep.clone());
                    stack.push(dep);
                }
            }
        }
        order
    }

    /// A copy of the library in which the widget at `path` is `widget`
    /// instead of what is on disk, so checks see unsaved edits.
    pub fn with_current(&self, path: &Path, widget: &WidgetAsset) -> Library {
        let mut copy = self.clone();
        for entries in copy.by_id.values_mut() {
            entries.retain(|e| !same_file(&e.path, path));
        }
        copy.by_id.retain(|_, v| !v.is_empty());
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        copy.by_id.entry(widget.id.clone()).or_default().push(Entry {
            id: widget.id.clone(),
            path: path.to_path_buf(),
            name,
            widget: Arc::new(widget.clone()),
        });
        copy
    }
}

impl WidgetSource for Library {
    fn widget(&self, id: &WidgetId) -> Option<Arc<WidgetAsset>> {
        self.entry(id).map(|e| e.widget.clone())
    }

    fn name(&self, id: &WidgetId) -> Option<String> {
        self.entry(id).map(|e| e.name.clone())
    }

    fn is_ambiguous(&self, id: &WidgetId) -> bool {
        self.claimants(id).len() > 1
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

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
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
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(WIDGET_EXTENSION)) {
            out.push(path);
        }
    }
}

/// Write `id` into the widget file at `path` if it has none, changing nothing
/// else. Used to give older or hand-made files their stable id.
pub fn stamp_file(path: &Path) -> Result<WidgetId, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let (mut widget, _) = WidgetAsset::parse(&text)?;
    if !widget.stamp() {
        return Ok(widget.id);
    }
    std::fs::write(path, widget.to_text()).map_err(|e| e.to_string())?;
    Ok(widget.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::schema::Kind;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ui_editor_lib_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, widget: &WidgetAsset) -> PathBuf {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, widget.to_text()).unwrap();
        path
    }

    #[test]
    fn widgets_are_found_by_id_wherever_they_are() {
        let dir = project("find");
        let a = WidgetAsset::new();
        let b = WidgetAsset::new();
        write(&dir, "ui/Menu.widget", &a);
        write(&dir, "ui/deep/er/Card.WIDGET", &b);
        write(&dir, "target/Skipped.widget", &WidgetAsset::new());
        std::fs::write(dir.join("notes.txt"), "x").unwrap();

        let lib = Library::scan(&dir);
        assert_eq!(lib.len(), 2);
        assert_eq!(lib.entry(&a.id).unwrap().name, "Menu");
        assert_eq!(lib.name(&b.id).as_deref(), Some("Card"));
        assert!(lib.problems().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renaming_or_moving_a_file_keeps_references_valid() {
        let dir = project("move");
        let card = WidgetAsset::new();
        let mut menu = WidgetAsset::new();
        let inst = menu.new_instance(card.id.clone(), "Card");
        let root = menu.root.id;
        menu.add(root, None, inst).unwrap();
        let card_path = write(&dir, "ui/Card.widget", &card);
        write(&dir, "ui/Menu.widget", &menu);

        let before = Library::scan(&dir);
        assert!(menu.validate(&before).iter().all(|i| i.severity < crate::model::Severity::Error));

        // Rename and move the file; nothing inside any file changes.
        std::fs::create_dir_all(dir.join("shared")).unwrap();
        std::fs::rename(&card_path, dir.join("shared/ItemCard.widget")).unwrap();
        let after = Library::scan(&dir);
        assert_eq!(after.path_of(&card.id), Some(dir.join("shared/ItemCard.widget").as_path()));
        let issues = menu.validate(&after);
        assert!(issues.iter().all(|i| i.severity < crate::model::Severity::Error), "{issues:?}");
        // The only note is that the display name changed.
        assert!(issues.iter().any(|i| i.message.contains("ItemCard")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_copied_file_makes_its_id_ambiguous_until_one_gets_a_new_id() {
        let dir = project("copy");
        let a = WidgetAsset::new();
        let original = write(&dir, "A.widget", &a);
        let copy = dir.join("A copy.widget");
        std::fs::copy(&original, &copy).unwrap();

        let lib = Library::scan(&dir);
        assert_eq!(lib.conflicts().len(), 1);
        assert!(lib.entry(&a.id).is_none(), "neither copy is picked");
        assert!(lib.is_ambiguous(&a.id));
        assert_eq!(lib.claimants(&a.id).len(), 2);

        // A widget that refers to it reports that, not "missing".
        let mut user = WidgetAsset::new();
        let inst = user.new_instance(a.id.clone(), "A");
        let root = user.root.id;
        user.add(root, None, inst).unwrap();
        assert!(user.validate(&lib).iter().any(|i| i.message.contains("More than one file")));

        // Giving the copy a new id resolves it.
        let (mut fixed, _) = WidgetAsset::parse(&std::fs::read_to_string(&copy).unwrap()).unwrap();
        fixed.regenerate_id();
        std::fs::write(&copy, fixed.to_text()).unwrap();
        let lib = Library::scan(&dir);
        assert!(lib.conflicts().is_empty());
        assert_eq!(lib.path_of(&a.id), Some(original.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn files_without_an_id_are_listed_and_can_be_stamped() {
        let dir = project("stamp");
        let path = dir.join("Old.widget");
        std::fs::write(&path, r#"{ "root": { "id": 1, "name": "Root", "kind": "canvas" } }"#).unwrap();
        std::fs::write(dir.join("Empty.widget"), "").unwrap();
        std::fs::write(dir.join("Junk.widget"), "{ nope").unwrap();

        let lib = Library::scan(&dir);
        assert!(lib.is_empty());
        let kinds: Vec<_> = lib.problems().iter().map(|p| (p.path.file_name().unwrap().to_string_lossy().into_owned(), p.kind.clone())).collect();
        assert!(kinds.contains(&("Old.widget".into(), ProblemKind::NoId)));
        assert!(kinds.contains(&("Empty.widget".into(), ProblemKind::NoId)));
        assert!(kinds.iter().any(|(n, k)| n == "Junk.widget" && matches!(k, ProblemKind::Unreadable(_))));

        let id = stamp_file(&path).unwrap();
        assert_eq!(stamp_file(&path).unwrap(), id, "stamping twice keeps the first id");
        assert_eq!(Library::scan(&dir).entry(&id).unwrap().name, "Old");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn usage_and_dependencies_follow_instances() {
        let dir = project("usage");
        let leaf = WidgetAsset::new();
        let mut mid = WidgetAsset::new();
        let mut top = WidgetAsset::new();
        let i = mid.new_instance(leaf.id.clone(), "Leaf");
        let r = mid.root.id;
        mid.add(r, None, i).unwrap();
        let i = top.new_instance(mid.id.clone(), "Mid");
        let r = top.root.id;
        top.add(r, None, i).unwrap();
        for (name, w) in [("Leaf", &leaf), ("Mid", &mid), ("Top", &top)] {
            write(&dir, &format!("{name}.widget"), w);
        }
        let lib = Library::scan(&dir);
        assert_eq!(lib.used_by(&leaf.id).len(), 1);
        assert_eq!(lib.used_by(&leaf.id)[0].name, "Mid");
        assert_eq!(lib.usage_counts().get(&mid.id), Some(&1));
        assert_eq!(lib.transitive_dependencies(&top.id), vec![mid.id.clone(), leaf.id.clone()]);
        assert!(lib.transitive_dependencies(&leaf.id).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsaved_edits_override_the_file_when_checking() {
        let dir = project("current");
        let a = WidgetAsset::new();
        let path = write(&dir, "A.widget", &a);
        let lib = Library::scan(&dir);
        let mut edited = a.clone();
        let node = edited.new_node(Kind::Text);
        let root = edited.root.id;
        edited.add(root, None, node).unwrap();
        let current = lib.with_current(&path, &edited);
        assert_eq!(current.entry(&a.id).unwrap().widget.root.children.len(), 1);
        assert_eq!(lib.entry(&a.id).unwrap().widget.root.children.len(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_project_root_is_found_from_a_file_inside_it() {
        let dir = project("root");
        std::fs::write(dir.join("Pulsar.toml"), "").unwrap();
        let file = dir.join("ui/menus/Main.widget");
        assert_eq!(project_root(&file), dir);
        let loose = project("loose");
        assert_eq!(project_root(&loose.join("A.widget")), loose);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&loose).ok();
    }
}
