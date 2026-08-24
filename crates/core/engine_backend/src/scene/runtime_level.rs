//! Play-mode level bootstrap: hydrate a `.level` file into
//! [`WorldSceneStore`]/SceneDb (Pulsar-Native#637).
//!
//! This is the runtime counterpart of the editor's own load path
//! (`SceneDatabase::load_from_file`): one authoritative copy of the scene,
//! owned by SceneDB, that renderers and gameplay share -- NOT a direct
//! imperative load into a Helio `Scene` (that was `pulsar_scene::
//! SceneLoader`, now legacy/import-only).
//!
//! Hydration mirrors the editor exactly:
//!
//! - Objects/transforms/hierarchy/visibility land via
//!   [`WorldSceneStore::load_from_snapshots`] (the same bridge undo/redo
//!   uses).
//! - Every enabled component instance whose class is
//!   `#[register_world_component]`-registered is hydrated to its typed
//!   World value through `pulsar_world_registry::
//!   hydrate_world_component_for_class` (`StaticMeshComponent`'s custom
//!   hydrate loads its mesh asset here -- it resolves paths via
//!   `engine_state::get_project_path()`, so callers must have set that
//!   before calling [`RuntimeLevel::load`]).
//! - Unregistered classes stay as metadata JSON in the object's
//!   `RenderProps.component_instances`, exactly as the editor does today.
//!
//! Component data source precedence matches the editor's ("persisted
//! components are authoritative when present"): a non-empty top-level
//! `components` entry for an object wins over that object's own
//! `component_instances`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::RwLock;
use pulsar_scene::component_instances_from_props;
use pulsar_scene::format::{ObjectType as FileObjectType, SceneFile};
use serde_json::Value;

use crate::scene::{
    LightType, MeshType, ObjectSnapshot, ObjectType, RenderProps, Transform, Visibility,
    WorldSceneStore,
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
}

/// Editor camera state persisted in the level file (`editor.camera`) --
/// position + yaw/pitch in radians, the same convention `FreeCam::place`
/// uses. Lets a play-mode camera start where the editor view was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorCamera {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// A scene loaded for runtime use: one shared, SceneDB-owned store plus
/// the level-file extras gameplay cares about (editor camera seed).
pub struct RuntimeLevel {
    store: Arc<RwLock<WorldSceneStore>>,
    editor_camera: Option<EditorCamera>,
}

