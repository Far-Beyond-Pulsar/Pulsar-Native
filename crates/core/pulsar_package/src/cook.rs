//! Cooking: turning project files into shipped content.
//!
//! What "cook" does in this version:
//!
//! - **Levels** (`*.level`, plus the startup level): legacy script bindings
//!   are migrated to `ClassInstance`s and every placed class is resolved to
//!   its GUID; editor-only data is dropped (the `editor` section with the
//!   editor camera, per-object `locked`, `children`, `scene_path`); asset
//!   references are rewritten to content-relative ids (`assets/...`).
//! - **Prefabs** (`prefab.json`): asset references rewritten the same way.
//! - **Assets** referenced by cooked levels and prefabs are collected with
//!   their bytes **as authored** (`.mesh`, `.fbx`, textures, ...), keyed by
//!   their content-relative id in the asset registry.
//!
//! Not yet (follow-up): converting source assets into GPU-ready runtime
//! formats (e.g. importing `.fbx` to `.mesh`, texture compression). The
//! runtime loads the same formats the editor does.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use pulsar_class::{ClassId, ClassRegistry};
use pulsar_content::{AssetKind, AssetRecord};
use serde_json::Value;

/// Keys that hold asset references even when the value's extension is not
/// a known asset type.
const ASSET_KEYS: &[&str] = &["mesh_asset", "texture", "texture_path", "material", "material_path", "asset", "asset_path"];

/// Top-level level sections only the editor uses.
const EDITOR_LEVEL_KEYS: &[&str] = &["editor"];
/// Object fields only the editor uses.
const EDITOR_OBJECT_KEYS: &[&str] = &["locked", "children", "scene_path"];

/// Assets found while cooking: content id -> where its bytes come from.
#[derive(Debug, Default)]
pub struct AssetSet {
    pub assets: BTreeMap<String, PathBuf>,
    /// References that resolved to no file, as `(where, value)`.
    pub unresolved: Vec<(String, String)>,
}

/// Resolves asset references against the project and the engine's
/// built-in assets.
pub struct AssetResolver {
    project: PathBuf,
    engine_assets: Option<PathBuf>,
}

impl AssetResolver {
    pub fn new(project: &Path, engine_assets: Option<PathBuf>) -> Self {
        let project = project.canonicalize().unwrap_or_else(|_| project.to_path_buf());
        let engine_assets = engine_assets.map(|p| p.canonicalize().unwrap_or(p));
        Self { project, engine_assets }
    }

    /// The source file `value` refers to, and its content id. Lookups
    /// mirror the runtime's: absolute, project-relative, under the
    /// project's `assets/`, then the engine's built-in assets.
    pub fn resolve(&self, value: &str) -> Option<(String, PathBuf)> {
        let norm = value.trim().replace('\\', "/");
        if norm.is_empty() {
            return None;
        }
        let path = Path::new(&norm);
        let candidates: Vec<PathBuf> = if path.is_absolute() || looks_windows_absolute(&norm) {
            vec![path.to_path_buf()]
        } else {
            let mut c = vec![self.project.join(&norm), self.project.join("assets").join(&norm)];
            if let Some(engine) = &self.engine_assets {
                c.push(engine.join(&norm));
            }
            c
        };
        let file = candidates.into_iter().find(|p| p.is_file())?;
        let file = file.canonicalize().unwrap_or(file);
        Some((self.content_id(&file), file))
    }

    /// `assets/<path>` for a file in the project (its own `assets/` folder
    /// flattened) or the engine's assets; `assets/external/<hash>/<name>`
    /// for anything else.
    fn content_id(&self, file: &Path) -> String {
        let rel = |base: &Path| file.strip_prefix(base).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
        if let Some(rel) = rel(&self.project) {
            let rel = rel.strip_prefix("assets/").unwrap_or(&rel).to_owned();
            return format!("assets/{rel}");
        }
        if let Some(rel) = self.engine_assets.as_deref().and_then(rel) {
            return format!("assets/{rel}");
        }
        let hash = pulsar_content::pak::content_hash(file.to_string_lossy().as_bytes());
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("assets/external/{:08x}/{name}", hash as u32)
    }
}

fn looks_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'/' || bytes[2] == b'\\')
}

/// Whether a string looks like an absolute filesystem path.
pub fn is_absolute_path(value: &str) -> bool {
    value.starts_with('/') || value.starts_with("\\\\") || looks_windows_absolute(value)
}

