//! Class registry: GUID → class directory, from `<project>/src/classes/*`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::component::ClassInstance;
use crate::id::{ensure_class_id, ClassId, CLASS_META_FILE};
use crate::prefab::{PrefabAsset, PREFAB_FILE};

/// One class found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassEntry {
    pub id: ClassId,
    /// Directory name, which is also the compiled module's class name.
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
            std::fs::read_to_string(self.dir.join("events").join(".build").join("module.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
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
    /// Scan `<project>/src/classes/*`, assigning a GUID to every class that
    /// has none yet.
    pub fn scan(project_root: &Path) -> Self {
        Self::scan_classes_dir(&classes_dir(project_root))
    }

    /// Scan one classes directory.
    pub fn scan_classes_dir(dir: &Path) -> Self {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| is_class_dir(p))
                    .collect()
            })
            .unwrap_or_default();
        dirs.sort();
        let mut registry = Self::default();
        for dir in dirs {
            let name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
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

    pub fn by_name(&self, name: &str) -> Option<&ClassEntry> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
