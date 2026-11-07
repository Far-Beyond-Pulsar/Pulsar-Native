//! Play-mode level bootstrap: hydrate a `.level` file into
//! the scene `World` (Pulsar-Native#637).
//!
//! This is the runtime counterpart of the editor's own load path
//! (`scene_edit::level_io::load_from_file`): one authoritative copy of the scene,
//! owned by SceneDB, that renderers and gameplay share -- NOT a direct
//! imperative load into a Helio `Scene` (that was `pulsar_scene::
//! SceneLoader`, now legacy/import-only).
//!
//! Hydration mirrors the editor exactly:
//!
//! - Objects/transforms/hierarchy/visibility are spawned straight into the
//!   world with [`SceneWorldExt::spawn_object`].
//! - Every component record becomes a component-instance entity attached
//!   to its object (Pulsar-Native#1035, D1) through `pulsar_world_registry::
//!   attach_records`, enabled or not: a `#[register_world_component]` class
//!   holds its typed value (`StaticMeshComponent`'s decode loads its mesh
//!   asset here -- it resolves paths via `engine_state::get_project_path()`,
//!   so callers must have set that before calling [`RuntimeLevel::load`]).
//! - A class this build does not register is attached as an explicit
//!   `UnresolvedComponent` that keeps its JSON for lossless saving; a
//!   registered class whose data does not decode fails the load.
//!
//! Component data source precedence matches the editor's ("persisted
//! components are authoritative when present"): a top-level
//! `components` entry for an object wins over that object's own
//! `component_instances`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use parking_lot::RwLock;
use pulsar_scene::component_instances_from_props;
use pulsar_scene::format::{ObjectType as FileObjectType, SceneFile};
use serde_json::Value;

use pulsar_scenedb::{Entity, World};

use crate::scene::hierarchy_transform;
use crate::scene::{
    ObjectType, RenderProps, SceneWorldExt, SharedScene, SpawnObject, Transform, Visibility,
};

/// Errors from [`RuntimeLevel::load`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeLevelError {
    #[error("failed to read '{path}': {message}")]
    Io { path: String, message: String },
    #[error("failed to parse '{path}': {message}")]
    Parse { path: String, message: String },
    #[error("unsupported scene version '{0}' (expected 1.x or 2.x)")]
    UnsupportedVersion(String),
    #[error("failed to hydrate component {class_name} on object {object_id}: {message}")]
    ComponentHydration {
        object_id: String,
        class_name: String,
        message: String,
    },
}

/// Editor camera state persisted in the level file (`editor.camera`) --
/// position (f64: a planet's surface is millions of metres out) + world
/// yaw/pitch in radians, the convention `FlyCamera::new` takes. Lets a
/// play-mode camera start where the editor view was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorCamera {
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// What a level load reports beyond hydration itself: the editor camera
/// seed. Which scripts run is not an extra: placed classes are
/// `ClassInstance` components in the world, which the script driver
/// (`pulsar_game::scripting::ScriptDriver`) follows (#922).
#[derive(Clone, Debug, Default)]
pub struct LevelExtras {
    /// The camera saved under `editor.camera`, if any.
    pub editor_camera: Option<EditorCamera>,
}

/// A scene loaded for runtime use: one shared SceneDB scene plus
/// the level-file extras gameplay cares about (editor camera seed).
pub struct RuntimeLevel {
    scene: SharedScene,
    extras: LevelExtras,
}

impl RuntimeLevel {
    /// Load and hydrate a level file. See the module doc for the hydration
    /// contract; call `engine_state::set_project_path` first so asset-
    /// resolving hydrates (`StaticMeshComponent`) can find project files.
    pub fn load(path: &Path) -> Result<Self, RuntimeLevelError> {
        Self::load_with_classes(path, &project_class_registry())
    }