/// Rewrite every asset reference inside `value` (recursively) to its
/// content id, recording the assets. `context` names where it is, for
/// diagnostics.
pub fn rewrite_assets(value: &mut Value, resolver: &AssetResolver, assets: &mut AssetSet, context: &str) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if let Value::String(text) = child {
                    let keyed = ASSET_KEYS.contains(&key.as_str()) || key.ends_with("_asset");
                    if (keyed || AssetKind::is_asset_path(text)) && !text.trim().is_empty() {
                        rewrite_one(text, resolver, assets, &format!("{context}.{key}"));
                    }
                } else {
                    rewrite_assets(child, resolver, assets, &format!("{context}.{key}"));
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                match item {
                    Value::String(text) if AssetKind::is_asset_path(text) => {
                        rewrite_one(text, resolver, assets, &format!("{context}[{index}]"));
                    }
                    other => rewrite_assets(other, resolver, assets, &format!("{context}[{index}]")),
                }
            }
        }
        _ => {}
    }
}

fn rewrite_one(text: &mut String, resolver: &AssetResolver, assets: &mut AssetSet, context: &str) {
    match resolver.resolve(text) {
        Some((id, file)) => {
            assets.assets.insert(id.clone(), file);
            *text = id;
        }
        None => {
            assets.unresolved.push((context.to_owned(), text.clone()));
            if is_absolute_path(text) {
                // Never ship a machine path: keep only the file name.
                let name = text.rsplit(['/', '\\']).next().unwrap_or_default().to_owned();
                *text = name;
            }
        }
    }
}

