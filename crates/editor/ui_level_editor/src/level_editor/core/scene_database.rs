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

impl SceneDatabase {
    pub fn new() -> Self {
        let store = Arc::new(RwLock::new(WorldSceneStore::new()));
        Self {
            component_store: Arc::new(SceneComponentStore::new(Arc::clone(&store))),
            store,
            property_changes: Arc::new(parking_lot::Mutex::new(PropertyChangeSet::default())),
            revision_tracker: Arc::new(RevisionTracker::default()),
        }
    }

    /// The underlying shared store handle -- for consumers that must hold the
    /// same world the editor mutates (the PIE host handing its world to the
    /// guest, #635; renderer construction, #637). Cloning is cheap; readers
    /// take `.read()`, writers `.write()`.
    pub fn shared_store(&self) -> Arc<RwLock<WorldSceneStore>> {
        Arc::clone(&self.store)
    }

    // ── Property change tracking ─────────────────────────────────────────

    /// Snapshot and clear the accumulated property changes.  Called exactly
    /// once per properties-panel render to decide which values need re-reading
    /// from World.
    pub fn drain_property_changes(&self) -> PropertyChangeSet {
        std::mem::take(&mut *self.property_changes.lock())
    }

    // ── World subscriptions (Pulsar-Native#575, SceneDB#47) ────────────────

    /// Store-swap generation the current `World` belongs to. Subscriptions
    /// live inside the `World` itself, so [`Self::restore_history_snapshot`]
    /// (undo/redo -- the one wholesale `*self.store.write() = new_store`
    /// site, whose epoch bump this reads straight out of `RevisionTracker`)
    /// silently kills every outstanding [`pulsar_scenedb::SubscriptionId`].
    /// A subscriber that caches snapshots against subscriptions MUST compare
    /// this value per frame and re-arm from scratch when it moves; within a
    /// stable epoch subscriptions stay valid forever (or until their entity
    /// despawns).
    pub fn subscriptions_epoch(&self) -> u64 {
        self.revision_tracker
            .epoch
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Arm a World-level change subscription for `(object_id, class_name)`'s
    /// live component -- the properties panel's subscribe-once-per-card
    /// replacement for poll-every-render. The returned
    /// [`pulsar_scenedb::SubscriptionId`] is what
    /// [`Self::take_world_component_events`] events are tagged with; drop it
    /// with [`Self::unsubscribe_component`] when the card unmounts.
    ///
    /// `None` when `class_name` has no `World`-registered component id (the
    /// legacy JSON-only classes), the object has no live entity yet, or the
    /// entity is dead -- all "nothing to subscribe to", not errors; callers
    /// keep polling those cards exactly as they did before.
    pub fn subscribe_component(
        &self,
        object_id: &ObjectId,
        class_name: &str,
    ) -> Option<pulsar_scenedb::SubscriptionId> {
        let cid = pulsar_world_registry::component_id_for_class(class_name)?;
        let mut store = self.store.write();
        let entity = store.entity_for(object_id)?;
        store.world_mut().subscribe_id(entity, cid)
    }

    /// Disarm a previously armed subscription. Idempotent no-op if it is
    /// already gone (unsubscribed, or its whole `World` was swapped out from
    /// under it by undo/redo).
    pub fn unsubscribe_component(&self, sub: pulsar_scenedb::SubscriptionId) {
        self.store.write().world_mut().unsubscribe(sub);
    }

    /// Drain every pending World component-change event (SceneDB#47's
    /// batched delivery). Call once per frame at your frame boundary --
    /// between drains the queue accumulates, bounded by SceneDB's own cap.
    ///
    /// SINGLE-DRAINER CONTRACT: like [`Self::drain_property_changes`], this
    /// empties a shared queue -- today exactly one consumer per frame does
    /// the draining and routes to whoever asked (the properties panel's
    /// component-card host). If a second live-UI subscriber ever appears,
    /// this needs to grow per-consumer fanout before both can coexist;
    /// events discarded by the wrong drainer would strand the other
    /// consumer's cache stale until something else touched it.
    pub fn take_world_component_events(&self) -> Vec<pulsar_scenedb::ComponentChangeEvent> {
        self.store
            .write()
            .world_mut()
            .take_component_change_events()
    }

    /// Record that a specific property was written.  Called from every
    /// mutating method that touches a component property.
    fn record_property_change(&self, object_id: &str, class_name: &str, prop_name: &str) {
        let mut changes = self.property_changes.lock();
        // Soft cap: with relevance-gated rendering the set can go several
        // drains' worth of edits without being emptied (nothing forces a
        // component-card render for, say, transform-only edits). Dropping
        // the history past this point is always *safe* — an empty set just
        // makes the panel fall back to reading values it might have skipped.
        if changes.changed.len() >= MAX_PROPERTY_CHANGE_SET {
            *changes = PropertyChangeSet::default();
        }
        changes.changed.insert((
            object_id.to_string(),
            class_name.to_string(),
            prop_name.to_string(),
        ));
    }

    /// Record a structural change (add/remove/reorder/enable-disable) on a
    /// component.
    fn record_structural_change(&self, object_id: &str, class_name: &str) {
        let mut changes = self.property_changes.lock();
        if changes.changed.len() >= MAX_PROPERTY_CHANGE_SET {
            *changes = PropertyChangeSet::default();
        }
        changes
            .structural
            .insert((object_id.to_string(), class_name.to_string()));
        changes.components_added_or_removed = true;
    }

    /// Non-consuming check: has anything touching `object_id`'s components
    /// been written since the last [`Self::drain_property_changes`]?
    ///
    /// This is the properties panel's relevance gate — see
    /// `PropertiesPanelWrapper::sync_sections`. Peeking (rather than
    /// draining) keeps the drain contract where it is today: the section's
    /// own render remains the single consumer.
    pub fn has_property_changes_for(&self, object_id: &str) -> bool {
        self.property_changes.lock().touches_object(object_id)
    }

    /// Hydrate one canonical component directly into the entity World.
    ///
    /// This hydrates an explicit edit into the authoritative typed
    /// component. The caller decides whether the attachment record should
    /// retain its input JSON as a dormant compatibility value.
    fn hydrate_canonical_component(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        data: &Value,
    ) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(object_id) else {
            return false;
        };
        match pulsar_world_registry::hydrate_world_component_for_class(
            class_name,
            store.world_mut(),
            entity,
            data,
        ) {
            Ok(true) => {
                store.mark_dirty(
                    object_id,
                    ObjectDirtyFlags::PROPS | ObjectDirtyFlags::COMPONENTS,
                );
                true
            }
            Ok(false) => false,
            Err(error) => {
                tracing::error!(
                    "World hydration failed for {class_name} on '{object_id}': {error}"
                );
                false
            }
        }
    }

    /// Attach a component while keeping the public metadata-shaped API
    /// compatible. For registered classes, the first enabled instance is
    /// hydrated into World and the metadata record stores only attachment
    /// state; disabled/failed entries retain JSON for re-enable compatibility.
    fn attach_component_instance(
        &self,
        object_id: &EditorObjectId,
        mut component: ComponentInstance,
        record_change: bool,
    ) {
        let class_name = component.class_name.clone();
        if component.enabled
            && is_scenedb_authority_class(&class_name)
            && !self
                .component_store
                .get_components(object_id)
                .iter()
                .any(|existing| existing.enabled && existing.class_name == class_name)
            && self.hydrate_canonical_component(object_id, &class_name, &component.data)
        {
            component.data = attachment_data(&component.data);
        }
        self.component_store
            .add_component_instance(object_id, component);
        self.sync_registered_component_props_to_scene_db(object_id);
        if record_change {
            self.record_structural_change(object_id, &class_name);
        }
    }

    /// Lightweight query: return the list of class names attached to
    /// `object_id`, reading only from `component_store` (no JSON clone, no
    /// `to_json()` serialization).  This replaces `get_components()` in the
    /// properties-panel hot path where only the class name + order are needed
    /// to look up cached property metadata.
    pub fn get_component_class_names(&self, object_id: &EditorObjectId) -> Vec<String> {
        self.component_store
            .get_components(object_id)
            .into_iter()
            .map(|c| c.class_name)
            .collect()
    }

    /// Cheap component count for `object_id` — avoids the full
    /// `get_components()` clone + `to_json()` serialization.
    pub fn component_count(&self, object_id: &EditorObjectId) -> usize {
        self.component_store.get_components(object_id).len()
    }

    // ── Object CRUD ───────────────────────────────────────────────────────
    //
    // WorldSceneStore is the single source of truth. sync_scene() in the
    // renderer reconciles Helio state every frame — no immediate
    // write-through needed.

