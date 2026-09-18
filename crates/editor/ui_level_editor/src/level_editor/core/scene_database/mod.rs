//! Editor scene facade over the SceneDB world shared with Helio.
//! Objects, hierarchy, selection, typed components, and component attachment
//! state all live in that world. JSON is used for persistence and dormant or
//! unregistered component instances; live registered values are authoritative.

use engine_backend::scene::SceneComponentStore;
use engine_backend::scene::{
    ObjectDirtyFlags, Transform as WorldTransform, Visibility as WorldVisibility,
    WorldSceneStoreError,
};
use engine_backend::{ComponentInstance, EditorObjectId};
use engine_fs::virtual_fs;
use parking_lot::RwLock;
use pulsar_reflection::{apply_scene_props_for_class, registered_scene_props_classes};
use pulsar_scenedb::Entity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::any::Any;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

fn is_scenedb_authority_class(class_name: &str) -> bool {
    pulsar_world_registry::component_id_for_class(class_name).is_some()
}

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

// ── Public re-exports for UI layer compatibility ───────────────────────────

pub use engine_backend::scene::{LightType, MeshType, ObjectId, ObjectType, WorldSceneStore};

// ── Transform ─────────────────────────────────────────────────────────────

/// Editor transform: position, Euler rotation (degrees), and scale.
///
/// Stored inline in `SceneObjectData` for easy UI access. The underlying
/// `WorldSceneStore` stores the same values behind one `RwLock` shared with
/// the renderer.
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

// ── SceneObjectData ────────────────────────────────────────────────────────

/// Snapshot of a single scene object – the primary data type used by editor panels.
///
/// This is a cheap-to-clone value that is produced by `SceneDatabase::get_object` /
/// `get_all_objects` and consumed by `SceneDatabase::add_object` /
/// `update_object`. Transform data is stored both here (for easy editing) and
/// in the underlying `WorldSceneStore` (shared with the renderer); calling
/// `update_object` keeps them in sync.
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
    /// Direct children (populated by `SceneDatabase` on read, ignored on write).
    pub children: Vec<ObjectId>,
    pub scene_path: String,
    /// Type-specific properties that round-trip through the level file.
    /// Lights: `"color_r"`, `"color_g"`, `"color_b"`, `"intensity"`, `"range"`.
    ///
    /// ⚠ This field does **not** contain `__component_instances`. Component
    /// data flows exclusively through `SceneDatabase::add_component` / etc.
    #[serde(default)]
    pub props: std::collections::HashMap<String, serde_json::Value>,
    /// Reflection-based component instances (synced from component_store).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_instances: Option<serde_json::Value>,
}

// ── Property change tracking ─────────────────────────────────────────────

/// Soft cap on the accumulated change set (see `record_property_change`).
const MAX_PROPERTY_CHANGE_SET: usize = 16_384;

/// Tracks which specific properties have been written since the last drain.
///
/// The properties panel drains this once per frame via [`SceneDatabase::drain_property_changes`]
/// and uses the result to skip World reads for unchanged properties — the
/// single biggest cost reduction for the panel.
#[derive(Default, Clone)]
pub struct PropertyChangeSet {
    /// `(object_id, class_name, prop_name)` triples written since last drain.
    changed: HashSet<(String, String, String)>,
    /// `(object_id, class_name)` pairs where a structural change occurred
    /// (add/remove/reorder/enable-disable) — the component list itself changed.
    structural: HashSet<(String, String)>,
    /// `true` when any component was added or removed on the target object,
    /// meaning the panel should rebuild its component card list entirely.
    components_added_or_removed: bool,
}

impl PropertyChangeSet {
    /// `true` if *any* property was written since the last drain.
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.structural.is_empty()
    }

    /// `true` when the component *list* changed (add/remove), not just a
    /// property value within an existing component.
    pub fn components_added_or_removed(&self) -> bool {
        self.components_added_or_removed
    }

    /// Check if a specific property was written since the last drain.
    pub fn has_changed(&self, object_id: &str, class_name: &str, prop_name: &str) -> bool {
        self.changed.contains(&(
            object_id.to_string(),
            class_name.to_string(),
            prop_name.to_string(),
        ))
    }

    /// Check if any property on a given class was written since the last drain.
    pub fn class_changed(&self, object_id: &str, class_name: &str) -> bool {
        self.changed
            .iter()
            .any(|(oid, cls, _)| oid == object_id && cls == class_name)
    }

    /// Non-consuming relevance check for one object: did anything touching
    /// `object_id`'s components change since the last drain?
    ///
    /// The properties panel's pump uses this to decide whether a scene
    /// revision bump needs the (expensive) component-card re-render at all —
    /// transform edits, gizmo drags and edits to *other* objects must not.
    fn touches_object(&self, object_id: &str) -> bool {
        self.components_added_or_removed
            || self.changed.iter().any(|(oid, _, _)| oid == object_id)
            || self.structural.iter().any(|(oid, _)| oid == object_id)
    }
}