impl RuntimeLevel {
    /// Load and hydrate a level file. See the module doc for the hydration
    /// contract; call `engine_state::set_project_path` first so asset-
    /// resolving hydrates (`StaticMeshComponent`) can find project files.
    pub fn load(path: &Path) -> Result<Self, RuntimeLevelError> {
        let path_display = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|e| RuntimeLevelError::Io {
            path: path_display.clone(),
            message: e.to_string(),
        })?;
        let text = String::from_utf8(bytes).map_err(|e| RuntimeLevelError::Io {
            path: path_display.clone(),
            message: e.to_string(),
        })?;
        let file: SceneFile = serde_json::from_str(&text).map_err(|e| {
            RuntimeLevelError::Parse { path: path_display.clone(), message: e.to_string() }
        })?;
        Self::from_scene_file(file)
    }

    /// Hydrate from an already-parsed [`SceneFile`] (import/legacy callers
    /// that get their JSON from somewhere other than disk).
    pub fn from_scene_file(file: SceneFile) -> Result<Self, RuntimeLevelError> {
        let version = version_string(&file.version);
        // Same accepted set as the editor's own loader: 1.x and 2.x.
        if !version.starts_with("1.") && !version.starts_with("2.") && version != "1" {
            return Err(RuntimeLevelError::UnsupportedVersion(version));
        }

        // Parent-before-child order is the format's own DFS guarantee (see
        // `SceneFile::objects`' doc), which is exactly what
        // `load_from_snapshots` requires.
        let snapshots: Vec<ObjectSnapshot> = file.objects.iter().map(object_snapshot).collect();
        let mut store =
            WorldSceneStore::load_from_snapshots(&snapshots).map_err(|error| {
                RuntimeLevelError::Parse { path: String::new(), message: error.to_string() }
            })?;

        let persisted = persisted_components(&file.components);
        for obj in &file.objects {
            let Some(entity) = store.entity_for(&obj.id) else { continue };
            let instances = match persisted.get(&obj.id) {
                Some(records) if !records.is_empty() => records.clone(),
                _ => component_instances_from_props(
                    &obj.props,
                    obj.component_instances.as_ref(),
                )
                .into_iter()
                .map(|(index, class_name, data)| ComponentRecord {
                    index,
                    class_name,
                    data,
                    enabled: true,
                })
                .collect(),
            };
            hydrate_components(&mut store, entity, &obj.id, &instances);
        }

        Ok(Self { store: Arc::new(RwLock::new(store)), editor_camera: editor_camera(&file.editor) })
    }

    /// The shared, authoritative scene store. Renderers and (once A2 lands)
    /// the tick loop all clone this handle.
    pub fn store(&self) -> Arc<RwLock<WorldSceneStore>> {
        Arc::clone(&self.store)
    }

    /// Editor camera saved with the level, if any.
    pub fn editor_camera(&self) -> Option<EditorCamera> {
        self.editor_camera
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

fn version_string(version: &Value) -> String {
    match version {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Map a file-format object onto the store's snapshot bridge type.
fn object_snapshot(obj: &pulsar_scene::format::SceneObject) -> ObjectSnapshot {
    ObjectSnapshot {
        stable_id: obj.id.clone(),
        name: obj.name.clone(),
        parent: obj.parent.clone(),
        transform: Transform {
            position: obj.world_position(),
            rotation: obj.world_rotation(),
            scale: obj.world_scale(),
        },
        visibility: Visibility { visible: obj.visible, locked: obj.locked },
        object_type: object_type(obj.object_type),
        render_props: RenderProps {
            props: obj.props.clone(),
            component_instances: obj.component_instances.clone(),
        },
    }
}

/// `pulsar_scene`'s loader-facing object classification ->
/// `engine_backend`'s store-facing one.
fn object_type(object_type: FileObjectType) -> ObjectType {
    match object_type {
        FileObjectType::Empty | FileObjectType::Unknown => ObjectType::Empty,
        FileObjectType::Folder => ObjectType::Folder,
        FileObjectType::Camera => ObjectType::Camera,
        FileObjectType::Mesh(mesh) => ObjectType::Mesh(match mesh {
            pulsar_scene::format::MeshType::Cube => MeshType::Cube,
            pulsar_scene::format::MeshType::Sphere => MeshType::Sphere,
            pulsar_scene::format::MeshType::Cylinder => MeshType::Cylinder,
            pulsar_scene::format::MeshType::Plane => MeshType::Plane,
            pulsar_scene::format::MeshType::Custom => MeshType::Custom,
        }),
        FileObjectType::Light(light) => ObjectType::Light(match light {
            pulsar_scene::format::LightType::Directional => LightType::Directional,
            pulsar_scene::format::LightType::Point => LightType::Point,
            pulsar_scene::format::LightType::Spot => LightType::Spot,
        }),
    }
}

/// Parse the editor's persisted components section:
/// `{ "<object_id>": [ { "class_name": ..., "data": ..., "enabled": ... } ] }`.
/// Lenient by design -- entries missing a class name are skipped; a missing
/// `enabled` flag means enabled (matching how the editor treats inline
/// instances).
fn persisted_components(components: &Value) -> HashMap<String, Vec<ComponentRecord>> {
    let mut out = HashMap::new();
    let Some(map) = components.as_object() else { return out };
    for (object_id, entries) in map {
        let Some(array) = entries.as_array() else { continue };
        let records = array
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                Some(ComponentRecord {
                    index,
                    class_name: entry.get("class_name")?.as_str()?.to_string(),
                    data: entry.get("data").cloned().unwrap_or(Value::Null),
                    enabled: entry.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                })
            })
            .collect();
        out.insert(object_id.clone(), records);
    }
    out
}

/// Hydrate/remove every registered class's typed World value for `entity`
/// against this object's enabled instance list; unregistered classes
/// intentionally stay JSON-only in `RenderProps` (the editor's exact
/// behavior). An absent or disabled registered class gets any stale typed
/// row removed, matching `sync_registered_component_props_to_scene_db`.
fn hydrate_components(
    store: &mut WorldSceneStore,
    entity: pulsar_scenedb::Entity,
    object_id: &str,
    instances: &[ComponentRecord],
) {
    for class_name in pulsar_world_registry::registered_world_component_classes() {
        match instances.iter().find(|r| r.enabled && r.class_name == *class_name) {
            Some(record) => {
                if let Err(error) = pulsar_world_registry::hydrate_world_component_for_class(
                    class_name,
                    store.world_mut(),
                    entity,
                    &record.data,
                ) {
                    tracing::error!(
                        "World hydration failed for {class_name} on '{object_id}': {error}"
                    );
                }
            }
            None => {
                pulsar_world_registry::remove_world_component_for_class(
                    class_name,
                    store.world_mut(),
                    entity,
                );
            }
        }
    }
}

/// Read `editor.camera` out of a level file's editor section, if present.
fn editor_camera(editor: &Value) -> Option<EditorCamera> {
    let cam = editor.get("camera")?;
    let pos = cam.get("position")?.as_array()?;
    if pos.len() < 3 {
        return None;
    }
    Some(EditorCamera {
        position: [pos[0].as_f64()? as f32, pos[1].as_f64()? as f32, pos[2].as_f64()? as f32],
        yaw: cam.get("yaw").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        pitch: cam.get("pitch").and_then(Value::as_f64).unwrap_or(0.0) as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::StableId;
    use helio_component::components::LightComponent;

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

    fn sample_level() -> RuntimeLevel {
        let file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        RuntimeLevel::from_scene_file(file).expect("sample hydrates")
    }

    /// #637: objects land in the shared store with transforms/hierarchy/
    /// visibility intact -- one copy of state, owned by SceneDB.
    #[test]
    fn load_hydrates_objects_hierarchy_and_visibility_into_the_store() {
        let level = sample_level();
        let store = level.store();
        let store = store.read();

        let sun = store.entity_for("sun").expect("sun loaded");
        assert_eq!(store.transform(sun).unwrap().position, [1.0, 5.0, 2.0]);
        assert_eq!(store.visibility(sun).unwrap().visible, true);

        let cube = store.entity_for("cube").expect("cube loaded");
        assert_eq!(store.visibility(cube).unwrap(), Visibility { visible: false, locked: true });
        let group = store.entity_for("group").unwrap();
        assert_eq!(store.parent_of(cube), Some(group));
        assert_eq!(store.children_of(Some(group)), &[cube]);
    }

    /// Build the component-instance array the way the editor itself writes
    /// it: `serde_json::to_value` of the FULL typed component (level files
    /// always carry complete component JSON, never sparse fragments -- the
    /// hydrate fns deserialize into the real structs, which don't default
    /// missing sub-groups).
    fn light_instances_json(intensity: f32) -> Value {
        let mut light = LightComponent::default();
        light.general.enabled = true;
        light.intensity.intensity = intensity;
        serde_json::json!([
            { "index": 0, "class_name": "LightComponent", "data": serde_json::to_value(&light).unwrap() }
        ])
    }

    fn level_with_sun_components(components_section: Value, instances: Option<Value>) -> SceneFile {
        let mut file: SceneFile =
            serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        if let Some(instances) = instances {
            file.objects[0].component_instances = Some(instances);
        }
        file.components = components_section;
        file
    }

    /// #637: registered classes hydrate to typed World values.
    #[test]
    fn registered_component_classes_hydrate_to_typed_values() {
        let file = level_with_sun_components(Value::Null, Some(light_instances_json(750.0)));
        let level = RuntimeLevel::from_scene_file(file).unwrap();
        let store = level.store();
        let store = store.read();

        let sun = store.entity_for("sun").unwrap();
        let light = store.world().get::<LightComponent>(sun).expect("hydrated");
        assert_eq!(light.intensity.intensity, 750.0);
        assert!(
            store
                .world()
                .get::<helio_component::components::LightComponentGpuMirror>(sun)
                .is_some(),
            "an enabled light carries its GPU mirror"
        );
    }

    /// #637: unregistered classes stay metadata JSON in RenderProps.
    #[test]
    fn unregistered_classes_stay_metadata_json() {
        let instances = serde_json::json!([
            { "index": 0, "class_name": "NotARealComponent", "data": { "x": 1 } }
        ]);
        let file = level_with_sun_components(Value::Null, Some(instances));
        let level = RuntimeLevel::from_scene_file(file).unwrap();
        let store = level.store();
        let store = store.read();

        let sun = store.entity_for("sun").unwrap();
        let props = store.render_props("sun").unwrap();
        let instances = props.component_instances.expect("kept as JSON");
        assert!(instances.to_string().contains("NotARealComponent"));
        assert!(store.world().get::<LightComponent>(sun).is_none());
    }

    /// #637: a non-empty persisted `components` map is authoritative over
    /// per-object `component_instances`, and disabled records don't hydrate.
    #[test]
    fn persisted_components_map_wins_and_respects_enabled() {
        let mut disabled = LightComponent::default();
        disabled.general.enabled = true;
        disabled.intensity.intensity = 42.0;
        let mut enabled = LightComponent::default();
        enabled.general.enabled = true;
        enabled.intensity.intensity = 99.0;
        // Inline instance data that must LOSE to the persisted map.
        let stale_inline = light_instances_json(1111.0);

        let components = serde_json::json!({
            "sun": [
                { "index": 0, "class_name": "LightComponent", "data": serde_json::to_value(&disabled).unwrap(), "enabled": false },
                { "index": 1, "class_name": "LightComponent", "data": serde_json::to_value(&enabled).unwrap(), "enabled": true }
            ]
        });
        let file = level_with_sun_components(components, Some(stale_inline));
        let level = RuntimeLevel::from_scene_file(file).unwrap();
        let store = level.store();
        let store = store.read();

        let light = store
            .entity_for("sun")
            .and_then(|e| store.world().get::<LightComponent>(e))
            .expect("persisted map drove hydration");
        assert_eq!(light.intensity.intensity, 99.0, "disabled record must lose to the enabled one");
    }

    #[test]
    fn editor_camera_is_extracted_when_present() {
        let level = sample_level();
        assert_eq!(
            level.editor_camera(),
            Some(EditorCamera { position: [10.0, 20.0, 30.0], yaw: 1.0, pitch: -0.25 })
        );
    }

    #[test]
    fn unsupported_versions_are_rejected() {
        let json = SAMPLE_LEVEL.replace("\"version\": \"2.1\"", "\"version\": \"9.9\"");
        let file: SceneFile = serde_json::from_str(&json).unwrap();
        // `.err().unwrap()` rather than `.unwrap_err()` -- the latter needs
        // `RuntimeLevel: Debug` (the `Ok` type), which it doesn't implement
        // (it holds a `WorldSceneStore`, which doesn't either).
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
        let store = level.store();
        let store = store.read();
        let cube = store.entity_for("cube").unwrap();
        assert_eq!(store.world().get::<StableId>(cube).map(|s| s.0.clone()), Some("cube".into()));
    }
}