    /// Add an object. Returns the assigned `ObjectId`.
    ///
    /// Blueprint objects always receive a `ScriptComponent` in `component_store`
    /// pointing at their blueprint directory. `sync_registered_component_props_to_scene_db`
    /// rebuilds `__component_instances` from `component_store`, so the component
    /// must live there — setting it only in `props` would be immediately overwritten.
    pub fn add_object(&self, obj: SceneObjectData, parent: Option<ObjectId>) -> ObjectId {
        // Reject caller-supplied identity/parent errors before touching the
        // metadata projection. An add must never turn a duplicate or stale
        // reference into a different object while the caller keeps using the
        // original ID.
        {
            let store = self.store.read();
            if !obj.id.is_empty() && store.entity_for(&obj.id).is_some() {
                tracing::error!(id = %obj.id, "SceneDatabase rejected duplicate object ID");
                return String::new();
            }
            if let Some(parent_id) = parent.as_deref() {
                if store.entity_for(parent_id).is_none() {
                    tracing::error!(parent = %parent_id, "SceneDatabase rejected missing parent");
                    return String::new();
                }
            }
        }

        // v2 scene objects may carry component instances inline. Preserve
        // those instances in the metadata store before the normal hydration
        // pass; otherwise the empty metadata store overwrites the inline list
        // and World-registered components (notably StaticMeshComponent) never
        // reach the live World.
        let mut inline_components = obj
            .component_instances
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .map(|instances| {
                instances
                    .iter()
                    .filter_map(|instance| {
                        let object = instance.as_object()?;
                        Some(ComponentInstance {
                            class_name: object.get("class_name")?.as_str()?.to_string(),
                            enabled: object
                                .get("enabled")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(true),
                            data: object
                                .get("data")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // A few older scene files projected StaticMeshComponent's asset path
        // into `props` without emitting a component-instances entry. Treat
        // that as a load-time compatibility form so those scenes also get a
        // typed World component; current files still use the normal path.
        if inline_components.is_empty() {
            if let Some(mesh_asset) = obj
                .props
                .get("mesh_asset")
                .and_then(serde_json::Value::as_str)
                .filter(|path| !path.trim().is_empty())
            {
                inline_components.push(ComponentInstance {
                    class_name: "StaticMeshComponent".to_string(),
                    enabled: true,
                    data: static_mesh_component_json(mesh_asset),
                });
            }
        }
        let blueprint_script_path = if obj.object_type == ObjectType::Blueprint {
            Some(find_script_path(
                &obj.props,
                obj.component_instances.as_ref(),
            ))
        } else {
            None
        };

        let object_id = {
            let mut store = self.store.write();
            let parent_entity = parent.as_deref().and_then(|p| store.entity_for(p));
            let requested_id = if obj.id.is_empty() {
                None
            } else {
                Some(obj.id.clone())
            };
            let entity = match store.spawn(requested_id, obj.name.clone(), parent_entity) {
                Ok(entity) => entity,
                Err(WorldSceneStoreError::DuplicateId(dup)) => {
                    tracing::warn!(
                        "SceneDatabase::add_object: id '{dup}' already exists, auto-assigning a new one"
                    );
                    store
                        .spawn(None, obj.name.clone(), parent_entity)
                        .expect("auto-assigned stable id cannot collide")
                }
                Err(err) => {
                    tracing::warn!("SceneDatabase::add_object: {err}, auto-assigning a new id");
                    store
                        .spawn(None, obj.name.clone(), parent_entity)
                        .expect("auto-assigned stable id cannot collide")
                }
            };
            store.set_transform(
                entity,
                WorldTransform {
                    position: obj.transform.position,
                    rotation: obj.transform.rotation,
                    scale: obj.transform.scale,
                },
            );
            store.set_visibility(
                entity,
                WorldVisibility {
                    visible: obj.visible,
                    locked: obj.locked,
                },
            );
            store.set_object_type(entity, obj.object_type);
            let id = store.stable_id_of(entity).unwrap_or_default().to_string();
            store.update_render_props(&id, |render_props| {
                render_props.props = obj.props;
                render_props.component_instances = obj.component_instances;
            });
            id
        };

        for component in inline_components {
            self.attach_component_instance(&object_id, component, false);
        }

        if let Some(script_path) = blueprint_script_path {
            let already_has = self
                .component_store
                .get_components(&object_id)
                .iter()
                .any(|c| c.class_name == "ScriptComponent");

            if !already_has {
                self.attach_component_instance(
                    &object_id,
                    ComponentInstance {
                        class_name: "ScriptComponent".to_string(),
                        enabled: true,
                        data: serde_json::json!({ "script_asset": script_path }),
                    },
                    false,
                );
            }
        }

        self.sync_registered_component_props_to_scene_db(&object_id);
        object_id
    }

    /// Remove an object and all of its descendants. Returns `true` if found.
    pub fn remove_object(&self, id: &ObjectId) -> bool {
        let ids_to_clear = {
            let mut store = self.store.write();
            let Some(entity) = store.entity_for(id) else {
                return false;
            };
            let mut ids_to_clear = vec![id.clone()];
            Self::collect_descendant_ids(&store, entity, &mut ids_to_clear);
            store.despawn(entity);
            ids_to_clear
        };
        for object_id in ids_to_clear {
            self.component_store.clear_components(&object_id);
        }
        true
    }

    /// Write updated transform, name, visibility, and component data back to an existing object.
    pub fn update_object(&self, obj: SceneObjectData) -> bool {
        let id = obj.id.clone();
        {
            let mut store = self.store.write();
            let Some(entity) = store.entity_for(&id) else {
                return false;
            };
            store.set_transform(
                entity,
                WorldTransform {
                    position: obj.transform.position,
                    rotation: obj.transform.rotation,
                    scale: obj.transform.scale,
                },
            );
            store.set_name(entity, obj.name);
            store.set_visibility(
                entity,
                WorldVisibility {
                    visible: obj.visible,
                    locked: obj.locked,
                },
            );
            store.update_render_props(&id, |render_props| render_props.props = obj.props);
        }
        self.sync_registered_component_props_to_scene_db(&id);
        true
    }

    /// Update a single component's data by index.
    ///
    /// This is the correct entry point for component edits; World-authoritative
    /// classes are hydrated directly and legacy classes retain the existing
    /// metadata-backed behavior. Callers must not access `component_store`
    /// directly.
    pub fn update_component(
        &self,
        object_id: &ObjectId,
        component_index: usize,
        data: serde_json::Value,
    ) {
        let component = self
            .component_store
            .get_components(object_id)
            .get(component_index)
            .cloned();
        if let Some(component) = component.as_ref() {
            if component.enabled
                && is_scenedb_authority_class(&component.class_name)
                && self.live_typed_component_index(object_id, &component.class_name)
                    == Some(component_index)
                && self.hydrate_canonical_component(object_id, &component.class_name, &data)
            {
                self.component_store.update_component(
                    object_id,
                    component_index,
                    attachment_data(&component.data),
                );
                self.sync_registered_component_props_to_scene_db(object_id);
                self.record_structural_change(object_id, &component.class_name);
                return;
            }
        }

        let ok = self
            .component_store
            .update_component(object_id, component_index, data);
        if !ok {
            tracing::warn!(
                "[UPDATE_COMPONENT] component_store.update_component returned false for {object_id} idx={component_index}"
            );
        }
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    /// Update a single property inside a reflection-based component by class name and property name.
    ///
    /// Legacy flat-JSON path -- kept only for classes that were never
    /// migrated to `pulsar_world_registry` (no `ComponentRuntimeBehavior`,
    /// e.g. `LODComponent`/`MaterialOverrideComponent`), where JSON in
    /// `component_store` genuinely is the only representation that exists.
    /// **Do not call this for anything that supports
    /// [`Self::update_live_component_property`]** -- it writes `new_value`
    /// at the top level of the component's JSON, which is wrong for any
    /// `#[sub_props]`-nested field (silently dropped, or worse, overwrites a
    /// nested sub-struct with a bare scalar/array if the names happen to
    /// collide). See Pulsar-Native#561.
    ///
    /// TODO(Pulsar-Native#561): delete this method (and the flat-JSON
    /// fallback branches in `property_renderer.rs`/`material_section.rs`
    /// that call it) once every component -- including the props-only ones
    /// with no `ComponentRuntimeBehavior` today -- is `World`-registered.
    /// This is a shrinking legacy path for a handful of not-yet-migrated
    /// classes, not a permanent second way to edit components; the end
    /// state is that `SceneDatabase`/`World` is the only live component
    /// storage, full stop.
    pub fn update_component_property(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        prop_name: &str,
        new_value: serde_json::Value,
    ) {
        let components = self.get_components(object_id);
        if let Some((idx, comp)) = components
            .iter()
            .enumerate()
            .find(|(_, c)| c.class_name == class_name)
        {
            let mut data = comp.data.clone();
            if let Some(obj) = data.as_object_mut() {
                obj.insert(prop_name.to_string(), new_value);
            }
            self.update_component(object_id, idx, data);
            self.record_property_change(object_id, class_name, prop_name);
        }
    }

    /// Which instance of `class_name` on `object_id` is the **live-typed**
    /// one -- the single instance whose value actually lives in `World`.
    ///
    /// `World` stores one value per `(entity, ComponentId)`, so of N
    /// instances of the same class on one entity exactly ONE can be
    /// live-typed: the first ENABLED one (the same instance
    /// [`Self::sync_registered_component_props_to_scene_db`] hydrates).
    /// Every other instance exists only as its own JSON blob in
    /// `component_store`. `None` when the class isn't `World`-registered at all,
    /// or no enabled instance of it is attached.
    ///
    /// This is Pulsar-Native#519's identity anchor: the properties panel
    /// uses it to decide, per card, whether values come from `World` (live
    /// card, subscribable) or from that card's own metadata JSON.
    pub fn live_typed_component_index(
        &self,
        object_id: &ObjectId,
        class_name: &str,
    ) -> Option<usize> {
        if pulsar_world_registry::component_id_for_class(class_name).is_none() {
            return None;
        }
        self.get_components(object_id)
            .iter()
            .enumerate()
            .find(|(_, c)| c.class_name == class_name && c.enabled)
            .map(|(idx, _)| idx)
    }

    /// Edit a single property on ONE specific component instance,
    /// correctly handling `#[sub_props]` nesting (Pulsar-Native#561) and
    /// per-instance field values (Pulsar-Native#519).
    ///
    /// `component_index` addresses the exact instance in the object's
    /// component list -- the same identity `remove_component`/
    /// `set_component_enabled`/`reorder_component` already use. An object
    /// can carry several instances of the same class, each with independent
    /// field values; routing edits by class name alone (the pre-#519
    /// behavior) made every such edit land in whichever instance sorted
    /// first.
    ///
    /// Routing by instance:
    /// - **The live-typed instance** ([`Self::live_typed_component_index`]):
    ///   the setter closure runs straight against the `World`-resident
    ///   typed value -- no JSON anywhere on this path -- then the full new
    ///   shape is persisted back into that instance's `component_store` JSON so
    ///   the two never diverge (Pulsar-Native#561, Bug B).
    /// - **Every other instance** (duplicate duplicates, disabled
    ///   representatives, and classes with no `World` registration): the
    ///   value is serialized once and merged into THAT instance's own JSON
    ///   blob. There is no `World` presence to keep in sync; if the
    ///   instance ever becomes the live-typed one (an earlier duplicate is
    ///   removed or disabled), re-hydration reads exactly this JSON.
    ///
    /// `class_name`/`prop_name` come from [`pulsar_reflection::PropertyMetadata`]
    /// (the same reflection metadata the properties panel already reads to
    /// render the row). The setter closure is looked up fresh from a
    /// throwaway `REGISTRY.create_instance` -- that instance's own field
    /// values are discarded immediately; only its *type-bound*
    /// getter/setter closures are used.
    ///
    /// `Err(new_value)` -- handing the value straight back, since nothing
    /// was written -- when the index/class pair doesn't match the object's
    /// actual component list, or the class has no reflection metadata here
    /// at all (plugin-only classes; the command layer's legacy flat-JSON
    /// fallback covers those).
    pub fn update_live_component_property(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        component_index: usize,
        prop_name: &str,
        new_value: Box<dyn Any + Send>,
    ) -> Result<(), Box<dyn Any + Send>> {
        // The index IS the identity: a stale or mismatched one must never
        // land an edit into some OTHER instance's storage.
        let components = self.get_components(object_id);
        let Some(target) = components.get(component_index) else {
            return Err(new_value);
        };
        if target.class_name != class_name {
            tracing::warn!(
                "[LIVE_PROPERTY_EDIT] index {component_index} holds '{}' not '{class_name}' -- edit refused",
                target.class_name
            );
            return Err(new_value);
        }

        let Some(prop_meta) = pulsar_reflection::REGISTRY
            .create_instance(class_name)
            .and_then(|instance| {
                instance
                    .get_properties()
                    .into_iter()
                    .find(|p| p.name == prop_name)
            })
        else {
            tracing::warn!(
                "[LIVE_PROPERTY_EDIT] no reflected property '{prop_name}' on '{class_name}'"
            );
            return Err(new_value);
        };

        // Non-live instances: apply the edit through the real typed
        // machinery against a THROWAWAY World seeded from this instance's
        // own JSON, then persist the full result back to that same blob. A
        // flat `{prop_name: value}` merge here would be wrong for any
        // `#[sub_props]`-nested leaf (Pulsar-Native#561's corruption class:
        // bare scalar landing where a nested group lives), and duplicates
        // deserve the same nesting-correct write the live card gets --
        // that's the whole point of #519.
        let is_live =
            self.live_typed_component_index(object_id, class_name) == Some(component_index);
        if !is_live {
            let mut scratch = pulsar_scenedb::World::new();
            let scratch_entity = scratch.spawn();
            let hydrated = pulsar_world_registry::hydrate_world_component_for_class(
                class_name,
                &mut scratch,
                scratch_entity,
                &target.data,
            );
            if hydrated.is_err() {
                // This instance's stored JSON doesn't deserialize for its
                // own class -- refuse the edit rather than guess.
                return Err(new_value);
            }
            let Some(instance) = pulsar_world_registry::get_world_component_as_engine_class_mut(
                class_name,
                &mut scratch,
                scratch_entity,
            ) else {
                // Hydrate was a no-op: this class has no `World` bridge at
                // all (plugin-only). Hand the value back untouched so the
                // command layer's legacy flat-JSON fallback can take it.
                return Err(new_value);
            };
            (prop_meta.setter)(instance, new_value);
            let Ok(value_json) = instance.to_json() else {
                return Err(Box::new(()));
            };
            self.component_store
                .update_component(object_id, component_index, value_json);
            self.record_property_change(object_id, class_name, prop_name);
            return Ok(());
        }

        let setter = prop_meta.setter;

        // Scoped so the `store` write-guard is dropped before the
        // `component_store` persistence step below -- that step goes through
        // `self.get_components`, which takes its own `self.store.read()`;
        // `parking_lot::RwLock` isn't reentrant, so holding this write guard
        // across that call would deadlock.
        let persisted_json = {
            let mut store = self.store.write();
            let Some(entity) = store.entity_for(object_id) else {
                return Err(new_value);
            };
            let Some(instance) = pulsar_world_registry::get_world_component_as_engine_class_mut(
                class_name,
                store.world_mut(),
                entity,
            ) else {
                return Err(new_value);
            };
            (setter)(instance, new_value);
            // Record the property change for the properties panel's change set
            // so it can skip re-reading unchanged properties.
            self.record_property_change(object_id, class_name, prop_name);
            // This was the actual bug behind "the properties panel shows the
            // right value but the light in the scene never changes": mutating
            // the live World component directly (above) is correct and
            // sufficient for anything that reads World directly (this method's
            // own read-side counterpart, `read_live_component_property`) --
            // but the renderer's per-frame sync (`sync_scene`/`sync_scene_delta`
            // in HelioRenderer) is gated entirely on `WorldSceneStore`'s own
            // dirty-tracking/`render_revision` counters, which a raw
            // `get_world_component_as_engine_class_mut` write never touches.
            // Without this, the edit is genuinely live in `World` -- correctly
            // observable by direct reads -- but invisible to the mechanism that
            // decides whether to re-sync Helio's scene at all. `mark_dirty`
            // (`WorldSceneStore::publish` under the hood) is what actually
            // signals the render thread.
            // Capture the component's full current shape while `instance` is
            // still borrowed (must happen before `mark_dirty` below, which
            // needs its own `&mut store` -- `instance` borrows `store`
            // mutably via `world_mut()`, so the two borrows can't overlap).
            let json = instance.to_json().ok();
            store.mark_dirty(
                object_id,
                ObjectDirtyFlags::PROPS | ObjectDirtyFlags::COMPONENTS,
            );
            json
        };

        // Persist back into `component_store` for legacy and non-live instances
        // (Pulsar-Native#561, Bug B). A live migrated class is deliberately
        // excluded below: World is its authority, while metadata keeps only
        // the attachment/order/enabled record. Every other instance is still
        // persisted to the EXACT edited index, not "first with this class" --
        // with duplicates those are different compatibility blobs
        // (Pulsar-Native#519).
        if let Some(json) =
            persisted_json.filter(|_| !(is_live && is_scenedb_authority_class(class_name)))
        {
            self.component_store
                .update_component(object_id, component_index, json);
        }

        Ok(())
    }

    /// Read a single property straight off the **live `World`-resident
    /// component**, correctly handling `#[sub_props]` nesting -- no JSON
    /// involved (Pulsar-Native#561's read-side counterpart to
    /// [`Self::update_live_component_property`]).
    ///
    /// `None` under the same conditions as `update_live_component_property`
    /// (not `World`-registered, no live entity, or not hydrated yet);
    /// callers should fall back to the flat-JSON path or a `Default`
    /// instance in that case.
    pub fn read_live_component_property(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        prop_name: &str,
    ) -> Option<Box<dyn Any>> {
        let getter = pulsar_reflection::REGISTRY
            .create_instance(class_name)
            .and_then(|instance| {
                instance
                    .get_properties()
                    .into_iter()
                    .find(|p| p.name == prop_name)
                    .map(|p| p.getter)
            })?;

        let store = self.store.read();
        let entity = store.entity_for(object_id)?;
        let instance = pulsar_world_registry::get_world_component_as_engine_class(
            class_name,
            store.world(),
            entity,
        )?;
        Some((getter)(instance))
    }

    /// Batch-read every property of a component in one `store.read()`
    /// acquisition.  Returns `None` if the class isn't World-registered,
    /// the entity doesn't exist, or the component isn't hydrated.
    ///
    /// Takes a pre-built property metadata slice (from the cached metadata)
    /// so we don't need to call `create_instance` + `get_properties` again.
    pub fn read_component_properties_batch(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        properties: &[pulsar_reflection::PropertyMetadata],
    ) -> Option<Vec<Box<dyn Any>>> {
        let store = self.store.read();
        let entity = store.entity_for(object_id)?;
        let instance = pulsar_world_registry::get_world_component_as_engine_class(
            class_name,
            store.world(),
            entity,
        )?;
        Some(
            properties
                .iter()
                .map(|prop| (prop.getter)(instance))
                .collect(),
        )
    }

    /// Run a closure with the live World component reference held under a
    /// single `store.read()`.  The closure can call property getters
    /// directly against the component reference without acquiring the lock
    /// again.
    ///
    /// Returns `None` if the class isn't World-registered, the entity
    /// doesn't exist, or the component isn't hydrated.
    pub fn with_world_component<T>(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        f: impl FnOnce(&dyn pulsar_reflection::EngineClass) -> T,
    ) -> Option<T> {
        let store = self.store.read();
        let entity = store.entity_for(object_id)?;
        let instance = pulsar_world_registry::get_world_component_as_engine_class(
            class_name,
            store.world(),
            entity,
        )?;
        Some(f(instance))
    }

    /// Clear the entire scene.
    pub fn clear(&self) {
        // Take the complete indexed object list once. Clearing only roots
        // leaves orphaned scene entities behind when a caller has created an
        // inconsistent parent link; those stale IDs then poison subsequent
        // loads/adds. Despawn is recursive, so one pass over the snapshot is
        // sufficient and does not require a lock per object.
        let object_ids: Vec<ObjectId> = self
            .store
            .read()
            .to_snapshots()
            .into_iter()
            .map(|object| object.stable_id)
            .collect();
        if !object_ids.is_empty() {
            let mut store = self.store.write();
            for id in object_ids {
                if let Some(entity) = store.entity_for(&id) {
                    store.despawn(entity);
                }
            }
        }
        tracing::info!("Scene cleared – ready for new level");
    }

    // ── Queries ───────────────────────────────────────────────────────────

    /// All objects in depth-first order.
    pub fn get_all_objects(&self) -> Vec<SceneObjectData> {
        let store = self.store.read();
        let mut out = Vec::new();
        Self::collect_dfs(&store, None, &mut out);
        drop(store);
        for obj in &mut out {
            self.merge_component_props(&obj.id, &mut obj.props);
        }
        out
    }

    /// Root-level objects (no parent).
    pub fn get_root_objects(&self) -> Vec<SceneObjectData> {
        let store = self.store.read();
        store
            .children_of(None)
            .iter()
            .map(|&e| Self::entity_to_scene_object_data(&store, e))
            .collect()
    }

    /// Return the hierarchy projection from one WorldSceneStore read.
    /// Keeping roots and objects together prevents the UI from observing two
    /// different revisions during a background mutation.
    pub fn get_hierarchy_snapshot(&self) -> (Vec<SceneObjectData>, Vec<ObjectId>) {
        let store = self.store.read();
        let mut objects = Vec::new();
        Self::collect_dfs(&store, None, &mut objects);
        let root_ids = store
            .children_of(None)
            .iter()
            .filter_map(|&entity| store.stable_id_of(entity).map(str::to_string))
            .collect();
        drop(store);
        for object in &mut objects {
            self.merge_component_props(&object.id, &mut object.props);
        }
        (objects, root_ids)
    }

    /// Number of root-level objects, without building their data.
    pub fn root_count(&self) -> usize {
        self.store.read().children_of(None).len()
    }

    /// Monotonic counter that advances whenever the scene content may have
    /// changed — UI-thread commands, AI tools, AND the render thread's own
    /// direct writes (gizmo-drag release, click-to-select all go through
    /// store mutators that `publish()`).
    ///
    /// The raw counter comes from the current `WorldSceneStore`; this folds
    /// it through [`RevisionTracker`] so the value stays monotonic across
    /// `restore_history_snapshot`'s wholesale store swap (undo/redo), which
    /// resets the raw counter to a fresh deterministic value. Selection is
    /// NOT covered (`select_object` deliberately doesn't publish) — poll it
    /// alongside.
    ///
    /// This is what panel frame pumps compare per tick: one read-lock
    /// acquisition + a couple of atomic ops, vs. re-deriving "did anything I
    /// render change" from content snapshots.
    pub fn store_revision(&self) -> u64 {
        let raw = self.store.read().render_revision();
        let epoch = self
            .revision_tracker
            .epoch
            .load(std::sync::atomic::Ordering::Relaxed);
        self.revision_tracker.fold(epoch, raw)
    }

    // ── Targeted component reads ──────────────────────────────────────────
    //
    // The bound-field editors refresh these values on every scene revision
    // bump under the current selection (gizmo drags, AI edits, typing). Each
    // of these used to go through `get_object`, whose cost is O(scene data):
    // a full `SceneObjectData` clone including the props map AND a
    // `merge_component_props` pass cloning every component instance plus
    // running reflection projection over it — twelve times per bump for the
    // transform + header fields alone. These accessors read exactly one
    // component under one short lock, allocate almost nothing, and never
    // touch component_store.

    /// Just the object's transform — no props merge, no path computation.
    pub fn get_object_transform(&self, id: &ObjectId) -> Option<Transform> {
        let store = self.store.read();
        let entity = store.entity_for(id)?;
        let t = store.transform(entity)?;
        Some(Transform {
            position: t.position,
            rotation: t.rotation,
            scale: t.scale,
        })
    }

    /// Just the object's name.
    pub fn get_object_name(&self, id: &ObjectId) -> Option<String> {
        let store = self.store.read();
        let entity = store.entity_for(id)?;
        Some(store.name(entity)?.to_string())
    }

    /// Just the object's `(visible, locked)` flags.
    pub fn get_object_visibility(&self, id: &ObjectId) -> Option<(bool, bool)> {
        let store = self.store.read();
        let entity = store.entity_for(id)?;
        let v = store.visibility(entity)?;
        Some((v.visible, v.locked))
    }

    /// Single object by ID, `None` if not found.
    pub fn get_object(&self, id: &ObjectId) -> Option<SceneObjectData> {
        let mut data = {
            let store = self.store.read();
            let entity = store.entity_for(id)?;
            Self::entity_to_scene_object_data(&store, entity)
        };
        self.merge_component_props(id, &mut data.props);
        Some(data)
    }

    /// Direct children of `id`.
    pub fn get_children(&self, id: &ObjectId) -> Vec<ObjectId> {
        let store = self.store.read();
        let Some(entity) = store.entity_for(id) else {
            return Vec::new();
        };
        store
            .children_of(Some(entity))
            .iter()
            .filter_map(|&e| store.stable_id_of(e).map(str::to_string))
            .collect()
    }

    // ── Selection ─────────────────────────────────────────────────────────

    pub fn select_object(&self, id: Option<ObjectId>) {
        self.store.write().select_object(id);
    }

    pub fn get_selected_object_id(&self) -> Option<ObjectId> {
        self.store.read().get_selected_id()
    }

    pub fn get_selected_object(&self) -> Option<SceneObjectData> {
        let store = self.store.read();
        let entity = store.get_selected_entity()?;
        Some(Self::entity_to_scene_object_data(&store, entity))
    }

    // ── Properties ────────────────────────────────────────────────────────

    pub fn set_name(&self, id: &ObjectId, name: String) -> bool {
        let mut store = self.store.write();
        match store.entity_for(id) {
            Some(entity) => store.set_name(entity, name),
            None => false,
        }
    }

    pub fn set_visible(&self, id: &ObjectId, visible: bool) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let mut visibility = store.visibility(entity).unwrap_or_default();
        visibility.visible = visible;
        store.set_visibility(entity, visibility)
    }

    pub fn set_locked(&self, id: &ObjectId, locked: bool) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let mut visibility = store.visibility(entity).unwrap_or_default();
        visibility.locked = locked;
        store.set_visibility(entity, visibility)
    }

    /// Narrow transform update -- writes only `WorldSceneStore`'s own
    /// transform, no full-object `SceneObjectData` round trip. Unlike
    /// `update_object(SceneObjectData)`, this does NOT call
    /// `sync_registered_component_props_to_scene_db` -- a transform never
    /// needs a component re-hydration, so the old whole-object path (used
    /// by `SceneCommand::SetTransform`'s handler before Pulsar-Native#561's
    /// properties-panel rewrite) was re-serializing/re-hydrating every
    /// `World`-registered component on the object on every keystroke of a
    /// position/rotation/scale field, for no reason. `None` fields are left
    /// unchanged; returns `false` if nothing actually changed or the object
    /// doesn't exist.
    pub fn set_transform(
        &self,
        id: &ObjectId,
        position: Option<[f32; 3]>,
        rotation: Option<[f32; 3]>,
        scale: Option<[f32; 3]>,
    ) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let mut transform = store.transform(entity).unwrap_or_default();
        let mut changed = false;
        if let Some(p) = position {
            if transform.position != p {
                transform.position = p;
                changed = true;
            }
        }
        if let Some(r) = rotation {
            if transform.rotation != r {
                transform.rotation = r;
                changed = true;
            }
        }
        if let Some(s) = scale {
            if transform.scale != s {
                transform.scale = s;
                changed = true;
            }
        }
        if !changed {
            return false;
        }
        store.set_transform(entity, transform)
    }

    /// Re-parent an object (cycle-safe).
    pub fn reparent_object(&self, id: &ObjectId, new_parent: Option<ObjectId>) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let new_parent_entity = match new_parent {
            Some(ref parent_id) => match store.entity_for(parent_id) {
                Some(e) => Some(e),
                None => return false,
            },
            None => None,
        };
        store.reparent(entity, new_parent_entity).is_ok()
    }

    /// Reorder two sibling objects by swapping their positions.
    ///
    /// Both objects must have the same parent. Returns false if they don't
    /// share a parent or either id is unknown.
    pub fn reorder_object_siblings(&self, object_id: &ObjectId, target_id: &ObjectId) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(object_id) else {
            return false;
        };
        let Some(target) = store.entity_for(target_id) else {
            return false;
        };
        store.reorder_sibling(entity, target)
    }

    // ── Ordering ──────────────────────────────────────────────────────────

    /// Move an object one step earlier among its siblings (swaps with the
    /// preceding sibling). No-op (returns without effect) if already first.
    pub fn move_object_up(&self, id: &str) {
        let mut store = self.store.write();
        if let Some(entity) = store.entity_for(id) {
            store.move_sibling_up(entity);
        }
    }

    /// Move an object one step later among its siblings (swaps with the
    /// following sibling). No-op if already last.
    pub fn move_object_down(&self, id: &str) {
        let mut store = self.store.write();
        if let Some(entity) = store.entity_for(id) {
            store.move_sibling_down(entity);
        }
    }

    // ── Duplication ────────────────────────────────────────────────────────

    /// Shallow-duplicate an object (children are not copied). Returns the new ID.
    pub fn duplicate_object(&self, id: &str) -> Option<ObjectId> {
        let source_id = id.to_string();
        let source_components = self.get_components(&source_id);
        let mut obj = self.get_object(&source_id)?;
        obj.id = String::new(); // force auto-assign
        obj.name = format!("{} (Copy)", obj.name);
        obj.children = vec![];
        let parent = obj.parent.clone();
        let new_id = self.add_object(obj, parent);

        self.component_store.clear_components(&new_id);
        for component in source_components {
            self.attach_component_instance(&new_id, component, false);
        }
        self.sync_registered_component_props_to_scene_db(&new_id);

        Some(new_id)
    }

    // ── Folder helper ──────────────────────────────────────────────────────

    pub fn add_folder(&self, name: &str, parent: Option<ObjectId>) -> ObjectId {
        let obj = SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type: ObjectType::Folder,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: parent.clone(),
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        };
        self.add_object(obj, parent)
    }

    // ── Reflection component system ────────────────────────────────────────

    pub fn add_component(
        &self,
        object_id: &EditorObjectId,
        class_name: String,
        data: serde_json::Value,
    ) {
        self.attach_component_instance(
            object_id,
            ComponentInstance {
                class_name,
                enabled: true,
                data,
            },
            true,
        );
    }

    /// Add a fully specified component instance.
    pub fn add_component_instance(&self, object_id: &EditorObjectId, component: ComponentInstance) {
        self.attach_component_instance(object_id, component, true);
    }

    pub fn remove_component(&self, object_id: &EditorObjectId, component_index: usize) {
        // Preserve the old representative before changing instance order.
        let mut components = self.get_components(object_id);
        if component_index >= components.len() {
            return;
        }
        let class_name = components.remove(component_index).class_name;
        remap_component_parents(&mut components, |parent| {
            if parent == component_index {
                None
            } else {
                Some(if parent > component_index {
                    parent - 1
                } else {
                    parent
                })
            }
        });
        self.component_store
            .replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
        self.record_structural_change(object_id, &class_name);
    }

    /// Enable or disable a component by index.
    pub fn set_component_enabled(
        &self,
        object_id: &EditorObjectId,
        component_index: usize,
        enabled: bool,
    ) -> bool {
        let class_name = self
            .component_store
            .get_components(object_id)
            .get(component_index)
            .map(|c| c.class_name.clone());
        let mut components = self.get_components(object_id);
        let Some(component) = components.get_mut(component_index) else {
            return false;
        };
        if component.enabled == enabled {
            return true;
        }
        if is_scenedb_authority_class(&component.class_name) && component.enabled {
            if let Some(live) = self.get_components(object_id).get(component_index) {
                component.data = live.data.clone();
            }
        }
        component.enabled = enabled;
        self.component_store
            .replace_components(object_id, components);
        let changed = true;
        if changed {
            self.sync_registered_component_props_to_scene_db(object_id);
            if let Some(name) = class_name {
                self.record_structural_change(object_id, &name);
            }
        }
        changed
    }

    /// Duplicate a component at the same object, inserting the copy directly after the source.
    pub fn duplicate_component(
        &self,
        object_id: &EditorObjectId,
        component_index: usize,
    ) -> Option<usize> {
        let mut components = self.get_components(object_id);
        if component_index >= components.len() {
            return None;
        }

        let insert_index = component_index.saturating_add(1);
        let component = components.get(component_index)?.clone();
        let class_name = component.class_name.clone();
        components.insert(insert_index, component);
        remap_component_parents(&mut components, |parent| {
            Some(if parent >= insert_index {
                parent + 1
            } else {
                parent
            })
        });
        self.component_store
            .replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
        self.record_structural_change(object_id, &class_name);
        Some(insert_index)
    }

    pub fn reorder_component(
        &self,
        object_id: &EditorObjectId,
        from_index: usize,
        to_index: usize,
    ) {
        let mut components = self.get_components(object_id);
        if from_index >= components.len() || to_index >= components.len() || from_index == to_index
        {
            return;
        }

        let component = components.remove(from_index);
        let class_name = component.class_name.clone();
        components.insert(to_index, component);
        remap_component_parents(&mut components, |parent| {
            Some(if parent == from_index {
                to_index
            } else if from_index < to_index && parent > from_index && parent <= to_index {
                parent - 1
            } else if to_index < from_index && parent >= to_index && parent < from_index {
                parent + 1
            } else {
                parent
            })
        });
        self.component_store
            .replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
        self.record_structural_change(object_id, &class_name);
    }

    /// Every component instance attached to `object_id`, with `data`
    /// resolved *live* off `World` for any class that has a live value
    /// there (Pulsar-Native#561) -- `component_store`'s stored JSON is no
    /// longer trusted for those classes' current field values, only for
    /// which components are attached, their order, and their `enabled`
    /// flag. This is the one choke point both the properties panel
    /// (`attached`) and save-to-disk (`save_to_file_with_editor_camera`)
    /// go through, so fixing it here is enough to make `World` the actual
    /// source of truth for both, without either one needing its own sync
    /// step: `update_live_component_property` writes straight to `World`
    /// and stops there (no component_store write-back at all), and this method
    /// is what makes that edit visible everywhere else that reads
    /// component data, including what eventually gets serialized to disk.
    /// Metadata-only view of the object's attached components: class names,
    /// order, enabled flags and stored JSON, with NO live-World overlay.
    ///
    /// [`Self::get_components`] serializes every World-registered component
    /// (`to_json()`) to overlay fresh values — the right thing for save-to-
    /// disk, but pure waste for callers that only need structure (the
    /// component tree's parent indices) or that read live values themselves
    /// (the property cards batch-read straight from World). Per-render cost
    /// of the properties panel used to scale with C × serialization for
    /// exactly this reason.
    pub fn get_components_metadata(&self, object_id: &EditorObjectId) -> Vec<ComponentInstance> {
        self.component_store.get_components(object_id)
    }

    pub fn get_components(&self, object_id: &EditorObjectId) -> Vec<ComponentInstance> {
        let store = self.store.read();
        Self::components_from_store(&store, object_id)
    }

    fn components_from_store(store: &WorldSceneStore, object_id: &str) -> Vec<ComponentInstance> {
        let Some(entity) = store.entity_for(object_id) else {
            return Vec::new();
        };
        let mut components = store
            .world()
            .get::<engine_backend::scene::ComponentAttachments>(entity)
            .map(|attachments| attachments.0.clone())
            .unwrap_or_default();
        // Overlay ONLY onto each class's one live-typed instance
        // (Pulsar-Native#519): `World` holds a single typed value per
        // `(entity, ComponentId)` -- the first enabled instance -- so
        // stamping it onto EVERY instance of the class used to clobber the
        // other duplicates' own stored field values on every read. A
        // duplicate's `data` is its own blob; if it becomes the live-typed
        // one later, re-hydration adopts exactly that blob.
        let mut live_index_of_class: HashMap<String, usize> = HashMap::new();
        for (idx, component) in components.iter().enumerate() {
            if !component.enabled {
                continue;
            }
            if pulsar_world_registry::component_id_for_class(&component.class_name).is_some() {
                live_index_of_class
                    .entry(component.class_name.clone())
                    .or_insert(idx);
            }
        }
        for (idx, component) in components.iter_mut().enumerate() {
            if live_index_of_class.get(component.class_name.as_str()) != Some(&idx) {
                continue;
            }
            if let Some(live) = pulsar_world_registry::get_world_component_as_engine_class(
                component.class_name.as_str(),
                store.world(),
                entity,
            ) {
                match live.to_json() {
                    Ok(json) => component.data = overlay_live_data(&component.data, json),
                    Err(error) => tracing::warn!(
                        "[GET_COMPONENTS] '{}' on '{object_id}' has a live World value but \
                         failed to serialize it, keeping the last-known-good stored copy: {error}",
                        component.class_name
                    ),
                }
            }
        }
        components
    }

    /// Check if a component is a descendant of another component
    fn is_component_descendant(
        components: &[ComponentInstance],
        potential_descendant: usize,
        potential_ancestor: usize,
    ) -> bool {
        let mut current = potential_descendant;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current) {
                return true;
            }
            if current == potential_ancestor {
                return true;
            }
            // Get parent of current component
            let parent = components[current]
                .data
                .get("__parent_index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);

            match parent {
                Some(parent_idx) if parent_idx < components.len() => {
                    current = parent_idx;
                }
                _ => return false, // Reached root or invalid parent
            }
        }
    }