// ── Production Scene Database ──────────────────────────────────────────────

/// Production-ready scene database — the single source of truth for all scene state.
///
/// Wraps `WorldSceneStore` (the `RwLock`-guarded object store shared with the
/// renderer) and `SceneComponentStore` for the reflection-based component system.
///
/// Helio consumes this world's GPU mirror; edits never target a renderer scene.
/// All UI panels and AI tools interact through `SceneDatabase` only.
#[derive(Clone)]
pub struct SceneDatabase {
    /// Primary store: transforms + hierarchy, behind one `RwLock` shared
    /// with the renderer.
    store: Arc<RwLock<WorldSceneStore>>,
    /// Attachment access through the same world lock, with no separate storage.
    component_store: Arc<SceneComponentStore>,
    /// Accumulated property changes since the last drain.
    /// Wrapped in `parking_lot::Mutex` so mutations can record changes
    /// while the outer `SceneDatabase` is `&self` (which it always is —
    /// the `RwLock<WorldSceneStore>` handles interior mutability for the
    /// World side).
    property_changes: Arc<parking_lot::Mutex<PropertyChangeSet>>,
    /// Folds the raw per-store `render_revision` into a value that is
    /// monotonic even across `restore_history_snapshot`'s wholesale store
    /// swap (undo/redo), which resets the raw counter to a fresh,
    /// deterministic value. Shared by every clone, like `store`.
    revision_tracker: Arc<RevisionTracker>,
}

/// Monotonicizer for [`SceneDatabase::store_revision`].
///
/// The raw counter lives on the current `WorldSceneStore`, and undo/redo
/// replaces that store with a freshly built one whose counter restarts at a
/// deterministic value (5 publishes per restored object). Comparing raw
/// values across a swap can therefore see "equal" or even "lower" without
/// anything being unchanged — e.g. undo then redo between two states with
/// the same object count lands on exactly the same number both times, and a
/// naive equality check silently misses the entire restore.
///
/// [`Self::note_swap`] is called at the one swap site, giving each store
/// generation its own epoch; within an epoch raw deltas accumulate verbatim,
/// and an epoch change itself counts as exactly one guaranteed change
/// regardless of what the new raw value is.
#[derive(Default)]
struct RevisionTracker {
    /// Store-swap generation; bumped by [`Self::note_swap`].
    epoch: std::sync::atomic::AtomicU64,
    last_epoch: std::sync::atomic::AtomicU64,
    last_raw: std::sync::atomic::AtomicU64,
    out: std::sync::atomic::AtomicU64,
}

impl RevisionTracker {
    fn note_swap(&self) {
        use std::sync::atomic::Ordering;
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }

    fn fold(&self, epoch: u64, raw: u64) -> u64 {
        use std::sync::atomic::Ordering;
        loop {
            let le = self.last_epoch.load(Ordering::Relaxed);
            let lr = self.last_raw.load(Ordering::Relaxed);
            let delta = if epoch != le {
                // Store was swapped: one guaranteed change no matter what the
                // fresh counter reads (it may be equal to or lower than the
                // baseline — that carries no information across epochs).
                1
            } else if raw > lr {
                raw - lr
            } else {
                0
            };

            if delta == 0 {
                // Nothing changed; just keep the baseline current (best
                // effort — a racing fold re-derives the same conclusion).
                let _ =
                    self.last_raw
                        .compare_exchange(lr, raw, Ordering::Relaxed, Ordering::Relaxed);
                return self.out.load(Ordering::Relaxed);
            }

            if epoch != le {
                // Claim the epoch transition so the swap's guaranteed delta
                // is applied by exactly one caller.
                match self.last_epoch.compare_exchange(
                    le,
                    epoch,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Err(_) => continue,
                    Ok(_) => {}
                }
            }
            // Claim the raw transition so intra-epoch growth is counted once.
            match self
                .last_raw
                .compare_exchange(lr, raw, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    let prev = self.out.fetch_add(delta, Ordering::Relaxed);
                    return prev + delta;
                }
                Err(_) => continue,
            }
        }
    }
}

/// A `StaticMeshComponent` data payload carrying every texture slot the
/// current class requires (Helio#237). Older scenes predate the slots; the
/// legacy `props.mesh_asset` projection and tests must emit all of them or
/// hydration's deserialization rejects the instance outright. Empty paths
/// mean "slot unassigned", which hydrate treats as zero-semantics.
fn static_mesh_component_json(mesh_asset: &str) -> serde_json::Value {
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


mod components;
mod history;
mod objects;
mod persistence;
mod store;

#[cfg(test)]
mod tests;

pub use history::SceneHistorySnapshot;
impl Default for SceneDatabase {
    fn default() -> Self {
        Self::new()
    }
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
    /// re-saves instead of being silently dropped. `save_to_file` preserves
    /// it by reading it back from the file on disk, mirroring how
    /// `preserved_editor` keeps camera state.
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