/// Rewrite every remaining string that is an absolute path inside the
/// project (a prefab's `blueprint_class.class_path`, a legacy
/// `script_asset`) to its project-relative, content-relative form.
pub fn relativize_project_paths(value: &mut Value, project: &Path) {
    let project = project.canonicalize().unwrap_or_else(|_| project.to_path_buf());
    fn walk(value: &mut Value, project: &Path) {
        match value {
            Value::String(text) if is_absolute_path(text) => {
                let path = Path::new(text.as_str());
                let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                if let Ok(rel) = path.strip_prefix(project) {
                    *text = rel.to_string_lossy().replace('\\', "/");
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| walk(v, project)),
            Value::Object(map) => map.values_mut().for_each(|v| walk(v, project)),
            _ => {}
        }
    }
    walk(value, &project);
}

/// What cooking one level found worth reporting.
#[derive(Debug, Default)]
pub struct LevelReport {
    pub migrated: usize,
    /// Placed classes no project class matches.
    pub unresolved_classes: Vec<String>,
}

/// Cook a level file's JSON. See the module doc.
pub fn cook_level(
    mut level: Value,
    registry: &ClassRegistry,
    resolver: &AssetResolver,
    assets: &mut AssetSet,
    name: &str,
) -> (Value, LevelReport) {
    let mut report = LevelReport::default();
    let migration = pulsar_class::migrate::migrate_level_value(&mut level, registry);
    report.migrated = migration.script_components.len() + migration.bindings.len();

    if let Value::Object(root) = &mut level {
        for key in EDITOR_LEVEL_KEYS {
            root.remove(*key);
        }
        if let Some(Value::Array(objects)) = root.get_mut("objects") {
            for object in objects.iter_mut() {
                let Value::Object(object) = object else { continue };
                for key in EDITOR_OBJECT_KEYS {
                    object.remove(*key);
                }
                let id = object.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
                if let Some(instances) = object.get_mut("component_instances") {
                    resolve_class_instances(instances, registry, &id, &mut report);
                }
                if let Some(props) = object.get_mut("props") {
                    if let Some(instances) = props.get_mut("__component_instances") {
                        resolve_class_instances(instances, registry, &id, &mut report);
                    }
                }
            }
        }
        if let Some(Value::Object(components)) = root.get_mut("components") {
            for (id, records) in components.iter_mut() {
                resolve_class_instances(records, registry, id, &mut report);
            }
        }
    }
    rewrite_assets(&mut level, resolver, assets, name);
    relativize_project_paths(&mut level, &resolver.project);
    (level, report)
}

/// Fill in the class GUID of every `ClassInstance` record, by GUID or
/// class name.
fn resolve_class_instances(records: &mut Value, registry: &ClassRegistry, object: &str, report: &mut LevelReport) {
    let Value::Array(records) = records else { return };
    for record in records {
        if record.get("class_name").and_then(Value::as_str) != Some(pulsar_class::CLASS_INSTANCE) {
            continue;
        }
        let Some(data) = record.get_mut("data").and_then(Value::as_object_mut) else { continue };
        let guid = data.get("class").and_then(Value::as_str).unwrap_or_default().to_owned();
        let name = data.get("class_name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let entry = (!guid.is_empty())
            .then(|| registry.by_id(&ClassId::from(guid.as_str())))
            .flatten()
            .or_else(|| (!name.is_empty()).then(|| registry.by_name(&name)).flatten());
        match entry {
            Some(entry) => {
                data.insert("class".into(), Value::String(entry.id.as_str().to_owned()));
                data.insert("class_name".into(), Value::String(entry.name.clone()));
            }
            None => report.unresolved_classes.push(format!("{object}: {}", if guid.is_empty() { &name } else { &guid })),
        }
    }
}

/// `Pulsar/scripting.json` with class names resolved to GUIDs.
pub fn cook_scripting_config(project: &Path, registry: &ClassRegistry) -> Result<Option<Value>, String> {
    let path = pulsar_game::scripting::scripting_config_path(project);
    let Ok(bytes) = std::fs::read(&path) else { return Ok(None) };
    let mut config: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(Value::Array(scripts)) = config.get_mut("global_scripts") {
        for script in scripts.iter_mut() {
            let Some(reference) = script.as_str() else { continue };
            let entry = registry.by_id(&ClassId::from(reference)).or_else(|| registry.by_name(reference));
            match entry {
                Some(entry) => *script = Value::String(entry.id.as_str().to_owned()),
                None => return Err(format!("{}: global script `{reference}` is not a class of this project", path.display())),
            }
        }
    }
    Ok(Some(config))
}

/// The asset registry record of a collected asset.
pub fn asset_record(id: &str, source: &Path, project: &Path, bytes: &[u8]) -> AssetRecord {
    let source = source
        .strip_prefix(project.canonicalize().unwrap_or_else(|_| project.to_path_buf()))
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| format!("<engine or external>/{}", source.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    AssetRecord {
        id: id.to_owned(),
        kind: AssetKind::from_path(id),
        source,
        size: bytes.len() as u64,
        hash: pulsar_content::registry::hash_hex(pulsar_content::pak::content_hash(bytes)),
        pak: None,
    }
}

/// Every `*.level` file of the project, content-relative, skipping build
/// output and version-control directories.
pub fn find_levels(project: &Path, skip: &[PathBuf]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![project.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                let skipped = matches!(name.as_str(), "target" | ".git" | "node_modules" | ".pulsar" | "scripts")
                    || skip.iter().any(|s| path.starts_with(s));
                if !skipped {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "level") {
                if let Ok(rel) = path.strip_prefix(project) {
                    out.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_lose_editor_data_and_absolute_paths() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("assets/meshes")).unwrap();
        std::fs::write(root.join("assets/meshes/a.mesh"), b"A").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("b.png"), b"B").unwrap();
        std::fs::create_dir_all(root.join("src/classes/Door")).unwrap();
        std::fs::write(root.join("src/classes/Door/class.json"), r#"{"class_id":"door-guid"}"#).unwrap();
        let registry = ClassRegistry::scan(root);
        let resolver = AssetResolver::new(root, None);
        let mut assets = AssetSet::default();

        let abs_png = outside.path().join("b.png").display().to_string();
        let level = json!({
            "version": "2.1",
            "editor": { "camera": { "position": [0, 1, 2] } },
            "objects": [{
                "id": "o1", "name": "Door", "object_type": "Empty", "locked": true,
                "children": [], "scene_path": "Door",
                "props": { "mesh_asset": "meshes/a.mesh" },
                "component_instances": [
                    { "index": 0, "class_name": "ClassInstance", "data": { "class": "", "class_name": "Door" } },
                    { "index": 1, "class_name": "StaticMeshComponent", "data": { "mesh_asset": root.join("assets/meshes/a.mesh").display().to_string() } },
                    { "index": 2, "class_name": "Decal", "data": { "texture": abs_png } },
                    { "index": 3, "class_name": "Other", "data": { "mesh_asset": "/nowhere/gone.mesh" } }
                ]
            }]
        });
        let (cooked, report) = cook_level(level, &registry, &resolver, &mut assets, "main.level");
        assert!(cooked.get("editor").is_none());
        let object = &cooked["objects"][0];
        assert!(object.get("locked").is_none() && object.get("children").is_none() && object.get("scene_path").is_none());
        assert_eq!(object["props"]["mesh_asset"], "assets/meshes/a.mesh");
        let components = &object["component_instances"];
        assert_eq!(components[0]["data"]["class"], "door-guid");
        assert_eq!(components[1]["data"]["mesh_asset"], "assets/meshes/a.mesh");
        let texture = components[2]["data"]["texture"].as_str().unwrap();
        assert!(texture.starts_with("assets/external/") && texture.ends_with("/b.png"), "{texture}");
        assert_eq!(components[3]["data"]["mesh_asset"], "gone.mesh", "machine paths never ship");
        assert!(report.unresolved_classes.is_empty());
        assert_eq!(assets.assets.len(), 2);
        assert_eq!(assets.unresolved.len(), 1);
        assert!(!cooked.to_string().contains(&root.display().to_string()));
    }
}