    /// Set the parent of a component (for hierarchical organization)
    pub fn set_component_parent(
        &self,
        object_id: &EditorObjectId,
        component_index: usize,
        parent_index: Option<usize>,
    ) {
        let mut components = self.get_components(object_id);
        if component_index >= components.len() {
            return;
        }

        // Prevent cycles: a component cannot be a parent of itself or its descendants
        if let Some(parent_idx) = parent_index {
            if parent_idx == component_index {
                return; // Can't be parent of itself
            }
            if parent_idx >= components.len() {
                return; // Invalid parent index
            }
            // Check if the target parent is actually a descendant of this component
            if Self::is_component_descendant(&components, parent_idx, component_index) {
                return; // Would create a cycle
            }
        }

        let component = &mut components[component_index];
        let mut data = component.data.as_object().cloned().unwrap_or_default();

        if let Some(parent_idx) = parent_index {
            data.insert("__parent_index".to_string(), serde_json::json!(parent_idx));
        } else {
            data.remove("__parent_index");
        }

        component.data = serde_json::Value::Object(data);
        self.component_store
            .replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    // ── Persistence ────────────────────────────────────────────────────────

    /// Serialize the scene to a JSON level file.
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        self.save_to_file_with_editor_camera(path, None, None)
    }

    /// Serialize the scene to a JSON level file, optionally persisting editor
    /// camera state and any authored voxel terrain.
    ///
    /// `terrain` is the level editor's terrain edit seam. Voxel data does not
    /// live in the scene database, so it cannot ride along in the `LevelFile`
    /// the way objects and components do; it is flushed to a sidecar beside
    /// the level instead (see [`crate::level_editor::core::terrain_sidecar`]).
    /// Passing `None` — as headless and test callers do — writes the level
    /// exactly as before.
    pub fn save_to_file_with_editor_camera<P: AsRef<Path>>(
        &self,
        path: P,
        editor_camera: Option<LevelEditorCameraState>,
        terrain: Option<&engine_backend::services::terrain_edit::TerrainEditApi>,
    ) -> Result<(), String> {
        if let Some(parent_dir) = path.as_ref().parent() {
            virtual_fs::create_dir_all(parent_dir)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        let objects = self.get_all_objects();
        let components = objects
            .iter()
            .map(|obj| (obj.id.clone(), self.get_components(&obj.id)))
            .collect::<HashMap<_, _>>();
        let now = chrono::Utc::now().to_rfc3339();
        // Read the existing file once: its editor camera is preserved when
        // no fresh camera state was supplied, and its #650 blueprint-binding
        // section always rides along (the editor cannot author it yet, but a
        // re-save must never destroy it).
        let existing_file = virtual_fs::read_file(path.as_ref())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|json: String| serde_json::from_str::<LevelFile>(&json).ok());
        let preserved_editor = if editor_camera.is_none() {
            existing_file.as_ref().and_then(|file| file.editor.clone())
        } else {
            None
        };
        let preserved_bindings = existing_file
            .map(|file| file.blueprint_bindings)
            .unwrap_or_default();
        let level_file = LevelFile {
            version: "2.1".into(),
            objects,
            components,
            blueprint_bindings: preserved_bindings,
            metadata: LevelMetadata {
                created: now.clone(),
                modified: now,
                editor_version: env!("CARGO_PKG_VERSION").into(),
            },
            editor: editor_camera
                .map(|camera| LevelEditorFileState {
                    camera: Some(camera),
                })
                .or(preserved_editor),
        };
        let json = serde_json::to_string_pretty(&level_file)
            .map_err(|e| format!("Failed to serialize: {e}"))?;
        virtual_fs::write_file(path.as_ref(), json.as_bytes())
            .map_err(|e| format!("Failed to write file: {e}"))?;

        // Flush voxel terrain after the level itself is on disk: a terrain
        // sidecar without its level is meaningless, so the level is the
        // thing that must land first.
        if let Some(terrain) = terrain {
            crate::level_editor::core::terrain_sidecar::save(path.as_ref(), terrain)?;
        }

        tracing::info!("Scene saved to: {}", path.as_ref().display());
        Ok(())
    }

