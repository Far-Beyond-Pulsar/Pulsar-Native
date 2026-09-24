//! Editor scene operations, written directly against the SceneDB world.
//!
//! There is no store or facade here: the scene is a `pulsar_scenedb::SceneDb`
//! shared with the renderer (see [`engine_backend::scene::SharedScene`]) and every
//! function in this module takes the `World` it should read or edit. Callers
//! hold the lock, so a whole edit is one lock scope and nothing here can
//! deadlock by re-entering it.
//!
//! | Module | What |
//! |--------|------|
//! | [`objects`] | object queries, creation/removal, hierarchy, name/visibility/transform edits |
//! | [`components`] | attach/remove/enable/reorder component instances, live property edits |
//! | [`level_io`] | `.level` file save/load |
//! | [`history`] | undo/redo snapshots (editor-owned; SceneDB has no undo) |
//! | [`changes`] | which component properties changed, for the properties panel's relevance gate |
//!
//! JSON is used for persistence and for dormant or unregistered component
//! instances; live registered component values in the world are authoritative.

use engine_backend::ComponentInstance;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

pub mod changes;
pub mod components;
pub mod history;
pub mod level_io;
pub mod objects;

#[cfg(test)]
mod hlfs_cathedral;
#[cfg(test)]
mod tests;

pub use changes::PropertyChangeSet;
pub use engine_backend::scene::{LightType, MeshType, ObjectId, ObjectType};
pub use history::{SceneHistoryDelta, SceneHistorySnapshot};

/// Whether `class_name` has a live typed component registered in the world
/// (as opposed to being a JSON-only attachment).
fn is_scenedb_authority_class(class_name: &str) -> bool {
    pulsar_world_registry::component_id_for_class(class_name).is_some()
}

/// The `__`-prefixed editor metadata keys of a component's JSON (e.g.
/// `__parent_index`), which live on the attachment record rather than in the
/// typed component.
fn attachment_data(data: &Value) -> Value {
    let metadata: serde_json::Map<String, Value> = data
        .as_object()
        .into_iter()
        .flat_map(|map| map.iter())
        .filter(|(key, _)| key.starts_with("__"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if metadata.is_empty() {
        Value::Null
    } else {
        Value::Object(metadata)
    }
}

fn overlay_live_data(data: &Value, mut live: Value) -> Value {
    if let (Some(metadata), Some(live)) = (attachment_data(data).as_object(), live.as_object_mut())
    {
        live.extend(metadata.clone());
    }
    live
}

fn remap_component_parents(
    components: &mut [ComponentInstance],
    remap: impl Fn(usize) -> Option<usize>,
) {
    for component in components {
        let Some(data) = component.data.as_object_mut() else {
            continue;
        };
        let Some(parent) = data.get("__parent_index").and_then(Value::as_u64) else {
            continue;
        };
        if let Some(parent) = remap(parent as usize) {
            data.insert("__parent_index".into(), serde_json::json!(parent));
        } else {
            data.remove("__parent_index");
        }
    }
}

// ── Transform ─────────────────────────────────────────────────────────────

/// Editor transform: position, Euler rotation (degrees), and scale. The same
/// values the world stores in `engine_backend::scene::Transform`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
        }
    }
}

impl From<engine_backend::scene::Transform> for Transform {
    fn from(t: engine_backend::scene::Transform) -> Self {
        Self {
            position: t.position,
            rotation: t.rotation,
            scale: t.scale,
        }
    }
}

impl From<&Transform> for engine_backend::scene::Transform {
    fn from(t: &Transform) -> Self {
        Self {
            position: t.position,
            rotation: t.rotation,
            scale: t.scale,
        }
    }
}

// ── SceneObjectData ────────────────────────────────────────────────────────

