//! Play-mode level bootstrap: hydrate a `.level` file into
//! the scene `World` (Pulsar-Native#637).
//!
//! This is the runtime counterpart of the editor's own load path
//! (`SceneDatabase::load_from_file`): one authoritative copy of the scene,
//! owned by SceneDB, that renderers and gameplay share -- NOT a direct
//! imperative load into a Helio `Scene` (that was `pulsar_scene::
//! SceneLoader`, now legacy/import-only).
//!
//! Hydration mirrors the editor exactly:
//!
//! - Objects/transforms/hierarchy/visibility are spawned straight into the
//!   world with [`SceneWorldExt::spawn_object`].
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
//! components are authoritative when present"): a top-level
//! `components` entry for an object wins over that object's own
//! `component_instances`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use parking_lot::RwLock;
use pulsar_scene::component_instances_from_props;
use pulsar_scene::format::{
    BlueprintBindings, ObjectType as FileObjectType, SceneFile, SceneLoadError,
};
use serde_json::Value;

use pulsar_scenedb::{Entity, World};

use crate::scene::{
    LightType, MeshType, ObjectType, RenderProps, SceneWorldExt, SharedScene, SpawnObject,
    Transform, Visibility,
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
/// position + yaw/pitch in radians, the same convention `FreeCam::place`
/// uses. Lets a play-mode camera start where the editor view was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorCamera {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// What a level load reports beyond hydration itself (#650): the editor
/// camera seed and the file's Blueprint class bindings. Hosts apply the
/// bindings through `pulsar_game::blueprint_runtime::level_bindings`, which
/// resolves each StableId against the hydrated store and spawns one bound
/// dispatcher instance per (object, class) pair.
#[derive(Clone, Debug, Default)]
pub struct LevelExtras {
    /// The camera saved under `editor.camera`, if any.
    pub editor_camera: Option<EditorCamera>,
    /// Object → Blueprint class bindings keyed by StableId; empty for
    /// pre-#650 files.
    pub blueprint_bindings: BlueprintBindings,
}

/// A scene loaded for runtime use: one shared SceneDB scene plus
/// the level-file extras gameplay cares about (editor camera seed, Blueprint
/// class bindings).
pub struct RuntimeLevel {
    scene: SharedScene,
    extras: LevelExtras,
}

impl RuntimeLevel {
    /// Load and hydrate a level file. See the module doc for the hydration
    /// contract; call `engine_state::set_project_path` first so asset-
    /// resolving hydrates (`StaticMeshComponent`) can find project files.
    pub fn load(path: &Path) -> Result<Self, RuntimeLevelError> {
        let file = load_scene_file(path)?;
        Self::from_scene_file(file)
    }
    /// Load a level file and hydrate it into an EXISTING world -- the
    /// one-world play-mode path (Pulsar-Native#637/#634): the tick loop's
    /// shared store is authoritative, so the level merges INTO it
    /// (additively; setup-time-registered actors survive) instead of the
    /// level constructing its own store. Duplicate stable ids between the
    /// file and live state are errors, never silent re-spawns.
    ///
    /// Returns the file's extras ([`LevelExtras`]: editor camera + Blueprint
    /// class bindings). Call `engine_state::set_project_path` first so
    /// asset-resolving hydrates (`StaticMeshComponent`) can find project
    /// files.
    pub fn load_into(
        path: &Path,
        world: &mut World,
    ) -> Result<LevelExtras, RuntimeLevelError> {
        let file = load_scene_file(path)?;
        let extras = LevelExtras {
            editor_camera: editor_camera(&file.editor),
            blueprint_bindings: file.blueprint_bindings.clone(),
        };
        Self::hydrate_scene_file(file, world)?;
        Ok(extras)
    }
    /// Hydrate from an already-parsed [`SceneFile`] into a fresh store
    /// (import/legacy callers that get their JSON from somewhere other than
    /// disk).
    pub fn from_scene_file(file: SceneFile) -> Result<Self, RuntimeLevelError> {
        let extras = LevelExtras {
            editor_camera: editor_camera(&file.editor),
            blueprint_bindings: file.blueprint_bindings.clone(),
        };
        let mut scene = crate::scene::new_scene();
        Self::hydrate_scene_file(file, &mut scene.world)?;
        Ok(Self {
            scene: Arc::new(RwLock::new(scene)),
            extras,
        })
    }

    /// Shared hydration core: version gate + objects + components into
    /// `world`.
    fn hydrate_scene_file(
        file: SceneFile,
        world: &mut World,
    ) -> Result<(), RuntimeLevelError> {
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
        for obj in &file.objects {
            spawn_file_object(world, obj)?;
        }

        let persisted = persisted_components(&file.components);
        for obj in &file.objects {
            let Some(entity) = world.entity_for(&obj.id) else {
                continue;
            };
            let (instances, has_component_source) = match persisted.get(&obj.id) {
                // A persisted entry is authoritative, including an explicit empty
                // array, which means all registered components are removed.
                Some(records) => (records.clone(), true),
                None => {
                    let records = component_instances_from_props(
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
                    .collect::<Vec<_>>();
                    (records, component_source_present(obj))
                }
            };

            // SceneDB keeps the ordered compatibility projection as well as the
            // typed registered component values. Older consumers can therefore
            // observe the same enabled/order state without another scene list.
            if has_component_source {
                let component_instances = component_records_value(&instances);
                if let Some(mut props) = world.get_mut::<RenderProps>(entity) {
                    props.component_instances = Some(component_instances);
                }
            }
            hydrate_components(world, entity, &obj.id, &instances)?;
        }
        Ok(())
    }

    /// The shared, authoritative scene. Renderers and the tick loop all clone
    /// this handle.
    pub fn scene(&self) -> SharedScene {
        Arc::clone(&self.scene)
    }

    /// The level's extras: editor camera seed + Blueprint class bindings
    /// (#650). Bindings are NOT applied by hydration itself — hosts apply
    /// them through `pulsar_game::blueprint_runtime::level_bindings` so the
    /// dispatcher stays a gameplay-side concern.
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

fn load_scene_file(path: &Path) -> Result<SceneFile, RuntimeLevelError> {
    SceneFile::load(path).map_err(|error| match error {
        SceneLoadError::Io(message) => RuntimeLevelError::Io {
            path: path.display().to_string(),
            message,
        },
        SceneLoadError::Parse(message) => RuntimeLevelError::Parse {
            path: path.display().to_string(),
            message,
        },
    })
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
        props.component_instances = obj.component_instances.clone();
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

/// Hydrate/remove every registered class typed World value for an entity.
fn component_source_present(obj: &pulsar_scene::format::SceneObject) -> bool {
    obj.component_instances
        .as_ref()
        .and_then(Value::as_array)
        .is_some()
        || obj
            .props
            .get("__component_instances")
            .and_then(Value::as_array)
            .is_some()
}

fn component_records_value(records: &[ComponentRecord]) -> Value {
    Value::Array(
        records
            .iter()
            .map(|record| {
                serde_json::json!({
                    "index": record.index,
                    "class_name": record.class_name,
                    "data": record.data,
                    "enabled": record.enabled,
                })
            })
            .collect(),
    )
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
fn hydrate_components(
    world: &mut World,
    entity: pulsar_scenedb::Entity,
    object_id: &str,
    instances: &[ComponentRecord],
) -> Result<(), RuntimeLevelError> {
    for class_name in pulsar_world_registry::registered_world_component_classes() {
        match instances
            .iter()
            .find(|r| r.enabled && r.class_name == *class_name)
        {
            Some(record) => {
                pulsar_world_registry::hydrate_world_component_for_class(
                    class_name,
                    world,
                    entity,
                    &record.data,
                )
                .map_err(|error| RuntimeLevelError::ComponentHydration {
                    object_id: object_id.to_string(),
                    class_name: class_name.to_string(),
                    message: error.to_string(),
                })?;
            }
            None => {
                pulsar_world_registry::remove_world_component_for_class(
                    class_name,
                    world,
                    entity,
                );
            }
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
        position: [
            pos[0].as_f64()? as f32,
            pos[1].as_f64()? as f32,
            pos[2].as_f64()? as f32,
        ],
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

    fn render_props(world: &World, id: &str) -> RenderProps {
        world
            .get::<RenderProps>(world.entity_for(id).unwrap())
            .cloned()
            .unwrap()
    }

    fn sample_level() -> RuntimeLevel {
        let file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        RuntimeLevel::from_scene_file(file).expect("sample hydrates")
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
        assert_eq!(world.get::<Transform>(sun).unwrap().position, [1.0, 5.0, 2.0]);
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
        let mut file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
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
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;

        let sun = world.entity_for("sun").unwrap();
        let light = world.get::<LightComponent>(sun).expect("hydrated");
        assert_eq!(light.intensity.intensity, 750.0);
        assert!(
            world.get::<helio_component::components::LightComponentGpuMirror>(sun)
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
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;

        let sun = world.entity_for("sun").unwrap();
        let props = render_props(world, "sun");
        let instances = props.component_instances.expect("kept as JSON");
        assert!(instances.to_string().contains("NotARealComponent"));
        assert!(world.get::<LightComponent>(sun).is_none());
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
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;

        let light = world.entity_for("sun")
            .and_then(|e| world.get::<LightComponent>(e))
            .expect("persisted map drove hydration");
        assert_eq!(
            light.intensity.intensity, 99.0,
            "disabled record must lose to the enabled one"
        );
    }

    #[test]
    fn persisted_empty_component_list_is_authoritative() {
        let file = level_with_sun_components(
            serde_json::json!({ "sun": [] }),
            Some(light_instances_json(1111.0)),
        );
        let level = RuntimeLevel::from_scene_file(file).unwrap();
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;
        let sun = world.entity_for("sun").unwrap();

        assert!(
            world.get::<LightComponent>(sun).is_none(),
            "an explicit empty persisted list removes the inline component"
        );
        assert_eq!(
            render_props(world, "sun").component_instances,
            Some(serde_json::json!([])),
            "the removal remains visible in SceneDB metadata"
        );
    }

    #[test]
    fn persisted_component_order_and_enabled_state_are_kept_in_scene_db_projection() {
        let mut disabled = LightComponent::default();
        disabled.general.enabled = true;
        let mut enabled = LightComponent::default();
        enabled.general.enabled = true;
        enabled.intensity.intensity = 99.0;

        let file = level_with_sun_components(
            serde_json::json!({
                "sun": [
                    {
                        "index": 7,
                        "class_name": "LightComponent",
                        "data": serde_json::to_value(&disabled).unwrap(),
                        "enabled": false
                    },
                    {
                        "index": 3,
                        "class_name": "NotARealComponent",
                        "data": { "x": 1 },
                        "enabled": true
                    },
                    {
                        "index": 11,
                        "class_name": "LightComponent",
                        "data": serde_json::to_value(&enabled).unwrap(),
                        "enabled": true
                    }
                ]
            }),
            Some(light_instances_json(1111.0)),
        );
        let level = RuntimeLevel::from_scene_file(file).unwrap();
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;
        let records = render_props(world, "sun")
            .component_instances
            .unwrap();
        let records = records.as_array().unwrap();

        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["index"], serde_json::json!(7));
        assert_eq!(records[0]["enabled"], serde_json::json!(false));
        assert_eq!(records[1]["index"], serde_json::json!(3));
        assert_eq!(records[2]["index"], serde_json::json!(11));
        assert_eq!(records[2]["enabled"], serde_json::json!(true));
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

    /// #650 additive guarantee: files without `blueprint_bindings` load with
    /// empty extras, and an authored bindings section rides along unharmed
    /// (hydration itself never applies it — hosts do, via
    /// `pulsar_game::blueprint_runtime::level_bindings`).
    #[test]
    fn blueprint_bindings_are_additive_extras() {
        let old: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        let level = RuntimeLevel::from_scene_file(old).expect("old shape hydrates");
        assert!(level.extras().blueprint_bindings.is_empty());
        assert!(level.editor_camera().is_some(), "camera extras unchanged");

        let mut file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        file.blueprint_bindings.insert(
            "cube".to_string(),
            vec![pulsar_scene::BlueprintBinding {
                class_name: "TickProbe".to_string(),
                overrides: std::collections::HashMap::new(),
            }],
        );
        let level = RuntimeLevel::from_scene_file(file).expect("bound shape hydrates");
        let bound = &level.extras().blueprint_bindings["cube"];
        assert_eq!(bound.len(), 1);
        assert_eq!(bound[0].class_name, "TickProbe");
    }
}