    /// Load a scene from a JSON level file (replaces the current scene).
    pub fn load_from_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        self.load_from_file_with_editor_camera(path).map(|_| ())
    }

    /// Load a scene from a JSON level file and return any persisted editor camera state.
    pub fn load_from_file_with_editor_camera<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<Option<LevelEditorCameraState>, String> {
        let bytes = virtual_fs::read_file(path.as_ref())
            .map_err(|e| format!("Failed to read file: {e}"))?;
        let json = String::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {e}"))?;
        let level_file: LevelFile =
            serde_json::from_str(&json).map_err(|e| format!("Failed to parse JSON: {e}"))?;
        if !level_file.version.starts_with("2.") && !level_file.version.starts_with("1.") {
            return Err(format!(
                "Unsupported scene version: {}. Expected 1.x or 2.x",
                level_file.version
            ));
        }
        self.clear();
        // Objects are stored in DFS order so parents are always inserted first.
        let has_persisted_components = !level_file.components.is_empty();
        for obj in level_file.objects {
            let parent = obj.parent.clone();
            self.add_object(obj, parent);
        }

        // When present, persisted components are authoritative and replace defaults.
        if has_persisted_components {
            for (object_id, components) in level_file.components {
                while !self.get_components(&object_id).is_empty() {
                    self.remove_component(&object_id, 0);
                }

                for component in components {
                    self.add_component_instance(&object_id, component);
                }
            }
        }

        tracing::info!(
            "Scene loaded from: {} (version: {})",
            path.as_ref().display(),
            level_file.version
        );
        Ok(level_file.editor.and_then(|editor| editor.camera))
    }

    // ── Internal conversion helpers ──────────────────────────────────────

    /// Build a `SceneObjectData` for `entity` directly off `WorldSceneStore` --
    /// transform/name/visibility/object_type/render_props plus the derived
    /// `parent`/`children`/`scene_path` fields. Does NOT merge live
    /// `component_store` component props on top (see [`Self::merge_component_props`]
    /// -- callers that need that call it separately afterward, matching the
    /// pre-B1 code's exact read paths: `get_object`/`get_all_objects` merge,
    /// `get_root_objects`/`get_selected_object` deliberately don't).
    fn entity_to_scene_object_data(store: &WorldSceneStore, entity: Entity) -> SceneObjectData {
        let stable_id = store.stable_id_of(entity).unwrap_or_default().to_string();
        let transform = store.transform(entity).unwrap_or_default();
        let visibility = store.visibility(entity).unwrap_or_default();
        let render_props = store.render_props(&stable_id).unwrap_or_default();
        let parent = store
            .parent_of(entity)
            .and_then(|p| store.stable_id_of(p))
            .map(str::to_string);
        let children = store
            .children_of(Some(entity))
            .iter()
            .filter_map(|&child| store.stable_id_of(child).map(str::to_string))
            .collect();

        SceneObjectData {
            id: stable_id,
            name: store.name(entity).unwrap_or_default().to_string(),
            object_type: store.object_type(entity).unwrap_or(ObjectType::Empty),
            transform: Transform {
                position: transform.position,
                rotation: transform.rotation,
                scale: transform.scale,
            },
            visible: visibility.visible,
            locked: visibility.locked,
            parent,
            children,
            scene_path: Self::compute_scene_path(store, entity),
            props: render_props.props,
            component_instances: render_props.component_instances,
        }
    }

    /// Name-joined path from the root to `entity`, matching the pre-B1
    /// `SceneDb::update_subtree_path` format exactly (`"Parent/Child"`,
    /// recomputed on every read rather than cached -- see
    /// `WorldSceneStore::ObjectSnapshot`'s doc for why it isn't stored data).
    fn compute_scene_path(store: &WorldSceneStore, entity: Entity) -> String {
        let mut parts = vec![store.name(entity).unwrap_or_default().to_string()];
        let mut current = store.parent_of(entity);
        while let Some(parent) = current {
            parts.push(store.name(parent).unwrap_or_default().to_string());
            current = store.parent_of(parent);
        }
        parts.reverse();
        parts.join("/")
    }

    fn collect_dfs(
        store: &WorldSceneStore,
        parent: Option<Entity>,
        out: &mut Vec<SceneObjectData>,
    ) {
        for &entity in store.children_of(parent) {
            out.push(Self::entity_to_scene_object_data(store, entity));
            Self::collect_dfs(store, Some(entity), out);
        }
    }

    fn merge_component_props(&self, object_id: &str, props: &mut HashMap<String, Value>) {
        let components = self.get_components(&object_id.to_string());
        for component in components.into_iter().filter(|component| component.enabled) {
            if apply_scene_props_for_class(&component.class_name, props, Some(&component.data)) {
                continue;
            }

            if let Value::Object(map) = component.data {
                for (k, v) in map {
                    props.insert(k, v);
                }
            }
        }
    }

    fn sync_registered_component_props_to_scene_db(&self, object_id: &str) {
        let mut components = self.component_store.get_components(object_id);
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(object_id) else {
            return;
        };
        for class_name in pulsar_world_registry::registered_world_component_classes() {
            let component = components
                .iter_mut()
                .find(|component| component.enabled && component.class_name == class_name);
            if let Some(component) = component {
                // Null is the attachment marker for a live typed value. Only
                // explicit edits or newly promoted instances carry input JSON.
                if component.data != attachment_data(&component.data) {
                    if let Err(error) = pulsar_world_registry::hydrate_world_component_for_class(
                        class_name,
                        store.world_mut(),
                        entity,
                        &component.data,
                    ) {
                        tracing::error!(
                            "World hydration failed for {class_name} on '{object_id}': {error}"
                        );
                        continue;
                    }
                }
                if pulsar_world_registry::get_world_component_as_engine_class(
                    class_name,
                    store.world(),
                    entity,
                )
                .is_some()
                {
                    component.data = attachment_data(&component.data);
                }
            } else {
                pulsar_world_registry::remove_world_component_for_class(
                    class_name,
                    store.world_mut(),
                    entity,
                );
            }
        }
        store.world_mut().insert(
            entity,
            engine_backend::scene::ComponentAttachments(components.clone()),
        );
        // Legacy props are a disposable serialization projection, never the
        // input to a typed component during an unrelated object edit.
        let mut projected_classes = HashSet::new();
        for component in &mut components {
            if component.enabled && projected_classes.insert(component.class_name.clone()) {
                if let Some(live) = pulsar_world_registry::get_world_component_as_engine_class(
                    &component.class_name,
                    store.world(),
                    entity,
                ) {
                    if let Ok(data) = live.to_json() {
                        component.data = overlay_live_data(&component.data, data);
                    }
                }
            }
        }
        store.update_render_props(object_id, |render_props| {
            for class_name in registered_scene_props_classes() {
                let data = components
                    .iter()
                    .find(|c| c.class_name == class_name && c.enabled)
                    .map(|c| &c.data);
                apply_scene_props_for_class(class_name, &mut render_props.props, data);
            }
            render_props.component_instances = Some(Value::Array(components.iter().enumerate()
                .filter(|(_, component)| component.enabled)
                .map(|(index, component)| serde_json::json!({
                    "index": index, "class_name": component.class_name, "data": component.data
                })).collect()));
        });
    }
    fn collect_descendant_ids(store: &WorldSceneStore, entity: Entity, out: &mut Vec<ObjectId>) {
        for &child in store.children_of(Some(entity)) {
            if let Some(id) = store.stable_id_of(child) {
                out.push(id.to_string());
            }
            Self::collect_descendant_ids(store, child, out);
        }
    }

    // ── Undo/redo (Pulsar-Native#554) ────────────────────────────────────
    //
    // Deliberately NOT built on `pulsar_scenedb::replication::Snapshot`
    // (`capture_full`/`restore_to_world`): that machinery is shaped for
    // network replication -- every component needs a registered
    // `ReplicationRegistry` schema with per-field encode/decode `FieldOps`,
    // which is real setup cost and a poor fit for `RenderProps`' free-form
    // JSON (`HashMap<String, serde_json::Value>`, `Option<Value>`) -- not
    // impossible, but a whole schema-registration subsystem to stand up for
    // something `WorldSceneStore`'s own bridge already does. `to_snapshots`/
    // `load_from_snapshots` already capture and round-trip everything a
    // scene needs (proven by `world_store.rs`'s own tests), so undo/redo
    // reuses that directly instead.

    /// Capture a full, restorable snapshot of the current scene --
    /// `WorldSceneStore`'s object/transform/hierarchy/render-props state
    /// plus every object's reflection component data from `component_store`
    /// (the two are captured together so a restore can't reintroduce one
    /// half stale relative to the other). Treat the result as opaque; pass
    /// it back to [`Self::restore_history_snapshot`] only.
    pub fn capture_history_snapshot(&self) -> SceneHistorySnapshot {
        let store = self.store.read();
        let objects = store.to_snapshots();
        let components = objects
            .iter()
            .map(|obj| {
                (
                    obj.stable_id.clone(),
                    Self::components_from_store(&store, &obj.stable_id),
                )
            })
            .filter(|(_, components)| !components.is_empty())
            .collect();
        SceneHistorySnapshot {
            objects,
            components,
        }
    }

    /// Restore a previously captured snapshot, replacing the current scene
    /// entirely (`WorldSceneStore` is swapped for a fresh one built from the
    /// snapshot; `component_store` is cleared and repopulated). Entity identity
    /// is NOT preserved across a restore -- nothing outside `WorldSceneStore`
    /// holds a raw `Entity` across calls (every `SceneDatabase` method
    /// resolves `entity_for` fresh), so this is safe. Selection is cleared
    /// (the fresh store has no `selected` entity) -- not preserving it is a
    /// deliberate v1 simplification, not an oversight.
    ///
    /// Returns `Err` (leaving the live scene untouched) only if `snapshot`
    /// itself is malformed -- a forward parent reference, which shouldn't
    /// happen for a snapshot this type itself produced, but is surfaced
    /// rather than silently no-op'd or panicking, since restoring a
    /// generation-old snapshot after intervening schema changes is exactly
    /// the kind of thing that's cheap to guard here and expensive to debug
    /// if it silently corrupted the scene instead.
    pub fn restore_history_snapshot(&self, snapshot: &SceneHistorySnapshot) -> Result<(), String> {
        let mut new_store =
            WorldSceneStore::load_from_snapshots(&snapshot.objects).map_err(|e| e.to_string())?;
        {
            let mut old_store = self.store.write();
            if let Some(mirror) = old_store.world().gpu_mirror().cloned() {
                // Helio retains this handle. Remove old GPU rows before
                // reusing entity indices, then keep that same mirror attached.
                let old_entities: Vec<_> = old_store
                    .world()
                    .query::<&engine_backend::scene::StableId>()
                    .map(|(entity, _)| entity)
                    .collect();
                for entity in old_entities {
                    old_store.despawn(entity);
                }
                new_store.world_mut().attach_gpu_mirror(mirror);
                let transforms: Vec<_> = new_store
                    .world()
                    .query::<&WorldTransform>()
                    .map(|(entity, transform)| (entity, *transform))
                    .collect();
                for (entity, transform) in transforms {
                    new_store.world_mut().insert(entity, transform);
                }
            }
            *old_store = new_store;
        }
        // The swapped-in store's raw revision counter is unrelated to the
        // old one's (it restarts at a deterministic value) — tell the
        // monotonicizer so `store_revision` keeps advancing across undo/redo.
        self.revision_tracker.note_swap();
        for (object_id, components) in &snapshot.components {
            for component in components {
                self.attach_component_instance(object_id, component.clone(), false);
            }
        }

        Ok(())
    }
}

