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
//! | [`classes`] | placed class instances: placement, rebuild, overrides, revert (#921) |
//!
//! Every attached component is its own entity holding its typed value
//! (Pulsar-Native#1035, D1). JSON is used only at boundaries: persistence,
//! history, tools, and the payload of a class this build does not register.

use crate::world_settings_data::WorldSettingsData;
use engine_backend::ComponentInstance;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

pub mod classes;
pub mod components;
pub mod history;
pub mod level_io;
pub mod objects;

#[cfg(test)]
mod hlfs_cathedral;
#[cfg(test)]
mod tests;

pub use engine_backend::scene::{LightType, MeshType, ObjectId, ObjectType};
pub use history::{SceneHistoryDelta, SceneHistorySnapshot};

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
    /// Inline component instances of an object being added (older v2
    /// level files and add-object callers). Read models leave it `None`:
    /// the attached instances are read through [`components`].
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
    /// Legacy per-object Blueprint class bindings keyed by StableId (#650).
    ///
    /// Retired by `ClassInstance` (#921): loading migrates each entry to a
    /// `ClassInstance` on the bound object (overrides become
    /// `variable_overrides`) and saving no longer writes migrated entries.
    /// Only entries one `ClassInstance` per object cannot express (a second
    /// class on the same object) are carried over from the file on disk.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub blueprint_bindings: pulsar_scene::BlueprintBindings,
    pub metadata: LevelMetadata,
    /// Per-level simulation and gameplay settings. Missing values in older
    /// files use [`WorldSettingsData::default`].
    #[serde(default)]
    pub world_settings: WorldSettingsData,
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
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

// ── Blueprint helpers ──────────────────────────────────────────────────────

/// A `StaticMeshComponent` data payload carrying every texture slot the
/// current class requires (Helio#237), for the HLFS demo scene builder.
/// Empty paths mean "slot unassigned".
#[cfg(test)]
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

/// Extract the legacy script asset path for a Blueprint object (a class
/// directory, which `objects::add_object` turns into a `ClassInstance`).
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