    /// [`load`](Self::load) with an explicit class registry instead of the
    /// current project's (tools, tests).
    pub fn load_with_classes(
        path: &Path,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<Self, RuntimeLevelError> {
        let file = load_scene_file(path, registry)?;
        Self::from_scene_file_with_classes(file, registry)
    }
    /// Load a level file and hydrate it into an EXISTING world -- the
    /// one-world play-mode path (Pulsar-Native#637/#634): the tick loop's
    /// shared store is authoritative, so the level merges INTO it
    /// (additively; setup-time-registered actors survive) instead of the
    /// level constructing its own store. Duplicate stable ids between the
    /// file and live state are errors, never silent re-spawns.
    ///
    /// Returns the file's extras ([`LevelExtras`]: the editor camera). Call
    /// `engine_state::set_project_path` first so asset-resolving hydrates
    /// (`StaticMeshComponent`) can find project files and placed classes
    /// resolve against the project's classes.
    pub fn load_into(path: &Path, world: &mut World) -> Result<LevelExtras, RuntimeLevelError> {
        Self::load_into_with_classes(path, world, &project_class_registry())
    }

    /// [`load_into`](Self::load_into) with an explicit class registry
    /// (tools, tests).
    pub fn load_into_with_classes(
        path: &Path,
        world: &mut World,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<LevelExtras, RuntimeLevelError> {
        let file = load_scene_file(path, registry)?;
        let editor_camera = editor_camera(&file.editor);
        Self::hydrate_scene_file(file, world, registry)?;
        crate::scene::sublevel_display::sync_level_instances(world);
        crate::scene::resolve_level_assets(world);
        Ok(LevelExtras { editor_camera })
    }
    /// Instantiate a level file's objects **under** `parent`: the file's origin
    /// is `parent`'s origin, its top-level objects become `parent`'s children
    /// (their transforms are the file's own, relative to its origin), and every
    /// object id gets `id_prefix` so a level can be placed more than once. This
    /// is how a level instance is shown (`SubLevelDisplayComponent`).
    ///
    /// Returns every object this created, class-generated children included.
    /// Call `engine_state::set_project_path` first, as for [`load_into`].
    /// Assets resolve through the host's normal asset pump. This path mutates
    /// a live shared world, so it must never wait for asset workers.
    pub fn load_under(
        path: &Path,
        world: &mut World,
        parent: Entity,
        id_prefix: &str,
    ) -> Result<Vec<Entity>, RuntimeLevelError> {
        let registry = project_class_registry();
        let mut file = load_scene_file(path, &registry)?;
        let prefixed = |id: &str| format!("{id_prefix}{id}");
        let top_level: Vec<String> = file
            .objects
            .iter()
            .filter(|obj| obj.parent.is_none())
            .map(|obj| prefixed(&obj.id))
            .collect();
        for obj in &mut file.objects {
            obj.id = prefixed(&obj.id);
            obj.parent = obj.parent.as_deref().map(prefixed);
        }
        if let Some(map) = file.components.as_object_mut() {
            *map = std::mem::take(map)
                .into_iter()
                .map(|(id, value)| (prefixed(&id), value))
                .collect();
        }
        file.blueprint_bindings = std::mem::take(&mut file.blueprint_bindings)
            .into_iter()
            .map(|(id, bindings)| (prefixed(&id), bindings))
            .collect();

        let mut before: HashSet<Entity> =
            crate::scene::hierarchy_transform::descendants_of(world, parent)
                .into_iter()
                .collect();
        before.insert(parent);
        Self::hydrate_scene_file(file, world, &registry)?;
        for id in top_level {
            if let Some(entity) = world.entity_for(&id) {
                // The file's transform is already relative to its origin.
                let _ = world.reparent(entity, Some(parent));
            }
        }
        Ok(
            crate::scene::hierarchy_transform::descendants_of(world, parent)
                .into_iter()
                .filter(|entity| !before.contains(entity))
                .collect(),
        )
    }

    /// Hydrate from an already-parsed [`SceneFile`] into a fresh store
    /// (import/legacy callers that get their JSON from somewhere other than
    /// disk).
    pub fn from_scene_file(file: SceneFile) -> Result<Self, RuntimeLevelError> {
        Self::from_scene_file_with_classes(file, &project_class_registry())
    }

    /// [`from_scene_file`](Self::from_scene_file) with an explicit class
    /// registry.
    pub fn from_scene_file_with_classes(
        file: SceneFile,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<Self, RuntimeLevelError> {
        let editor_camera = editor_camera(&file.editor);
        let mut scene = crate::scene::new_scene();
        Self::hydrate_scene_file(file, &mut scene.world, registry)?;
        crate::scene::sublevel_display::sync_level_instances(&mut scene.world);
        crate::scene::resolve_level_assets(&mut scene.world);
        let extras = LevelExtras { editor_camera };
        Ok(Self {
            scene: Arc::new(RwLock::new(scene)),
            extras,
        })
    }

    /// Shared hydration core: version gate + class migration + objects +
    /// components + class instances into `world`.
    fn hydrate_scene_file(
        mut file: SceneFile,
        world: &mut World,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<(), RuntimeLevelError> {
        migrate_scene_file(&mut file, registry);
        let version = version_string(&file.version);
        // Same accepted set as the editor's own loader: 1.x and 2.x.
        if !version.starts_with("1.") && !version.starts_with("2.") && version != "1" {
            return Err(RuntimeLevelError::UnsupportedVersion(version));
        }

        // Parent-before-child order is the format's own DFS guarantee (see
        // `SceneFile::objects` doc), which is what spawning requires.
        // Validate before mutating World so malformed hierarchy/identity data
        // cannot leave a partially hydrated SceneDB.
        validate_objects(world, &file.objects)?;
        let mut spawned = Vec::with_capacity(file.objects.len());
        for obj in &file.objects {
            spawned.push(spawn_file_object(world, obj)?);
        }
        // A level saved before parent-relative transforms holds world-space
        // values for parented objects: make them local, in place.
        if hierarchy_transform::file_has_world_space_transforms(&version) {
            hierarchy_transform::convert_legacy_world_transforms(world, &spawned);
        }
        // Rigid frames (an object prop) become frame components.
        crate::scene::frames::sync_frames_from_props(world, &spawned);
        for &entity in &spawned {
            crate::scene::partitions::apply_space_prop(world, entity);
        }

        let persisted = persisted_components(&file.components);
        for obj in &file.objects {
            let Some(entity) = world.entity_for(&obj.id) else {
                continue;
            };
            let instances = match persisted.get(&obj.id) {
                // A persisted entry is authoritative, including an explicit empty
                // array, which means the object has no components.
                Some(records) => records.clone(),
                None => {
                    component_instances_from_props(&obj.props, obj.component_instances.as_ref())
                        .into_iter()
                        .map(|(index, class_name, data)| ComponentRecord {
                            index,
                            class_name,
                            data,
                            enabled: true,
                        })
                        .collect::<Vec<_>>()
                }
            };
            // Legacy shapes were migrated with the file
            // (`pulsar_class::records`: flat data nested, and the plugins'
            // migrations, such as the meshes').
            hydrate_components(world, entity, &obj.id, &instances)?;
        }

        // Placed classes (#921): rebuild each instance from the current class
        // definition, then apply its overrides. Unresolved classes keep their
        // ClassInstance and are only warned about.
        let roots: Vec<Entity> = file
            .objects
            .iter()
            .filter_map(|obj| world.entity_for(&obj.id))
            .collect();
        let report = pulsar_class::world::expand_roots(world, registry, &roots);
        for id in &report.unresolved {
            tracing::warn!(object = %id, "Placed class instance has no class in this project");
        }
        // Plugin singleton rules are repaired on load; directional lights
        // are unrestricted.
        crate::scene::level_rules::enforce_on_load(world);
        // Components are hydrated now: everything that defines a space (a sublevel
        // placement is a component) gets its slot.
        crate::scene::frames::sync_spaces(world);
        // What the migration could not turn into a ClassInstance (a second
        // class bound to one object) has no script instance any more.
        for (stable_id, bindings) in &file.blueprint_bindings {
            for binding in bindings {
                tracing::warn!(
                    object = %stable_id,
                    class = %binding.class_name,
                    "Legacy script binding not migrated (one class per object); it does not run"
                );
            }
        }
        Ok(())
    }

    /// The shared, authoritative scene. Renderers and the tick loop all clone
    /// this handle.
    pub fn scene(&self) -> SharedScene {
        Arc::clone(&self.scene)
    }

    /// The level's extras: the editor camera seed.
    pub fn extras(&self) -> &LevelExtras {
        &self.extras
    }

    /// Editor camera saved with the level, if any.
    pub fn editor_camera(&self) -> Option<EditorCamera> {
        self.extras.editor_camera
    }
}

/// One component instance record -- either parsed from the file's persisted
/// components array (which carries its own enabled flag) or converted from
/// `component_instances_from_props`'s `(index, class_name, data)` tuples
/// (where presence means enabled).
#[derive(Clone, Debug)]
struct ComponentRecord {
    #[allow(dead_code)]
    index: usize,
    class_name: String,
    data: Value,
    enabled: bool,
}

fn load_scene_file(
    path: &Path,
    registry: &pulsar_class::ClassRegistry,
) -> Result<SceneFile, RuntimeLevelError> {
    let io_error = |message: String| RuntimeLevelError::Io {
        path: path.display().to_string(),
        message,
    };
    let parse_error = |message: String| RuntimeLevelError::Parse {
        path: path.display().to_string(),
        message,
    };
    let bytes = engine_fs::virtual_fs::read_file(path).map_err(|e| io_error(e.to_string()))?;
    let mut value: Value =
        serde_json::from_slice(&bytes).map_err(|e| parse_error(e.to_string()))?;
    // Migrate on the raw JSON first: it still sees fields the typed
    // `SceneFile` drops (a `Blueprint` object type, a flat `script_asset`).
    let report = pulsar_class::migrate::migrate_level_value(&mut value, registry);
    if report.changed() {
        tracing::info!(
            path = %path.display(),
            script_components = report.script_components.len(),
            bindings = report.bindings.len(),
            "Migrated level classes to ClassInstance"
        );
    }
    serde_json::from_value(value).map_err(|error| parse_error(error.to_string()))
}

/// Classes of the current project (`engine_state`'s project path); empty
/// when no project is set, which leaves every placed class unresolved.
fn project_class_registry() -> pulsar_class::ClassRegistry {
    match engine_state::get_project_path() {
        Some(root) => pulsar_class::ClassRegistry::scan(Path::new(&root)),
        None => pulsar_class::ClassRegistry::default(),
    }
}

/// Run the #921 class migration over an already-parsed file: its objects'
/// component lists, the top-level `components` section and
/// `blueprint_bindings`.
fn migrate_scene_file(file: &mut SceneFile, registry: &pulsar_class::ClassRegistry) {
    let objects: Vec<Value> = file
        .objects
        .iter()
        .map(|obj| {
            let mut entry = serde_json::json!({ "id": obj.id, "props": obj.props });
            if let Some(instances) = &obj.component_instances {
                entry["component_instances"] = instances.clone();
            }
            entry
        })
        .collect();
    let mut value = serde_json::json!({
        "objects": objects,
        "components": file.components,
        "blueprint_bindings": file.blueprint_bindings,
    });
    let report = pulsar_class::migrate::migrate_level_value(&mut value, registry);
    if !report.changed() {
        return;
    }
    let records = &report.records;
    if records.changed() {
        tracing::info!(
            plugins = records.plugins.len(),
            nested = records.nested.len(),
            stripped_props = records.stripped_props.len(),
            retired_fields = records.retired_fields.len(),
            "Migrated legacy component records"
        );
    }
    if let Some(objects) = value.get("objects").and_then(Value::as_array) {
        for (obj, migrated) in file.objects.iter_mut().zip(objects) {
            if let Some(props) = migrated
                .get("props")
                .and_then(|p| serde_json::from_value(p.clone()).ok())
            {
                obj.props = props;
            }
            obj.component_instances = migrated.get("component_instances").cloned();
        }
    }
    file.components = value.get("components").cloned().unwrap_or(Value::Null);
    file.blueprint_bindings = value
        .get("blueprint_bindings")
        .and_then(|b| serde_json::from_value(b.clone()).ok())
        .unwrap_or_default();
}

fn version_string(version: &Value) -> String {
    match version {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Spawn one file-format object into the world, with its props and
/// component-instance JSON attached.
fn spawn_file_object(
    world: &mut World,
    obj: &pulsar_scene::format::SceneObject,
) -> Result<Entity, RuntimeLevelError> {
    let parse_error = |message: String| RuntimeLevelError::Parse {
        path: String::new(),
        message,
    };
    let parent = match &obj.parent {
        Some(parent_id) => Some(world.entity_for(parent_id).ok_or_else(|| {
            parse_error(format!(
                "object '{}' references parent '{}' before it is available",
                obj.id, parent_id
            ))
        })?),
        None => None,
    };
    let entity = world
        .spawn_object(SpawnObject {
            stable_id: Some(obj.id.clone()),
            name: obj.name.clone(),
            parent,
            transform: Transform {
                position: obj.world_position(),
                rotation: obj.world_rotation(),
                scale: obj.world_scale(),
            },
            visibility: Visibility {
                visible: obj.visible,
                locked: obj.locked,
            },
            object_type: object_type(obj.object_type),
        })
        .map_err(|error| parse_error(error.to_string()))?;
    if let Some(mut props) = world.get_mut::<RenderProps>(entity) {
        props.props = obj.props.clone();
    }
    Ok(entity)
}

/// `pulsar_scene`'s loader-facing object classification ->
/// `engine_backend`'s store-facing one.
fn object_type(object_type: FileObjectType) -> ObjectType {
    match object_type {
        FileObjectType::Empty | FileObjectType::Unknown => ObjectType::Empty,
        FileObjectType::Folder => ObjectType::Folder,
        FileObjectType::Camera => ObjectType::Camera,
    }
}

/// Parse the editor's persisted components section:
/// `{ "<object_id>": [ { "class_name": ..., "data": ..., "enabled": ... } ] }`.
/// Lenient by design -- entries missing a class name are skipped; a missing
/// `enabled` flag means enabled (matching how the editor treats inline
/// instances).
fn persisted_components(components: &Value) -> HashMap<String, Vec<ComponentRecord>> {
    let mut out = HashMap::new();
    let Some(map) = components.as_object() else {
        return out;
    };
    for (object_id, entries) in map {
        let Some(array) = entries.as_array() else {
            continue;
        };
        let records = array
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                Some(ComponentRecord {
                    index: entry
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|value| value as usize)
                        .unwrap_or(index),
                    class_name: entry.get("class_name")?.as_str()?.to_string(),
                    data: entry.get("data").cloned().unwrap_or(Value::Null),
                    enabled: entry
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                })
            })
            .collect();
        out.insert(object_id.clone(), records);
    }
    out
}

fn validate_objects(
    world: &World,
    objects: &[pulsar_scene::format::SceneObject],
) -> Result<(), RuntimeLevelError> {
    let mut seen = HashSet::with_capacity(objects.len());
    for obj in objects {
        if world.entity_for(&obj.id).is_some() || !seen.insert(obj.id.as_str()) {
            return Err(RuntimeLevelError::Parse {
                path: String::new(),
                message: format!("duplicate stable id '{}'", obj.id),
            });
        }
        if let Some(parent) = &obj.parent {
            if !seen.contains(parent.as_str()) && world.entity_for(parent).is_none() {
                return Err(RuntimeLevelError::Parse {
                    path: String::new(),
                    message: format!(
                        "object '{}' references parent '{}' before it is available",
                        obj.id, parent
                    ),
                });
            }
        }
    }
    Ok(())
}
/// Attach `instances` to `entity` as component-instance entities, in order
/// (Pulsar-Native#1035, D1): every record becomes its own instance, enabled
/// or not, with its stable id and parent link. A class this build does not
/// register is kept as an explicit unresolved payload; a registered class
/// whose data does not decode fails the load, naming the object and class.
fn hydrate_components(
    world: &mut World,
    entity: pulsar_scenedb::Entity,
    object_id: &str,
    instances: &[ComponentRecord],
) -> Result<(), RuntimeLevelError> {
    let records: Vec<pulsar_scene_model::ComponentInstance> = instances
        .iter()
        .map(|record| pulsar_scene_model::ComponentInstance {
            class_name: record.class_name.clone(),
            enabled: record.enabled,
            data: record.data.clone(),
        })
        .collect();
    let attached =
        pulsar_world_registry::attach_records(world, entity, &records).map_err(|error| {
            RuntimeLevelError::ComponentHydration {
                object_id: object_id.to_string(),
                class_name: String::new(),
                message: error.to_string(),
            }
        })?;
    for (record, instance) in records.iter().zip(attached) {
        let Some(unresolved) = world.get::<pulsar_scene_model::UnresolvedComponent>(instance)
        else {
            continue;
        };
        if pulsar_world_registry::component_id_for_class(&record.class_name).is_some() {
            return Err(RuntimeLevelError::ComponentHydration {
                object_id: object_id.to_string(),
                class_name: record.class_name.clone(),
                message: unresolved.reason.clone(),
            });
        }
    }
    Ok(())
}

/// Read `editor.camera` out of a level file's editor section, if present.
fn editor_camera(editor: &Value) -> Option<EditorCamera> {
    let cam = editor.get("camera")?;
    let pos = cam.get("position")?.as_array()?;
    if pos.len() < 3 {
        return None;
    }
    Some(EditorCamera {
        position: [pos[0].as_f64()?, pos[1].as_f64()?, pos[2].as_f64()?],
        yaw: cam.get("yaw").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        pitch: cam.get("pitch").and_then(Value::as_f64).unwrap_or(0.0) as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::StableId;

    const SAMPLE_LEVEL: &str = r#"{
        "version": "2.1",
        "objects": [
            {
                "id": "sun", "name": "Sun", "object_type": {"Light": "Point"},
                "transform": {"position": [1.0, 5.0, 2.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
                "parent": null, "visible": true, "locked": false, "props": {}
            },
            {
                "id": "group", "name": "Group", "object_type": "Folder",
                "transform": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
                "parent": null, "visible": true, "locked": false, "props": {}
            },
            {
                "id": "cube", "name": "Cube", "object_type": {"Mesh": "Cube"},
                "transform": {"position": [4.0, 0.0, 0.0], "rotation": [0.0, 90.0, 0.0], "scale": [2.0, 2.0, 2.0]},
                "parent": "group", "visible": false, "locked": true, "props": {}
            }
        ],
        "components": {},
        "metadata": {},
        "editor": {"camera": {"position": [10.0, 20.0, 30.0], "yaw": 1.0, "pitch": -0.25}}
    }"#;

    /// The object's attached component records, in order.
    fn records(world: &World, id: &str) -> Vec<pulsar_scene_model::ComponentInstance> {
        pulsar_world_registry::component_records(world, world.entity_for(id).unwrap())
    }

    fn sample_level() -> RuntimeLevel {
        let file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        RuntimeLevel::from_scene_file(file).expect("sample hydrates")
    }

    /// A level saved before transforms were parent-relative keeps every object
    /// where it was; a current level's values are already local.
    #[test]
    fn legacy_world_space_transforms_load_as_parent_relative() {
        let level = |version: &str| {
            let json = format!(
                r#"{{
                "version": "{version}",
                "objects": [
                    {{"id": "group", "name": "Group", "object_type": "Folder",
                      "transform": {{"position": [10.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}},
                      "parent": null, "visible": true, "locked": false, "props": {{}}}},
                    {{"id": "cube", "name": "Cube", "object_type": {{"Mesh": "Cube"}},
                      "transform": {{"position": [11.0, 2.0, 3.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}},
                      "parent": "group", "visible": true, "locked": false, "props": {{}}}}
                ],
                "components": {{}}, "metadata": {{}}
            }}"#
            );
            let file: SceneFile = serde_json::from_str(&json).expect("parses");
            RuntimeLevel::from_scene_file(file).expect("hydrates")
        };
        let local_of_cube = |level: &RuntimeLevel| {
            let scene = level.scene();
            let scene = scene.read();
            let cube = scene.world.entity_for("cube").unwrap();
            scene.world.get::<Transform>(cube).unwrap().position
        };
        assert_eq!(
            local_of_cube(&level("2.1")),
            [1.0, 2.0, 3.0],
            "2.1 held world values"
        );
        assert_eq!(
            local_of_cube(&level("2.2")),
            [11.0, 2.0, 3.0],
            "2.2 holds local values"
        );
    }

    /// #637: objects land in the shared store with transforms/hierarchy/
    /// visibility intact -- one copy of state, owned by SceneDB.
    #[test]
    fn load_hydrates_objects_hierarchy_and_visibility_into_the_store() {
        let level = sample_level();
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;

        let sun = world.entity_for("sun").expect("sun loaded");
        assert_eq!(
            world.get::<Transform>(sun).unwrap().position,
            [1.0, 5.0, 2.0]
        );
        assert_eq!(world.get::<Visibility>(sun).unwrap().visible, true);

        let cube = world.entity_for("cube").expect("cube loaded");
        assert_eq!(
            *world.get::<Visibility>(cube).unwrap(),
            Visibility {
                visible: false,
                locked: true
            }
        );
        let group = world.entity_for("group").unwrap();
        assert_eq!(world.parent_of(cube), Some(group));
        assert_eq!(world.children_of(Some(group)), vec![cube]);
    }

    fn level_with_sun_components(components_section: Value, instances: Option<Value>) -> SceneFile {
        let mut file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        if let Some(instances) = instances {
            file.objects[0].component_instances = Some(instances);
        }
        file.components = components_section;
        file
    }

    #[test]
    fn editor_camera_is_extracted_when_present() {
        let level = sample_level();
        assert_eq!(
            level.editor_camera(),
            Some(EditorCamera {
                position: [10.0, 20.0, 30.0],
                yaw: 1.0,
                pitch: -0.25
            })
        );
    }

    #[test]
    fn unsupported_versions_are_rejected() {
        let json = SAMPLE_LEVEL.replace("\"version\": \"2.1\"", "\"version\": \"9.9\"");
        let file: SceneFile = serde_json::from_str(&json).unwrap();
        // `.err().unwrap()` rather than `.unwrap_err()` -- the latter needs
        // `RuntimeLevel: Debug` (the `Ok` type), which it doesn't implement
        // (it holds a `SceneDb`, which doesn't either).
        assert_eq!(
            RuntimeLevel::from_scene_file(file).err().unwrap(),
            RuntimeLevelError::UnsupportedVersion("9.9".into())
        );
    }

    /// Stable ids survive exactly as authored (save/load identity), unlike
    /// raw Entity bits (#553 decision #2).
    #[test]
    fn stable_ids_round_trip_as_authored() {
        let level = sample_level();
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;
        let cube = world.entity_for("cube").unwrap();
        assert_eq!(
            world.get::<StableId>(cube).map(|s| s.0.clone()),
            Some("cube".into())
        );
    }

    /// Legacy `blueprint_bindings` (#650) are read only through the #921
    /// migration: a binding becomes the object's `ClassInstance`, with its
    /// overrides as variable overrides, which the script driver follows
    /// (#922). Nothing is left for hosts to apply.
    #[test]
    fn legacy_blueprint_bindings_load_as_class_instances() {
        let project = tempfile::tempdir().unwrap();
        let class_dir = project.path().join("src").join("classes").join("TickProbe");
        std::fs::create_dir_all(&class_dir).unwrap();
        std::fs::write(class_dir.join("graph_save.json"), "{}").unwrap();
        let registry = pulsar_class::ClassRegistry::scan(project.path());

        let mut file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        file.blueprint_bindings.insert(
            "cube".to_string(),
            vec![pulsar_scene::BlueprintBinding {
                class_name: "TickProbe".to_string(),
                overrides: [("speed".to_string(), serde_json::json!(2.5))].into(),
            }],
        );
        let level = RuntimeLevel::from_scene_file_with_classes(file, &registry)
            .expect("bound shape hydrates");
        assert!(level.editor_camera().is_some(), "camera extras unchanged");
        let scene = level.scene();
        let scene = scene.read();
        let cube = scene.world.entity_for("cube").unwrap();
        let instance = pulsar_class::world::class_instance_of(&scene.world, cube)
            .expect("binding migrated to a ClassInstance");
        assert_eq!(instance.class_name, "TickProbe");
        assert_eq!(instance.class, registry.by_name("TickProbe").unwrap().id);
        assert_eq!(instance.variable_overrides["speed"], serde_json::json!(2.5));
    }
}