/// Opaque capture produced by [`SceneDatabase::capture_history_snapshot`],
/// consumed by [`SceneDatabase::restore_history_snapshot`]. See that pair's
/// docs for what it carries and why.
#[derive(Clone, Debug)]
pub struct SceneHistorySnapshot {
    objects: Vec<engine_backend::scene::ObjectSnapshot>,
    components: HashMap<ObjectId, Vec<ComponentInstance>>,
}

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

#[cfg(test)]
mod history_snapshot_tests {
    use super::*;

    fn object(name: &str, object_type: ObjectType) -> SceneObjectData {
        SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        }
    }

    #[test]
    fn capture_and_restore_round_trips_an_empty_scene() {
        let db = SceneDatabase::new();
        let snapshot = db.capture_history_snapshot();
        db.add_folder("Should be undone", None);
        assert_eq!(db.get_all_objects().len(), 1);

        db.restore_history_snapshot(&snapshot).unwrap();

        assert!(db.get_all_objects().is_empty());
    }

    #[test]
    fn restore_brings_back_a_removed_object_with_its_transform() {
        let db = SceneDatabase::new();
        let mut obj = object("Cube", ObjectType::Mesh(MeshType::Cube));
        obj.transform.position = [1.0, 2.0, 3.0];
        let id = db.add_object(obj, None);

        let snapshot = db.capture_history_snapshot();
        db.remove_object(&id);
        assert!(db.get_object(&id).is_none());

        db.restore_history_snapshot(&snapshot).unwrap();

        let restored = db.get_object(&id).expect("object restored");
        assert_eq!(restored.name, "Cube");
        assert_eq!(restored.transform.position, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn restore_brings_back_reflection_components() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Light", ObjectType::Light(LightType::Point)), None);
        db.add_component(
            &id,
            "LightComponent".to_string(),
            serde_json::json!({"intensity": 5.0}),
        );
        assert_eq!(db.get_components(&id).len(), 1);

        let snapshot = db.capture_history_snapshot();
        db.remove_component(&id, 0);
        assert!(db.get_components(&id).is_empty());

        db.restore_history_snapshot(&snapshot).unwrap();

        let components = db.get_components(&id);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].class_name, "LightComponent");
    }

    #[test]
    fn restore_preserves_hierarchy() {
        let db = SceneDatabase::new();
        let parent_id = db.add_object(object("Parent", ObjectType::Empty), None);
        let child_id = db.add_object(object("Child", ObjectType::Empty), Some(parent_id.clone()));

        let snapshot = db.capture_history_snapshot();
        db.clear();
        assert!(db.get_all_objects().is_empty());

        db.restore_history_snapshot(&snapshot).unwrap();

        let child = db.get_object(&child_id).expect("child restored");
        assert_eq!(child.parent.as_deref(), Some(parent_id.as_str()));
    }

    /// `store_revision` must advance across a restore even when the swapped-in
    /// store's raw counter lands on exactly the same value as the swapped-out
    /// one. Both restores below build a 1-object scene, so the fresh store's
    /// raw `render_revision` is identical both times — a naive equality check
    /// misses the second restore entirely, which is how an undo→redo pair
    /// would leave every panel stale.
    #[test]
    fn store_revision_advances_across_restores_with_identical_raw_counters() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("A", ObjectType::Empty), None);

        let snapshot_a = db.capture_history_snapshot();
        db.set_name(&id, "B".to_string());
        let snapshot_b = db.capture_history_snapshot();
        let _ = snapshot_b; // symmetric with the undo/redo flow below

        // Restore A, then B: two wholesale store swaps with equal object
        // counts and therefore equal raw counters after each swap.
        db.restore_history_snapshot(&snapshot_a).unwrap();
        assert_eq!(db.get_object(&id).unwrap().name, "A");
        let rev_after_first_restore = db.store_revision();

        db.restore_history_snapshot(&snapshot_a).unwrap();
        let rev_after_second_restore = db.store_revision();
        assert!(
            rev_after_second_restore > rev_after_first_restore,
            "identical raw counters across a store swap must still fold to an advanced revision"
        );
    }

    #[test]
    fn targeted_reads_match_the_full_object_read() {
        let db = SceneDatabase::new();
        let mut obj = object("Cube", ObjectType::Mesh(MeshType::Cube));
        obj.transform.position = [1.0, 2.0, 3.0];
        obj.transform.rotation = [10.0, 20.0, 30.0];
        obj.transform.scale = [2.0, 2.0, 2.0];
        obj.visible = false;
        obj.locked = true;
        let id = db.add_object(obj, None);

        let full = db.get_object(&id).unwrap();
        let t = db.get_object_transform(&id).unwrap();
        assert_eq!(t.position, full.transform.position);
        assert_eq!(t.rotation, full.transform.rotation);
        assert_eq!(t.scale, full.transform.scale);
        assert_eq!(db.get_object_name(&id).unwrap(), full.name);
        assert_eq!(
            db.get_object_visibility(&id).unwrap(),
            (full.visible, full.locked)
        );
        assert!(db.get_object_transform(&"missing".into()).is_none());
    }
}

