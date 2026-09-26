//! Class registry: GUID → class directory. Classes are found in
//! `<project>/src/classes/*` and, anywhere in the project, in Blueprint
//! class folders named `<Name>.class/` (what the content browser creates).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::component::ClassInstance;
use crate::id::{ensure_class_id, ClassId, CLASS_META_FILE};
use crate::prefab::{PrefabAsset, PREFAB_FILE};

/// One class found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassEntry {
    pub id: ClassId,
    /// Class name: the directory name without a `.class` extension
    /// ([`class_name_of_dir`]), which is also the compiled module's name.
    pub name: String,
    pub dir: PathBuf,
}

impl ClassEntry {
    /// Load the class's current definition from disk.
    pub fn load_definition(&self) -> Result<ClassDefinition, String> {
        Ok(ClassDefinition {
            id: self.id.clone(),
            name: self.name.clone(),
            dir: self.dir.clone(),
            prefab: PrefabAsset::load_from_dir(&self.dir)?,
        })
    }
}

/// A class's definition as instances are built from it.
#[derive(Clone, Debug, Default)]
pub struct ClassDefinition {
    pub id: ClassId,
    pub name: String,
    pub dir: PathBuf,
    /// Components with slot ids filled in, plus variable defaults.
    pub prefab: PrefabAsset,
}

/// Value kind of a class script variable, as the instance details panel
/// edits it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VariableKind {
    Bool,
    Int,
    Float,
    String,
    /// Any other script type (entity, component, value types): shown, but
    /// not editable as a plain value.
    Other(String),
}

/// One script variable of a class, with its class default.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassVariable {
    pub name: String,
    pub kind: VariableKind,
    /// The class default in its natural JSON form (`5.0`, `true`, `"hi"`).
    pub default: Value,
}

impl VariableKind {
    fn from_script_type(ty: &Value) -> Self {
        match ty.get("kind").and_then(Value::as_str) {
            Some("bool") => Self::Bool,
            Some("int") => Self::Int,
            Some("float") => Self::Float,
            Some("str") => Self::String,
            Some(other) => Self::Other(other.to_string()),
            None => Self::Other(ty.to_string()),
        }
    }

    fn infer(value: &Value) -> Self {
        match value {
            Value::Bool(_) => Self::Bool,
            Value::Number(n) if n.is_i64() || n.is_u64() => Self::Int,
            Value::Number(_) => Self::Float,
            _ => Self::String,
        }
    }

    /// `value` (possibly a string, as blueprint defaults are stored) in this
    /// kind's JSON form; the kind's zero when it does not convert.
    pub fn coerce(&self, value: Option<&Value>) -> Value {
        let parsed = match value {
            Some(Value::String(s)) if *self != Self::String => {
                serde_json::from_str::<Value>(s).ok()
            }
            other => other.cloned(),
        };
        match self {
            Self::Bool => Value::Bool(parsed.and_then(|v| v.as_bool()).unwrap_or(false)),
            Self::Int => Value::from(
                parsed
                    .and_then(|v| v.as_f64())
                    .map(|f| f as i64)
                    .unwrap_or(0),
            ),
            Self::Float => Value::from(parsed.and_then(|v| v.as_f64()).unwrap_or(0.0)),
            Self::String => Value::String(
                parsed
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_default(),
            ),
            Self::Other(_) => parsed.unwrap_or(Value::Null),
        }
    }
}