/// Value copy of one scene object, the shape editor panels and commands pass
/// around. Produced by [`objects::get_object`] / [`objects::get_all_objects`]
/// and consumed by [`objects::add_object`] / [`objects::update_object`]; the
/// world remains the only place the data lives.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneObjectData {
    pub id: ObjectId,
    pub name: String,
    pub object_type: ObjectType,
    pub transform: Transform,
    pub visible: bool,
    pub locked: bool,
    /// Parent object ID (`None` = root level).
    pub parent: Option<ObjectId>,
    /// Direct children (filled in on read, ignored on write).
    pub children: Vec<ObjectId>,
    pub scene_path: String,
    /// Type-specific properties that round-trip through the level file.
    /// Lights: `"color_r"`, `"color_g"`, `"color_b"`, `"intensity"`, `"range"`.
    ///
    /// This does **not** contain `__component_instances`. Component data flows
    /// exclusively through the [`components`] functions.
    #[serde(default)]
    pub props: HashMap<String, Value>,
    /// Reflection-based component instances (projection of the attachments).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_instances: Option<Value>,
}

// ── Level File Format ──────────────────────────────────────────────────────

/// JSON level file (version 2.x).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelFile {
    pub version: String,
    pub objects: Vec<SceneObjectData>,
    /// Reflection component instances keyed by object id.
    #[serde(default)]
    pub components: HashMap<ObjectId, Vec<ComponentInstance>>,
    /// Per-object Blueprint class bindings keyed by StableId (#650).
    ///
    /// The editor has no binding-authoring UI yet (editor phase F); the
    /// field exists so hand-authored or future sections survive editor
    /// re-saves instead of being silently dropped. Saving preserves it by
    /// reading it back from the file on disk, mirroring how the editor camera
    /// state is kept.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub blueprint_bindings: pulsar_scene::BlueprintBindings,
    pub metadata: LevelMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<LevelEditorFileState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelMetadata {
    pub created: String,
    pub modified: String,
    pub editor_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelEditorFileState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<LevelEditorCameraState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelEditorCameraState {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

// ── Blueprint helpers ──────────────────────────────────────────────────────

/// A `StaticMeshComponent` data payload carrying every texture slot the
/// current class requires (Helio#237). Older scenes predate the slots; the
/// legacy `props.mesh_asset` projection and tests must emit all of them or
/// hydration's deserialization rejects the instance outright. Empty paths
/// mean "slot unassigned", which hydrate treats as zero-semantics.
fn static_mesh_component_json(mesh_asset: &str) -> Value {
    serde_json::json!({
        "mesh_asset": mesh_asset,
        "base_color_asset": "",
        "normal_asset": "",
        "roughness_metallic_asset": "",
        "emissive_asset": "",
        "occlusion_asset": "",
        "specular_color_asset": "",
        "specular_weight_asset": ""
    })
}

/// Extract the script asset path for a Blueprint object.
///
/// Checks `component_instances[ScriptComponent].data.script_asset` first
/// (modern path), falls back to the legacy `props["__component_instances"]`
/// array, and finally the flat `props["script_asset"]`. Returns an empty
/// string if none are present (the user will fill it in via the properties panel).
fn find_script_path(props: &HashMap<String, Value>, component_instances: Option<&Value>) -> String {
    // Helper: find ScriptComponent data in a component-instances array.
    fn find_in(arr: &[Value]) -> Option<&str> {
        arr.iter()
            .find(|inst| inst.get("class_name").and_then(|v| v.as_str()) == Some("ScriptComponent"))
            .and_then(|inst| inst.get("data"))
            .and_then(|data| data.get("script_asset"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    }

    // 1. Dedicated field (modern).
    if let Some(arr) = component_instances.and_then(|v| v.as_array()) {
        if let Some(path) = find_in(arr) {
            return path.to_string();
        }
    }

    // 2. Legacy __component_instances inside props (older scene files).
    if let Some(arr) = props
        .get("__component_instances")
        .and_then(|v| v.as_array())
    {
        if let Some(path) = find_in(arr) {
            return path.to_string();
        }
    }

    // 3. Flat prop fallback.
    props
        .get("script_asset")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}