/// Phase B4 (Pulsar-Native#555): proves `StaticMeshComponent` -- the first
/// component migrated onto `pulsar_world_registry`'s `World` bridge --
/// actually gets hydrated/removed through the real `SceneDatabase` wiring,
/// not just the synthetic fixture `pulsar_world_registry`'s own unit tests
/// use. Reaches into `db.store` directly (a private field) -- valid since
/// this module is a descendant of `scene_database`, not external code
/// working through the public API only.
#[cfg(test)]
mod world_component_hydration_tests {
    use super::*;
    use helio_component::StaticMeshComponent;

    fn object(name: &str) -> SceneObjectData {
        SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type: ObjectType::Mesh(MeshType::Custom),
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        }
    }

    #[test]
    fn add_component_hydrates_the_typed_world_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);

        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<StaticMeshComponent>(entity).unwrap();
        assert_eq!(
            hydrated.mesh_asset.as_str(),
            "meshes/primitives/SM_Cube.fbx"
        );
    }

    #[test]
    fn update_component_property_re_hydrates_the_typed_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );

        db.update_component_property(
            &id,
            "StaticMeshComponent",
            "mesh_asset",
            serde_json::json!("meshes/primitives/SM_Sphere.fbx"),
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<StaticMeshComponent>(entity).unwrap();
        assert_eq!(
            hydrated.mesh_asset.as_str(),
            "meshes/primitives/SM_Sphere.fbx"
        );
    }

    #[test]
    fn remove_component_drops_the_typed_world_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );
        {
            let store = db.store.read();
            let entity = store.entity_for(&id).unwrap();
            assert!(store.world().get::<StaticMeshComponent>(entity).is_some());
        }

        db.remove_component(&id, 0);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store.world().get::<StaticMeshComponent>(entity).is_none());
    }

    #[test]
    fn disabling_a_component_drops_the_typed_world_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );

        db.set_component_enabled(&id, 0, false);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store.world().get::<StaticMeshComponent>(entity).is_none());
    }

    #[test]
    fn malformed_component_json_does_not_hydrate_but_does_not_panic() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);

        // `mesh_asset` should be a string; this is a type mismatch, not a
        // missing field, so it should fail hydration cleanly.
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            serde_json::json!({"mesh_asset": 12345}),
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store.world().get::<StaticMeshComponent>(entity).is_none());
    }

    /// Phase B5 (Pulsar-Native#556) spot check: the migration mechanism
    /// itself is already fully proven generically (`pulsar_world_registry`'s
    /// own tests) and end-to-end on `StaticMeshComponent` above -- this
    /// isn't re-proving the mechanism per component (that would just be
    /// duplicating the same five tests seven more times), it's checking for
    /// component-specific surprises. `LightComponent` has many fields with
    /// nested enum sub-props (`IntensityUnits`, `ShadowCacheMode`, ...) --
    /// worth confirming `Default`-derived JSON round-trips through
    /// hydration cleanly, not just a single-field component like
    /// `StaticMeshComponent`.
    #[test]
    fn light_component_hydrates_via_its_default_json() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json =
            serde_json::to_value(helio_component::LightComponent::default()).unwrap();

        db.add_component(&id, "LightComponent".to_string(), default_json);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store
            .world()
            .get::<helio_component::LightComponent>(entity)
            .is_some());
    }

    /// Pulsar-Native#561 regression test: editing a `#[sub_props]`-nested
    /// leaf field (e.g. `LightComponent.intensity.intensity`) through
    /// `update_live_component_property` must land in the correct nested
    /// location and must not disturb any sibling field or sub-group -- the
    /// exact failure mode of the bug this fixes was a flat top-level JSON
    /// write either landing on a JSON key the struct doesn't have (silently
    /// dropped) or, worse, overwriting a whole nested sub-struct with a bare
    /// scalar when the leaf name happened to collide with its parent
    /// sub-props field's own name (`color`/`color`).
    #[test]
    fn update_live_component_property_edits_only_the_targeted_nested_leaf() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json);

        // `intensity` is a leaf field inside `IntensityLightProps`, itself
        // reached through `LightComponent.intensity: IntensityLightProps` --
        // a flat top-level JSON write would land on a key `LightComponent`
        // doesn't have at all (silently ignored by serde on next load), not
        // `data.intensity.intensity`.
        let applied = db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(500.0_f32) as Box<dyn Any + Send>,
        );
        assert!(
            applied.is_ok(),
            "live edit should apply directly, no JSON fallback needed"
        );

        // `color` is a leaf field inside `ColorLightProps`, whose *parent*
        // sub-props field on `LightComponent` is also named `color` -- the
        // exact name collision that made the old flat write corrupt the
        // whole nested object instead of just failing quietly.
        let applied = db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "color",
            Box::new([0.25_f32, 0.5, 0.75, 1.0]) as Box<dyn Any + Send>,
        );
        assert!(applied.is_ok());

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<LightComponent>(entity).unwrap();

        assert_eq!(hydrated.intensity.intensity, 500.0);
        assert_eq!(hydrated.color.color, [0.25, 0.5, 0.75, 1.0]);

        // Every sibling field on both touched sub-groups, and every
        // untouched sub-group, must still match `Default` exactly -- proving
        // the edit was scoped to just the one targeted leaf, not a
        // sub-struct-clobbering overwrite.
        let expected = LightComponent::default();
        assert_eq!(
            hydrated.intensity.intensity_units,
            expected.intensity.intensity_units
        );
        assert_eq!(
            hydrated.intensity.exposure_compensation,
            expected.intensity.exposure_compensation
        );
        assert_eq!(
            hydrated.color.use_temperature,
            expected.color.use_temperature
        );
        assert_eq!(
            hydrated.color.temperature_kelvin,
            expected.color.temperature_kelvin
        );
        // `GeneralLightProps`/`AttenuationLightProps`/`ShadowLightProps`
        // don't derive `PartialEq` -- compare via their own `Serialize`
        // impl instead (both already derive it for the JSON boundary).
        assert_eq!(
            serde_json::to_value(&hydrated.general).unwrap(),
            serde_json::to_value(&expected.general).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&hydrated.attenuation).unwrap(),
            serde_json::to_value(&expected.attenuation).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&hydrated.shadows).unwrap(),
            serde_json::to_value(&expected.shadows).unwrap()
        );
    }

    /// Regression test for the actual bug behind "the properties panel
    /// shows the right value, but the light in the scene never changes":
    /// `update_live_component_property` mutated the live `World` component
    /// correctly, but never touched `WorldSceneStore`'s own dirty-tracking
    /// (`dirty`/`dirty_gen`/`render_revision`), which is the *only* thing
    /// `HelioRenderer::render_frame` checks to decide whether a sync pass
    /// (`sync_scene`/`sync_scene_delta` -- the thing that actually pushes a
    /// component's current value into Helio's scene) should run at all.
    /// A `World`-correct edit that never bumps `render_revision` is
    /// invisible to the renderer, indefinitely, even though every direct
    /// `World` read (this method's own `read_live_component_property`
    /// counterpart, and the properties panel that calls it) sees it fine.
    #[test]
    fn update_live_component_property_marks_the_object_dirty_for_the_renderer() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json);

        let revision_before = db.store.read().render_revision();

        db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(1000.0_f32) as Box<dyn Any + Send>,
        )
        .expect("live edit should apply");

        assert!(
            db.store.read().render_revision() > revision_before,
            "a live component edit must bump render_revision, or HelioRenderer's \
             render_frame never even attempts a sync pass for it -- the World value \
             would be correct (readable directly) but never reach the actual scene"
        );

        let flags = db.store.write().take_dirty_flags(&id);
        assert!(
            flags.contains(engine_backend::scene::ObjectDirtyFlags::COMPONENTS),
            "dirty flags must include COMPONENTS so sync picks the object's \
             components back up, not just its transform"
        );
    }

    /// Pulsar-Native#561 regression test: `update_live_component_property`
    /// writes straight to `World` and nowhere else -- `get_components`
    /// (what both the properties panel's card list and
    /// `save_to_file_with_editor_camera` read) must still see the edit, by
    /// resolving `data` fresh off the live `World` value rather than
    /// trusting `component_store`'s now-stale stored copy. Without this, a live
    /// edit would render correctly in the properties panel (which reads
    /// each field individually via `read_live_component_property`) but be
    /// silently lost on save -- exactly the kind of two-competing-copies
    /// bug this whole fix exists to eliminate.
    #[test]
    fn live_edit_is_visible_through_get_components_not_just_the_live_read_path() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json);

        db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(750.0_f32) as Box<dyn Any + Send>,
        )
        .expect("LightComponent is World-registered, edit should apply live");

        let components = db.get_components(&id);
        let light = components
            .iter()
            .find(|c| c.class_name == "LightComponent")
            .expect("LightComponent should still be attached");
        assert_eq!(
            light.data.get("intensity").and_then(|v| v.get("intensity")),
            Some(&serde_json::json!(750.0)),
            "get_components (and therefore save-to-disk) must reflect the live edit, \
             not component_store's stale stored JSON"
        );
    }

    /// Pulsar-Native#561 regression test for Bug B (the light-color crash's
    /// second, independent cause): `update_live_component_property` writes
    /// straight to `World`, but before this fix never persisted back into
    /// `component_store`. `sync_registered_component_props_to_scene_db` -- which
    /// runs on *every* transform/name/visibility/legacy-component edit, not
    /// just component-property edits -- re-hydrates every `World`-registered
    /// component from `component_store`'s (stale, pre-edit) JSON. Net effect
    /// before the fix: a live-edited property was visible immediately, then
    /// silently reverted the moment the user made *any other* edit to the
    /// same object. This test edits a component property live, then performs
    /// a wholly unrelated `update_object` (a transform move) on the SAME
    /// object, and asserts the property edit survived -- the exact sequence
    /// that used to clobber it.
    #[test]
    fn update_live_component_property_survives_an_unrelated_update_object_call() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json);

        db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(750.0_f32) as Box<dyn Any + Send>,
        )
        .expect("LightComponent is World-registered, edit should apply live");

        // An edit to something else entirely on the same object -- this used
        // to be exactly what triggered the clobber, since `update_object`
        // calls `sync_registered_component_props_to_scene_db` unconditionally.
        let mut moved = db.get_object(&id).expect("object should exist");
        moved.transform.position = [1.0, 2.0, 3.0];
        db.update_object(moved);

        let components = db.get_components(&id);
        let light = components
            .iter()
            .find(|c| c.class_name == "LightComponent")
            .expect("LightComponent should still be attached");
        assert_eq!(
            light.data.get("intensity").and_then(|v| v.get("intensity")),
            Some(&serde_json::json!(750.0)),
            "an unrelated update_object call must not revert a live component \
             property edit -- component_store and World must never diverge for \
             typed-path edits"
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<LightComponent>(entity).unwrap();
        assert_eq!(
            hydrated.intensity.intensity, 750.0,
            "the live World value itself must also survive, not just what \
             get_components reports"
        );
    }

    /// `PortalComponent` is the trickiest of B5's list -- its
    /// `sync_component` pairs two components sharing a `portal_id` via
    /// `PortalLinkCache`, tracked independently of storage. Worth confirming
    /// two portal-typed objects both hydrate correctly (the pairing logic
    /// itself is unaffected by storage, but this proves *that* claim rather
    /// than just asserting it).
    #[test]
    fn portal_component_hydrates_on_both_sides_of_a_pair() {
        let db = SceneDatabase::new();
        let a = db.add_object(object("PortalA"), None);
        let b = db.add_object(object("PortalB"), None);
        let default_json =
            serde_json::to_value(helio_component::PortalComponent::default()).unwrap();

        db.add_component(&a, "PortalComponent".to_string(), default_json.clone());
        db.add_component(&b, "PortalComponent".to_string(), default_json);

        let store = db.store.read();
        let entity_a = store.entity_for(&a).unwrap();
        let entity_b = store.entity_for(&b).unwrap();
        assert!(store
            .world()
            .get::<helio_component::PortalComponent>(entity_a)
            .is_some());
        assert!(store
            .world()
            .get::<helio_component::PortalComponent>(entity_b)
            .is_some());
    }

    /// Phase D (Pulsar-Native#558): `ReflectionCaptureComponent` is the
    /// first newly-authored (not migrated) component to go through this
    /// mechanism -- same spot-check shape as B5's, confirming the
    /// already-proven mechanism holds for brand-new components too, not
    /// just migrated ones.
    #[test]
    fn reflection_capture_component_hydrates_via_its_default_json() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Probe"), None);
        let default_json =
            serde_json::to_value(helio_component::ReflectionCaptureComponent::default()).unwrap();

        db.add_component(&id, "ReflectionCaptureComponent".to_string(), default_json);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store
            .world()
            .get::<helio_component::ReflectionCaptureComponent>(entity)
            .is_some());
    }

    #[test]
    fn water_volume_component_hydrates_via_its_default_json() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Lake"), None);
        let default_json =
            serde_json::to_value(helio_component::WaterVolumeComponent::default()).unwrap();

        db.add_component(&id, "WaterVolumeComponent".to_string(), default_json);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store
            .world()
            .get::<helio_component::WaterVolumeComponent>(entity)
            .is_some());
    }

    #[test]
    fn post_process_volume_component_hydrates_via_its_default_json() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("GlobalPostFx"), None);
        let default_json =
            serde_json::to_value(helio_component::PostProcessVolumeComponent::default()).unwrap();

        db.add_component(&id, "PostProcessVolumeComponent".to_string(), default_json);

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        assert!(store
            .world()
            .get::<helio_component::PostProcessVolumeComponent>(entity)
            .is_some());
    }

    // ── World subscriptions (Pulsar-Native#575, SceneDB#47) ────────────────

    /// Pulsar-Native#519: two instances of the SAME class on one object are
    /// two independent value stores. `World` can hold only the first
    /// enabled instance typed; the duplicate keeps its OWN metadata JSON,
    /// and edits route by index -- editing instance 1 must never touch
    /// instance 0 (or the reverse), on either the read or write side.
    #[test]
    fn duplicate_class_instances_hold_independent_field_values() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json.clone());
        db.add_component(&id, "LightComponent".to_string(), default_json);

        // Exactly one live-typed representative, and it's instance 0.
        assert_eq!(
            db.live_typed_component_index(&id, "LightComponent"),
            Some(0),
            "of N duplicates, only the first enabled one is World-typed"
        );

        // Distinct edits to each instance, by index.
        db.update_live_component_property(
            &id,
            "LightComponent",
            1,
            "intensity",
            Box::new(111.0_f32) as Box<dyn Any + Send>,
        )
        .expect("duplicate-instance edit routes by index");
        db.update_live_component_property(
            &id,
            "LightComponent",
            0,
            "intensity",
            Box::new(222.0_f32) as Box<dyn Any + Send>,
        )
        .expect("live-typed edit applies");

        // Write side stayed per-instance: the World-typed value (and its
        // metadata mirror at index 0) carries ONLY instance 0's edit;
        // instance 1's blob carries only its own.
        let components = db.get_components(&id);
        assert_eq!(
            components[0].data.pointer("/intensity/intensity"),
            Some(&serde_json::json!(222.0)),
        );
        assert_eq!(
            components[1].data.pointer("/intensity/intensity"),
            Some(&serde_json::json!(111.0)),
            "instance 1's stored value must be its own -- neither the World \
             overlay nor instance 0's edit may clobber it"
        );
        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<LightComponent>(entity).unwrap();
        assert_eq!(hydrated.intensity.intensity, 222.0);
    }

    /// Pulsar-Native#519 follow-through: when the current live-typed
    /// instance goes away (removed), the NEXT duplicate becomes the
    /// representative and is re-hydrated from ITS OWN edited JSON -- not
    /// from anything instance 0 left behind.
    #[test]
    fn removing_the_live_instance_promotes_the_duplicate_from_its_own_json() {
        use helio_component::LightComponent;
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Light"), None);
        let default_json = serde_json::to_value(LightComponent::default()).unwrap();
        db.add_component(&id, "LightComponent".to_string(), default_json.clone());
        db.add_component(&id, "LightComponent".to_string(), default_json);

        // Give each instance a distinct intensity BEFORE any removal.
        db.update_live_component_property(
            &id,
            "LightComponent",
            1,
            "intensity",
            Box::new(111.0_f32) as Box<dyn Any + Send>,
        )
        .unwrap();

        // Remove instance 0; instance 1 (intensity 111.0) is now first.
        db.remove_component(&id, 0);

        assert_eq!(
            db.live_typed_component_index(&id, "LightComponent"),
            Some(0),
            "the surviving duplicate is now the class's live-typed instance"
        );
        let components = db.get_components(&id);
        assert_eq!(
            components[0].data.pointer("/intensity/intensity"),
            Some(&serde_json::json!(111.0)),
            "promotion must adopt the duplicate's OWN field values"
        );
    }

    /// The index is a hard identity check, not advisory: a stale index
    /// pointing at a different class must refuse the edit rather than land
    /// it in some other instance.
    #[test]
    fn update_live_component_property_refuses_a_mismatched_component_index() {
        use helio_component::{LightComponent, StaticMeshComponent};
        use std::any::Any;

        let db = SceneDatabase::new();
        let id = db.add_object(object("Thing"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );
        db.add_component(
            &id,
            "LightComponent".to_string(),
            serde_json::to_value(LightComponent::default()).unwrap(),
        );

        // Index 0 is the StaticMeshComponent; claiming it for a Light edit
        // must bounce the value straight back, unmodified.
        let value = Box::new(500.0_f32) as Box<dyn Any + Send>;
        let result =
            db.update_live_component_property(&id, "LightComponent", 0, "intensity", value);
        assert!(result.is_err(), "class/index mismatch must refuse");
    }

    /// The properties panel's core contract: arm once per card, edit the
    /// live value through the real write path, and the subscription delivers
    /// exactly one event tagged with that card's id. A drain empties; an
    /// unsubscribed card hears nothing further.
    #[test]
    fn subscribe_component_delivers_events_for_live_edits_to_that_card_only() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            static_mesh_component_json("meshes/primitives/SM_Cube.fbx"),
        );

        let sub = db
            .subscribe_component(&id, "StaticMeshComponent")
            .expect("registered class with a live entity subscribes");
        assert!(
            db.take_world_component_events().is_empty(),
            "arming alone must deliver nothing"
        );
        // Drain is emptying, not peeking: a second drain is empty too.
        assert!(db.take_world_component_events().is_empty());

        // Real mutation through the same typed path the panel's setter
        // closures ride (World::get_mut -> Mut guard -> into_inner).
        {
            let mut store = db.store.write();
            let entity = store.entity_for(&id).unwrap();
            let instance = pulsar_world_registry::get_world_component_as_engine_class_mut(
                "StaticMeshComponent",
                store.world_mut(),
                entity,
            )
            .unwrap();
            let concrete = instance
                .as_any_mut()
                .downcast_mut::<StaticMeshComponent>()
                .unwrap();
            concrete.mesh_asset = "meshes/primitives/SM_Sphere.fbx".into();
        }

        let events: Vec<_> = db
            .take_world_component_events()
            .into_iter()
            .filter(|e| e.subscription == sub)
            .collect();
        assert_eq!(events.len(), 1, "one real mutation = exactly one event");
        assert_eq!(events[0].kind, pulsar_scenedb::ComponentChangeKind::Mutated);
        assert!(db.take_world_component_events().is_empty());

        // After unsubscribe, the same kind of write stays silent.
        db.unsubscribe_component(sub);
        {
            let mut store = db.store.write();
            let entity = store.entity_for(&id).unwrap();
            let instance = pulsar_world_registry::get_world_component_as_engine_class_mut(
                "StaticMeshComponent",
                store.world_mut(),
                entity,
            )
            .unwrap();
            let concrete = instance
                .as_any_mut()
                .downcast_mut::<StaticMeshComponent>()
                .unwrap();
            concrete.mesh_asset = "meshes/primitives/SM_Cone.fbx".into();
        }
        assert!(db.take_world_component_events().is_empty());
    }

    /// Unregistered classes have no live `World` representation -- there is
    /// nothing to subscribe to, and `None` (not an error) is the answer.
    #[test]
    fn subscribing_an_unregistered_class_is_none() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        assert!(db
            .subscribe_component(&id, "NoSuchComponentClass")
            .is_none());
    }

    /// Undo/redo swaps the whole `World` out from under any outstanding
    /// subscriptions (they live inside it) without firing events. The epoch
    /// is the only signal that this happened, so it MUST advance across a
    /// restore even when the snapshot content is identical.
    #[test]
    fn restore_history_snapshot_bumps_the_subscriptions_epoch() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);

        let before = db.subscriptions_epoch();
        let snapshot = db.capture_history_snapshot();

        db.restore_history_snapshot(&snapshot).unwrap();
        assert_ne!(
            db.subscriptions_epoch(),
            before,
            "a store swap must invalidate every outstanding subscription"
        );
    }

    /// The lifecycle owner creates the SceneDB-backed store once; consumers
    /// such as the renderer receive only a shared handle to that same store.
    #[test]
    fn shared_store_is_the_database_scene() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        let store = db.shared_store();

        let store = store.read();
        let entity = store.entity_for(&id).expect("object lives in shared store");
        assert_eq!(store.name(entity), Some("Cube"));
    }
}