impl ClassDefinition {
    /// The class's script variables: those its compiled module declares
    /// (`events/.build/module.json`), with defaults from the prefab's
    /// blueprint defaults. Hidden variables (`__…`, e.g. component-slot
    /// handles) are left out. Without a compiled module, the prefab's
    /// variable defaults are listed with their kind inferred.
    pub fn variables(&self) -> Vec<ClassVariable> {
        let defaults = self.prefab.variable_defaults();
        let module =
            engine_fs::virtual_fs::read_file(&self.dir.join("events").join(".build").join("module.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let mut out: Vec<ClassVariable> = Vec::new();
        if let Some(vars) = module
            .as_ref()
            .and_then(|m| m.get("variables"))
            .and_then(Value::as_array)
        {
            for var in vars {
                let Some(name) = var.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if name.starts_with("__") {
                    continue;
                }
                let kind = var
                    .get("ty")
                    .map(VariableKind::from_script_type)
                    .unwrap_or(VariableKind::Other(String::new()));
                let default = kind.coerce(defaults.get(name));
                out.push(ClassVariable {
                    name: name.to_string(),
                    kind,
                    default,
                });
            }
        }
        let mut extra: Vec<(&String, &Value)> = defaults
            .iter()
            .filter(|(name, _)| !out.iter().any(|v| &v.name == *name))
            .collect();
        extra.sort_by(|a, b| a.0.cmp(b.0));
        for (name, raw) in extra {
            let parsed = match raw {
                Value::String(s) => {
                    serde_json::from_str::<Value>(s).unwrap_or_else(|_| raw.clone())
                }
                other => other.clone(),
            };
            let kind = VariableKind::infer(&parsed);
            out.push(ClassVariable {
                name: name.clone(),
                default: kind.coerce(Some(raw)),
                kind,
            });
        }
        out
    }
}

/// Every class of one project, indexed by GUID.
#[derive(Clone, Debug, Default)]
pub struct ClassRegistry {
    entries: Vec<ClassEntry>,
}

/// `<project>/src/classes`.
pub fn classes_dir(project_root: &Path) -> PathBuf {
    project_root.join("src").join("classes")
}

/// The extension of Blueprint class folders (`<Name>.class/`).
pub const CLASS_DIR_EXTENSION: &str = "class";

/// `name` without a trailing `.class` extension.
pub fn strip_class_ext(name: &str) -> &str {
    name.strip_suffix(".class").filter(|n| !n.is_empty()).unwrap_or(name)
}

/// The class name of a class directory: its folder name without a
/// `.class` extension (`Door.class/` and `src/classes/Door/` are both
/// `Door`).
pub fn class_name_of_dir(dir: &Path) -> String {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    strip_class_ext(name).to_string()
}

/// Directories never searched for classes.
fn skip_when_searching(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules" | "Content" | "build" | "dist")
}

/// Every class directory of the project at `project_root`: the class
/// directories in `<project>/src/classes`, plus every `<Name>.class/`
/// folder anywhere under the project (hidden directories, `target` and
/// build outputs are skipped; a class folder's own contents are not
/// searched). Sorted, without duplicates.
pub fn find_class_dirs(project_root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(classes_dir(project_root))
        .map(|entries| entries.flatten().map(|e| e.path()).filter(|p| is_class_dir(p)).collect())
        .unwrap_or_default();
    let mut stack = vec![(project_root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if skip_when_searching(name) {
                continue;
            }
            let is_class_folder = path.extension().and_then(|e| e.to_str()) == Some(CLASS_DIR_EXTENSION);
            if is_class_folder && is_class_dir(&path) {
                found.push(path);
            } else if !is_class_folder && depth < 16 && !is_class_dir(&path) {
                stack.push((path, depth + 1));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The project a class directory belongs to: `<project>` for
/// `<project>/src/classes/<Name>`, otherwise the nearest ancestor holding
/// a `Pulsar/` directory or a `Cargo.toml`.
pub fn project_root_of_class_dir(class_dir: &Path) -> Option<PathBuf> {
    let parent = class_dir.parent()?;
    if parent.file_name().and_then(|n| n.to_str()) == Some("classes") {
        if let Some(src) = parent.parent().filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("src")) {
            if let Some(root) = src.parent() {
                return Some(root.to_path_buf());
            }
        }
    }
    parent
        .ancestors()
        .find(|dir| dir.join("Pulsar").is_dir() || dir.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
}

/// Whether `dir` looks like a class directory.
pub fn is_class_dir(dir: &Path) -> bool {
    let module = dir.join("events").join(".build").join("module.json");
    dir.is_dir()
        && ([CLASS_META_FILE, PREFAB_FILE, "graph_save.json"]
            .iter()
            .any(|f| dir.join(f).is_file())
            || module.is_file())
}

impl ClassRegistry {
    /// The project's classes: from its class index (`Pulsar/class_index.json`,
    /// written into packaged content, where nothing is scanned or
    /// written), otherwise by scanning `<project>/src/classes/*` and
    /// assigning a GUID to every class that has none yet.
    pub fn scan(project_root: &Path) -> Self {
        if let Some(index) = ClassIndex::read(project_root) {
            return index.registry(project_root);
        }
        Self::from_dirs(find_class_dirs(project_root))
    }

    /// Scan one classes directory (its direct children only).
    pub fn scan_classes_dir(dir: &Path) -> Self {
        let dirs: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| is_class_dir(p))
                    .collect()
            })
            .unwrap_or_default();
        Self::from_dirs(dirs)
    }

    /// A registry of these class directories, assigning a GUID to any
    /// that has none yet.
    pub fn from_dirs(mut dirs: Vec<PathBuf>) -> Self {
        dirs.sort();
        dirs.dedup();
        let mut registry = Self::default();
        for dir in dirs {
            let name = class_name_of_dir(&dir);
            let id = ensure_class_id(&dir);
            if registry.by_id(&id).is_some() {
                // A copied class dir brought its class.json along. Keep the
                // first (sorted) one; the copy gets a fresh id.
                tracing::warn!(dir = %dir.display(), %id, "Duplicate class id; assigning a new one");
                let mut meta = crate::id::ClassMeta::read(&dir).unwrap_or_default();
                meta.class_id = ClassId::new_random();
                let id = match meta.write(&dir) {
                    Ok(()) => meta.class_id,
                    Err(_) => crate::id::fallback_class_id(&name),
                };
                registry.entries.push(ClassEntry { id, name, dir });
                continue;
            }
            registry.entries.push(ClassEntry { id, name, dir });
        }
        registry
    }

    /// Build a registry from known entries (tests, tools).
    pub fn from_entries(entries: Vec<ClassEntry>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[ClassEntry] {
        &self.entries
    }

    pub fn by_id(&self, id: &ClassId) -> Option<&ClassEntry> {
        self.entries.iter().find(|e| &e.id == id)
    }

    /// The class named `name`. A `.class` extension on `name` (a folder
    /// name, as older levels recorded it) is ignored.
    pub fn by_name(&self, name: &str) -> Option<&ClassEntry> {
        let name = strip_class_ext(name);
        self.entries.iter().find(|e| e.name == name)
    }

    /// Resolve an instance's class: by GUID, then by its `class_name` hint.
    pub fn resolve(&self, instance: &ClassInstance) -> Option<&ClassEntry> {
        if !instance.class.is_empty() {
            if let Some(entry) = self.by_id(&instance.class) {
                return Some(entry);
            }
        }
        (!instance.class_name.is_empty())
            .then(|| self.by_name(&instance.class_name))
            .flatten()
    }

    /// Resolve a legacy `ScriptComponent.script_asset` path (absolute,
    /// project-relative, or from another machine) to a class: the same
    /// directory when it exists here, otherwise a class with the same
    /// directory name.
    pub fn resolve_script_asset(&self, script_asset: &str) -> Option<&ClassEntry> {
        let trimmed = script_asset.trim().trim_end_matches(['/', '\\']);
        if trimmed.is_empty() {
            return None;
        }
        let path = Path::new(trimmed);
        if let Ok(canonical) = path.canonicalize() {
            if let Some(entry) = self
                .entries
                .iter()
                .find(|e| e.dir.canonicalize().ok().as_deref() == Some(canonical.as_path()))
            {
                return Some(entry);
            }
        }
        let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
        self.by_name(name)
    }

    /// Load the definition of the class `instance` refers to.
    pub fn definition_for(&self, instance: &ClassInstance) -> Option<ClassDefinition> {
        let entry = self.resolve(instance)?;
        match entry.load_definition() {
            Ok(def) => Some(def),
            Err(error) => {
                tracing::warn!(class = %entry.name, "Class definition unreadable: {error}");
                None
            }
        }
    }
}

/// The class index file, relative to the project (or content) root.
pub const CLASS_INDEX_FILE: &str = "Pulsar/class_index.json";

/// One class in a [`ClassIndex`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClassIndexEntry {
    pub id: ClassId,
    pub name: String,
    /// The class directory, relative to the root, `/`-separated.
    pub dir: String,
}

/// Class GUID -> class directory, resolved ahead of time (by the
/// packager) so a packaged game never scans or writes class directories.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClassIndex {
    pub format: u32,
    pub classes: Vec<ClassIndexEntry>,
}

impl ClassIndex {
    pub const FORMAT: u32 = 1;

    /// The index of `registry`'s classes, with directories relative to
    /// `root`. Classes outside `root` are left out.
    pub fn from_registry(registry: &ClassRegistry, root: &Path) -> Self {
        let classes = registry
            .entries()
            .iter()
            .filter_map(|entry| {
                let rel = entry.dir.strip_prefix(root).ok()?;
                let dir = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                Some(ClassIndexEntry { id: entry.id.clone(), name: entry.name.clone(), dir })
            })
            .collect();
        Self { format: Self::FORMAT, classes }
    }

    /// Read `<root>/Pulsar/class_index.json` (through the virtual
    /// filesystem). `None` when there is none.
    pub fn read(root: &Path) -> Option<Self> {
        let bytes = engine_fs::virtual_fs::read_file(&root.join(CLASS_INDEX_FILE)).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(index) => Some(index),
            Err(error) => {
                tracing::warn!(root = %root.display(), "Unreadable class index: {error}");
                None
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// The registry these classes form under `root`.
    pub fn registry(&self, root: &Path) -> ClassRegistry {
        ClassRegistry::from_entries(
            self.classes
                .iter()
                .map(|c| ClassEntry { id: c.id.clone(), name: c.name.clone(), dir: root.join(&c.dir) })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_index_replaces_the_scan() {
        let root = tempfile::tempdir().unwrap();
        let classes = classes_dir(root.path());
        std::fs::create_dir_all(classes.join("Lamp")).unwrap();
        std::fs::write(classes.join("Lamp").join("graph_save.json"), "{}").unwrap();
        let scanned = ClassRegistry::scan(root.path());
        let index = ClassIndex::from_registry(&scanned, root.path());
        assert_eq!(index.classes[0].dir, "src/classes/Lamp");

        // Elsewhere (a packaged game): only the index, no class dirs to scan.
        let shipped = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(shipped.path().join("Pulsar")).unwrap();
        std::fs::write(shipped.path().join(CLASS_INDEX_FILE), index.to_json()).unwrap();
        let registry = ClassRegistry::scan(shipped.path());
        let lamp = registry.by_name("Lamp").unwrap();
        assert_eq!(lamp.id, scanned.by_name("Lamp").unwrap().id);
        assert_eq!(lamp.dir, shipped.path().join("src/classes/Lamp"));
    }

    #[test]
    fn class_folders_anywhere_in_the_project_are_classes_named_without_extension() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::create_dir_all(root.join("Pulsar")).unwrap();
        for dir in ["src/classes/Door", "content/blueprints/NewBlueprintClass.class", "Lamp.class"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("graph_save.json"), "{}").unwrap();
        }
        // Build outputs and hidden directories are not searched.
        for dir in ["target/debug/Stale.class", ".git/Old.class"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("graph_save.json"), "{}").unwrap();
        }

        let registry = ClassRegistry::scan(root);
        let mut names: Vec<_> = registry.entries().iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["Door", "Lamp", "NewBlueprintClass"]);

        let class = registry.by_name("NewBlueprintClass").expect("found by name");
        assert_eq!(class.dir, root.join("content/blueprints/NewBlueprintClass.class"));
        // Levels saved before names dropped the extension still resolve.
        assert_eq!(registry.by_name("NewBlueprintClass.class").map(|e| &e.id), Some(&class.id));
        let instance = ClassInstance::new(class.id.clone(), "NewBlueprintClass.class");
        assert_eq!(registry.resolve(&instance).map(|e| &e.name), Some(&class.name));
        assert_eq!(project_root_of_class_dir(&class.dir).as_deref(), Some(root));
    }

    #[test]
    fn scan_assigns_ids_and_resolves_paths() {
        let root = tempfile::tempdir().unwrap();
        let classes = classes_dir(root.path());
        std::fs::create_dir_all(classes.join("Lamp")).unwrap();
        std::fs::write(classes.join("Lamp").join("graph_save.json"), "{}").unwrap();
        std::fs::create_dir_all(classes.join("NotAClass")).unwrap();

        let registry = ClassRegistry::scan(root.path());
        assert_eq!(registry.entries().len(), 1);
        let lamp = registry.by_name("Lamp").unwrap().clone();
        assert!(!lamp.id.is_empty());

        // Stable across rescans.
        assert_eq!(
            ClassRegistry::scan(root.path()).by_name("Lamp").unwrap().id,
            lamp.id
        );

        // Legacy absolute path, this machine and another machine.
        let here = classes.join("Lamp").display().to_string();
        assert_eq!(registry.resolve_script_asset(&here).unwrap().id, lamp.id);
        assert_eq!(
            registry
                .resolve_script_asset("D:\\Other\\Project\\src\\classes\\Lamp\\")
                .unwrap()
                .id,
            lamp.id
        );
        assert!(registry.resolve_script_asset("/x/Missing").is_none());
    }
}

#[cfg(test)]
mod variable_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn variables_come_from_the_module_with_prefab_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("events/.build")).unwrap();
        std::fs::write(
            dir.path().join("events/.build/module.json"),
            json!({ "name": "C", "variables": [
                { "name": "speed", "ty": { "kind": "float" } },
                { "name": "alive", "ty": { "kind": "bool" } },
                { "name": "__slot:x", "ty": { "kind": "component", "name": "A" } }
            ] })
            .to_string(),
        )
        .unwrap();
        let mut def = ClassDefinition {
            dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        def.prefab.blueprint_class = Some(crate::prefab::BlueprintClassRef {
            class_path: String::new(),
            variable_defaults: [
                ("speed".to_string(), json!("5.0")),
                ("label".to_string(), json!("hi")),
            ]
            .into(),
        });
        let vars = def.variables();
        let names: Vec<_> = vars.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            ["speed", "alive", "label"],
            "hidden slot handles are not variables"
        );
        assert_eq!(vars[0].kind, VariableKind::Float);
        assert_eq!(vars[0].default, json!(5.0));
        assert_eq!(vars[1].default, json!(false));
        assert_eq!(vars[2].kind, VariableKind::String);
        assert_eq!(vars[2].default, json!("hi"));
    }
}