#[cfg(test)]
mod blueprint_bindings_preservation_tests {
    //! #650 — editor saves must never destroy a level's Blueprint-binding
    //! section (the editor cannot author it yet, but the runtime loader
    //! consumes it).

    use super::*;

    fn sample_bindings() -> pulsar_scene::BlueprintBindings {
        let mut bindings = pulsar_scene::BlueprintBindings::new();
        bindings.insert(
            "lever_a".to_string(),
            vec![pulsar_scene::BlueprintBinding {
                class_name: "Lever".to_string(),
                overrides: {
                    let mut map = std::collections::HashMap::new();
                    map.insert("speed".to_string(), serde_json::json!(7.5));
                    map
                },
            }],
        );
        bindings
    }

    /// A save over an existing file preserves its `blueprint_bindings`
    /// section byte-for-value, keyed by StableId with overrides intact.
    #[test]
    fn saving_preserves_an_authored_bindings_section() {
        let dir =
            std::env::temp_dir().join(format!("pulsar_650_editor_save_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let path = dir.join("roundtrip.level.json");

        // Seed a file as if hand-authored / written by the runtime tooling
        // (full v2.x shape — objects carry their required fields).
        let seeded = format!(
            r#"{{ "version": "2.1",
                 "objects": [],
                 "metadata": {{"created": "2026-01-01T00:00:00Z", "modified": "2026-01-01T00:00:00Z", "editor_version": "0.1.0"}},
                 "blueprint_bindings": {{"lever_a": [{{"class_name": "Lever", "overrides": {{"speed": 7.5}}}}]}} }}"#
        );
        virtual_fs::write_file(&path, seeded.as_bytes()).expect("seed file");

        // An ordinary editor save (fresh LevelFile construction) must keep it.
        let db = SceneDatabase::new();
        db.save_to_file_with_editor_camera(&path, None, None)
            .expect("save");

        let saved: LevelFile = {
            let bytes = virtual_fs::read_file(&path).expect("read back");
            serde_json::from_str(&String::from_utf8(bytes).unwrap()).expect("parse")
        };
        assert_eq!(
            saved.blueprint_bindings,
            sample_bindings(),
            "bindings survive re-save"
        );

        // Files without the section still save cleanly (no phantom key).
        let bare = dir.join("bare.level.json");
        virtual_fs::write_file(
            &bare,
            r#"{ "version": "2.1", "objects": [],
                 "metadata": {"created": "2026-01-01T00:00:00Z", "modified": "2026-01-01T00:00:00Z", "editor_version": "0.1.0"} }"#
                .as_bytes(),
        )
        .expect("seed bare");
        db.save_to_file(&bare).expect("save bare");
        let saved_bare: LevelFile = {
            let bytes = virtual_fs::read_file(&bare).expect("read back");
            serde_json::from_str(&String::from_utf8(bytes).unwrap()).expect("parse")
        };
        assert!(saved_bare.blueprint_bindings.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Loading a file carrying bindings succeeds (the section is additive,
    /// ignored by the editor today) and the objects load untouched.
    #[test]
    fn loading_a_bound_level_succeeds_and_ignores_the_section_for_now() {
        let db = SceneDatabase::new();
        let json = r#"{
            "version": "2.1",
            "objects": [
                { "id": "lever_a", "name": "Lever A", "object_type": {"Mesh": "Cube"},
                  "transform": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
                  "parent": null, "visible": true, "locked": false,
                  "children": [], "scene_path": "", "props": {} }
            ],
            "metadata": {"created": "2026-01-01T00:00:00Z", "modified": "2026-01-01T00:00:00Z", "editor_version": "0.1.0"},
            "blueprint_bindings": { "lever_a": [ { "class_name": "Lever", "overrides": {} } ] }
        }"#;
        let path = std::env::temp_dir().join(format!(
            "pulsar_650_editor_load_{}.json",
            std::process::id()
        ));
        virtual_fs::write_file(&path, json.as_bytes()).expect("write");

        db.load_from_file(&path).expect("bound levels load");
        let objects = db.get_all_objects();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].id, "lever_a");

        let _ = std::fs::remove_file(&path);
    }
}
