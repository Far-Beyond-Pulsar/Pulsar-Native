//! Shared scene database for the Pulsar engine.
//!
//! This module provides two scene management systems:
//!
//! ## Legacy SceneDb (existing system)
//! - Single `SceneDb` that is shared by UI panels and renderer
//! - Stores transforms as atomics for lock-free access
//! - Being phased out in favor of Helio Scene + metadata layer
//!
//! ## New Metadata System (production-ready)
//! - `SceneMetadataDb`: Organizational layer (folders, names, hierarchy)
//! - `ComponentDb`: Component instances using reflection system
//! - `HierarchyManager`: Parent-child relationships with cycle prevention
//! - Helio Scene is the single source of truth for transform/render data
//! - Uses engine class reflection system for extensible components
//!
//! The new system reduces memory footprint by eliminating duplicate transform
//! storage and provides a clean separation between organizational metadata
//! and render data.

// New metadata system modules
pub mod component_db;
pub mod hierarchy;
pub mod metadata;
pub mod metadata_db;

// Re-export new system types for convenience
pub use component_db::ComponentDb;
pub use hierarchy::HierarchyManager;
pub use metadata::{
    ComponentInstance, EditorObjectId, HelioActorHandle, HelioLightId, HelioObjectId,
    LightType as MetadataLightType, MeshType as MetadataMeshType, ObjectType as MetadataObjectType,
    SceneObjectMetadata,
};
pub use metadata_db::{SceneMetadataDb, SceneSnapshot};

use dashmap::DashMap;
use glam::{Mat4, Vec3};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ─── Public types ────────────────────────────────────────────────────────────

pub type ObjectId = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    Empty,
    Folder,
    Camera,
    Light(LightType),
    Mesh(MeshType),
    ParticleSystem,
    AudioSource,
    Blueprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightType {
    Directional,
    Point,
    Spot,
    Area,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshType {
    Cube,
    Sphere,
    Cylinder,
    Plane,
    Custom,
}

/// Field type information for UI generation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTypeInfo {
    F32,
    F64,
    I32,
    I64,
    U32,
    U64,
    Bool,
    String,
    F32Array(usize),
    Other(&'static str),
}

/// A point-in-time snapshot of an object — used by the UI for display, undo/redo, and serialization.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneObjectSnapshot {
    pub id: ObjectId,
    pub name: String,
    /// Canonical path from scene root, e.g. "Geometry/Spheres/Blue Sphere".
    /// Derived from the parent chain; always kept in sync by SceneDb.
    #[serde(default)]
    pub scene_path: String,
    pub object_type: ObjectType,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub parent: Option<ObjectId>,
    /// Children are populated on read by SceneDb; not stored per-entry.
    pub children: Vec<ObjectId>,
    pub visible: bool,
    pub locked: bool,
    /// Arbitrary type-specific properties that round-trip through the level file.
    ///
    /// Lights store: `"color_r"`, `"color_g"`, `"color_b"`, `"intensity"`, `"range"`.
    /// Any future object type can extend this without schema changes.
    ///
    /// ⚠ This field does NOT contain `__component_instances`.  Component data is
    /// carried in the separate `component_instances` field below and is synced
    /// exclusively through `SceneDatabase::sync_registered_component_props_to_scene_db`.
    /// Setting `__component_instances` directly in `props` has no effect on the
    /// rendering subsystem.
    #[serde(default)]
    pub props: HashMap<String, serde_json::Value>,
    /// Reflection-based component instances synced from the editor's metadata
    /// database by `sync_registered_component_props_to_scene_db`.  This is the
    /// authoritative source that the renderer reads each frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_instances: Option<serde_json::Value>,
}

// ─── Live object entry ───────────────────────────────────────────────────────

/// A live scene object stored in the database.
///
/// Transform and visibility are atomic so the render thread reads them
/// without ever acquiring a lock.
pub struct SceneEntry {
    pub id: ObjectId,
    /// Object type is immutable after creation.
    pub object_type: ObjectType,

    // Hot path — lock-free atomic reads/writes
    position: [AtomicU32; 3],
    rotation: [AtomicU32; 3],
    scale: [AtomicU32; 3],
    visible: AtomicBool,
    locked: AtomicBool,

    // Cold data — hierarchy + name + components; rarely changes
    pub meta: RwLock<SceneEntryMeta>,
}

pub struct SceneEntryMeta {
    pub name: String,
    pub parent: Option<ObjectId>,
    /// Canonical path, e.g. "Geometry/Spheres/Blue Sphere".  Kept in sync by SceneDb.
    pub scene_path: String,
    /// Type-specific properties — see `SceneObjectSnapshot::props`.
    pub props: HashMap<String, serde_json::Value>,
    /// Reflection-based component instances (set by
    /// `SceneDatabase::sync_registered_component_props_to_scene_db`).
    /// Renderer reads this each frame instead of looking for `__component_instances` in props.
    pub component_instances: Option<serde_json::Value>,
}

impl SceneEntry {
    fn new(snap: &SceneObjectSnapshot) -> Self {
        Self {
            id: snap.id.clone(),
            object_type: snap.object_type,
            position: [
                AtomicU32::new(snap.position[0].to_bits()),
                AtomicU32::new(snap.position[1].to_bits()),
                AtomicU32::new(snap.position[2].to_bits()),
            ],
            rotation: [
                AtomicU32::new(snap.rotation[0].to_bits()),
                AtomicU32::new(snap.rotation[1].to_bits()),
                AtomicU32::new(snap.rotation[2].to_bits()),
            ],
            scale: [
                AtomicU32::new(snap.scale[0].to_bits()),
                AtomicU32::new(snap.scale[1].to_bits()),
                AtomicU32::new(snap.scale[2].to_bits()),
            ],
            visible: AtomicBool::new(snap.visible),
            locked: AtomicBool::new(snap.locked),
            meta: RwLock::new(SceneEntryMeta {
                name: snap.name.clone(),
                parent: snap.parent.clone(),
                scene_path: snap.scene_path.clone(),
                props: snap.props.clone(),
                component_instances: snap.component_instances.clone(),
            }),
        }
    }

    // ── Hot-path accessors (fully lock-free) ─────────────────────────────

    #[inline]
    pub fn get_position(&self) -> [f32; 3] {
        [
            f32::from_bits(self.position[0].load(Ordering::Relaxed)),
            f32::from_bits(self.position[1].load(Ordering::Relaxed)),
            f32::from_bits(self.position[2].load(Ordering::Relaxed)),
        ]
    }

    #[inline]
    pub fn set_position(&self, v: [f32; 3]) -> bool {
        let bits = v.map(f32::to_bits);
        let x = self.position[0].swap(bits[0], Ordering::Relaxed) != bits[0];
        let y = self.position[1].swap(bits[1], Ordering::Relaxed) != bits[1];
        let z = self.position[2].swap(bits[2], Ordering::Relaxed) != bits[2];
        x || y || z
    }

    #[inline]
    pub fn get_rotation(&self) -> [f32; 3] {
        [
            f32::from_bits(self.rotation[0].load(Ordering::Relaxed)),
            f32::from_bits(self.rotation[1].load(Ordering::Relaxed)),
            f32::from_bits(self.rotation[2].load(Ordering::Relaxed)),
        ]
    }

    #[inline]
    pub fn set_rotation(&self, v: [f32; 3]) -> bool {
        let bits = v.map(f32::to_bits);
        let x = self.rotation[0].swap(bits[0], Ordering::Relaxed) != bits[0];
        let y = self.rotation[1].swap(bits[1], Ordering::Relaxed) != bits[1];
        let z = self.rotation[2].swap(bits[2], Ordering::Relaxed) != bits[2];
        x || y || z
    }

    #[inline]
    pub fn get_scale(&self) -> [f32; 3] {
        [
            f32::from_bits(self.scale[0].load(Ordering::Relaxed)),
            f32::from_bits(self.scale[1].load(Ordering::Relaxed)),
            f32::from_bits(self.scale[2].load(Ordering::Relaxed)),
        ]
    }

    #[inline]
    pub fn set_scale(&self, v: [f32; 3]) -> bool {
        let bits = v.map(f32::to_bits);
        let x = self.scale[0].swap(bits[0], Ordering::Relaxed) != bits[0];
        let y = self.scale[1].swap(bits[1], Ordering::Relaxed) != bits[1];
        let z = self.scale[2].swap(bits[2], Ordering::Relaxed) != bits[2];
        x || y || z
    }

    #[inline]
    pub fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_visible(&self, v: bool) -> bool {
        self.visible.swap(v, Ordering::Relaxed) != v
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_locked(&self, v: bool) -> bool {
        self.locked.swap(v, Ordering::Relaxed) != v
    }

    /// Build a world-space Mat4 from the current atomic transform.
    /// Called by the renderer every frame — zero locks.
    #[inline]
    pub fn to_mat4(&self) -> Mat4 {
        let p = self.get_position();
        let r = self.get_rotation();
        let s = self.get_scale();
        Mat4::from_translation(Vec3::from(p))
            * Mat4::from_euler(
                glam::EulerRot::YXZ,
                r[1].to_radians(),
                r[0].to_radians(),
                r[2].to_radians(),
            )
            * Mat4::from_scale(Vec3::from(s))
    }

    /// Take a snapshot of this entry for UI display / undo-redo.
    /// Note: `children` is left empty — SceneDb populates it from the children_map.
    pub fn snapshot(&self) -> SceneObjectSnapshot {
        let meta = self.meta.read();
        SceneObjectSnapshot {
            id: self.id.clone(),
            name: meta.name.clone(),
            scene_path: meta.scene_path.clone(),
            object_type: self.object_type,
            position: self.get_position(),
            rotation: self.get_rotation(),
            scale: self.get_scale(),
            parent: meta.parent.clone(),
            children: vec![],
            visible: self.is_visible(),
            locked: self.is_locked(),
            props: meta.props.clone(),
            component_instances: meta.component_instances.clone(),
        }
    }
}

// ─── SceneDb ─────────────────────────────────────────────────────────────────

/// Gizmo state for the level editor
#[derive(Clone, Debug, PartialEq)]
pub struct GizmoState {
    pub gizmo_type: GizmoType,
    pub highlighted_axis: Option<GizmoAxis>,
    pub scale_factor: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoType {
    None,
    Translate,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoAxis {
    X,
    Y,
    Z,
}

impl Default for GizmoState {
    fn default() -> Self {
        Self {
            gizmo_type: GizmoType::None,
            highlighted_axis: None,
            scale_factor: 1.0,
        }
    }
}

struct SceneDbInner {
    /// All objects — concurrent reads, no global lock.
    objects: DashMap<ObjectId, Arc<SceneEntry>>,
    /// Maps parent_id → ordered child ids.  Key "" (empty) = root-level objects.
    /// Single source of truth for parent-child relationships; replaces per-entry children lists.
    children_map: RwLock<HashMap<String, Vec<ObjectId>>>,
    /// Auto-incrementing id counter.
    next_id: AtomicU64,
    /// Monotonic generation for data consumed by the renderer. A release bump
    /// publishes the preceding atomic/cold-data writes to the render thread.
    render_revision: AtomicU64,
    /// Currently selected object id.
    selected: RwLock<Option<ObjectId>>,
    /// Gizmo state for the level editor
    gizmo_state: RwLock<GizmoState>,
}

/// The shared scene database. Clone-able — all clones share the same data.
#[derive(Clone)]
pub struct SceneDb {
    inner: Arc<SceneDbInner>,
}

impl SceneDb {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SceneDbInner {
                objects: DashMap::new(),
                children_map: RwLock::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                render_revision: AtomicU64::new(1),
                selected: RwLock::new(None),
                gizmo_state: RwLock::new(GizmoState::default()),
            }),
        }
    }

    // ── Object creation / deletion ────────────────────────────────────────

    /// Add an object. Returns its id.
    pub fn add_object(
        &self,
        mut snap: SceneObjectSnapshot,
        parent_id: Option<ObjectId>,
    ) -> ObjectId {
        // Assign id if empty
        if snap.id.is_empty() {
            let n = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
            snap.id = format!("object_{}", n);
        }
        let id = snap.id.clone();
        snap.parent = parent_id.clone();

        // Always recompute scene_path from the live parent chain so it is always accurate.
        snap.scene_path = {
            let parent_path = parent_id
                .as_deref()
                .and_then(|pid| self.inner.objects.get(pid))
                .map(|p| p.meta.read().scene_path.clone())
                .unwrap_or_default();
            if parent_path.is_empty() {
                snap.name.clone()
            } else {
                format!("{}/{}", parent_path, snap.name)
            }
        };

        // Register in children_map under the parent key (or "" for roots).
        {
            let key = parent_id.as_deref().unwrap_or("").to_string();
            self.inner
                .children_map
                .write()
                .entry(key)
                .or_insert_with(Vec::new)
                .push(id.clone());
        }

        self.inner
            .objects
            .insert(id.clone(), Arc::new(SceneEntry::new(&snap)));
        self.bump_render_revision();
        id
    }

    /// Remove an object (and recursively its children).
    pub fn remove_object(&self, id: &str) -> bool {
        // Collect children before touching anything (get_children reads children_map).
        let children = self.get_children(Some(id));

        if let Some((_, entry)) = self.inner.objects.remove(id) {
            let parent = entry.meta.read().parent.clone();

            // Remove id from its parent's list and drop id's own children list.
            {
                let key = parent.as_deref().unwrap_or("").to_string();
                let mut map = self.inner.children_map.write();
                if let Some(siblings) = map.get_mut(&key) {
                    siblings.retain(|c| c != id);
                }
                map.remove(id);
            }

            // Recurse into children (locks fully released above).
            for child_id in children {
                self.remove_object(&child_id);
            }

            // Deselect if needed.
            let mut sel = self.inner.selected.write();
            if sel.as_deref() == Some(id) {
                *sel = None;
            }
            self.bump_render_revision();
            true
        } else {
            false
        }
    }

    // ── Selection ─────────────────────────────────────────────────────────

    pub fn select_object(&self, id: Option<ObjectId>) {
        *self.inner.selected.write() = id;
    }

    pub fn get_selected_id(&self) -> Option<ObjectId> {
        self.inner.selected.read().clone()
    }

    pub fn get_selected(&self) -> Option<SceneObjectSnapshot> {
        let id = self.inner.selected.read().clone()?;
        self.get_object(&id)
    }

    // ── Reads ─────────────────────────────────────────────────────────────

    pub fn get_object(&self, id: &str) -> Option<SceneObjectSnapshot> {
        self.inner.objects.get(id).map(|e| {
            let mut snap = e.snapshot();
            snap.children = self.get_children(Some(id));
            snap
        })
    }

    /// Get a direct Arc to the live entry. The renderer uses this for atomic transform reads.
    pub fn get_entry(&self, id: &str) -> Option<Arc<SceneEntry>> {
        self.inner.objects.get(id).map(|e| e.clone())
    }

    pub fn get_root_snapshots(&self) -> Vec<SceneObjectSnapshot> {
        self.get_children(None)
            .into_iter()
            .filter_map(|id| self.get_object(&id))
            .collect()
    }

    pub fn get_all_snapshots(&self) -> Vec<SceneObjectSnapshot> {
        self.collect_dfs(None)
    }

    /// Current generation of scene data consumed by the renderer.
    #[inline]
    pub fn render_revision(&self) -> u64 {
        self.inner.render_revision.load(Ordering::Acquire)
    }

    /// Depth-first ordered snapshot of all objects (parents before children).
    /// Use this for serialisation so load order is always valid.
    fn collect_dfs(&self, parent_id: Option<&str>) -> Vec<SceneObjectSnapshot> {
        let mut result = Vec::new();
        for id in self.get_children(parent_id) {
            if let Some(snap) = self.get_object(&id) {
                result.push(snap);
                let mut children = self.collect_dfs(Some(&id));
                result.append(&mut children);
            }
        }
        result
    }

    /// Iterate over all live entries — the render thread calls this each frame.
    /// Uses DashMap's internal sharding so this is concurrent-safe and very fast.
    pub fn for_each_entry(&self, mut f: impl FnMut(&SceneEntry)) {
        for entry in self.inner.objects.iter() {
            f(&entry);
        }
    }

    // ── Atomic transform writes (called by props panel) ───────────────────

    pub fn set_position(&self, id: &str, v: [f32; 3]) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            if e.set_position(v) {
                self.bump_render_revision();
            }
            true
        } else {
            false
        }
    }

    pub fn set_rotation(&self, id: &str, v: [f32; 3]) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            if e.set_rotation(v) {
                self.bump_render_revision();
            }
            true
        } else {
            false
        }
    }

    pub fn set_scale(&self, id: &str, v: [f32; 3]) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            if e.set_scale(v) {
                self.bump_render_revision();
            }
            true
        } else {
            false
        }
    }

    pub fn set_visible(&self, id: &str, v: bool) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            if e.set_visible(v) {
                self.bump_render_revision();
            }
            true
        } else {
            false
        }
    }

    /// Update all three transform components at once from a snapshot.
    pub fn apply_transform(&self, id: &str, pos: [f32; 3], rot: [f32; 3], scale: [f32; 3]) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            let position_changed = e.set_position(pos);
            let rotation_changed = e.set_rotation(rot);
            let scale_changed = e.set_scale(scale);
            let changed = position_changed || rotation_changed || scale_changed;
            if changed {
                self.bump_render_revision();
            }
            true
        } else {
            false
        }
    }

    // ── Cold data writes ──────────────────────────────────────────────────

    pub fn set_name(&self, id: &str, name: String) -> bool {
        // Read parent first, then release entry ref before any further lookups.
        let parent = match self.inner.objects.get(id) {
            Some(e) => e.meta.read().parent.clone(),
            None => return false,
        };

        let parent_path = parent
            .as_deref()
            .and_then(|pid| self.inner.objects.get(pid))
            .map(|p| p.meta.read().scene_path.clone())
            .unwrap_or_default();
        let new_path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", parent_path, name)
        };

        if let Some(e) = self.inner.objects.get(id) {
            e.meta.write().name = name;
        }
        self.update_subtree_path(id, &new_path);
        true
    }

    pub fn set_locked(&self, id: &str, v: bool) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            e.set_locked(v);
            true
        } else {
            false
        }
    }

    /// Mutate cold component/property data while publishing the change to the
    /// renderer. Editor integrations must use this instead of writing `meta`
    /// directly, otherwise an unchanged render generation could hide the edit.
    pub fn update_render_data(&self, id: &str, update: impl FnOnce(&mut SceneEntryMeta)) -> bool {
        let Some(entry) = self.inner.objects.get(id) else {
            return false;
        };
        update(&mut entry.meta.write());
        drop(entry);
        self.bump_render_revision();
        true
    }

    /// Reparent an object. Prevents circular references.
    pub fn reparent_object(&self, id: &str, new_parent: Option<ObjectId>) -> bool {
        // Guard against cycles
        if let Some(ref pid) = new_parent {
            if self.is_ancestor_of(id, pid) {
                return false;
            }
        }

        let old_parent = match self.inner.objects.get(id) {
            Some(e) => e.meta.read().parent.clone(),
            None => return false,
        };

        // Update children_map atomically: remove from old parent, add to new.
        {
            let mut map = self.inner.children_map.write();
            let old_key = old_parent.as_deref().unwrap_or("").to_string();
            if let Some(siblings) = map.get_mut(&old_key) {
                siblings.retain(|c| c != id);
            }
            let new_key = new_parent.as_deref().unwrap_or("").to_string();
            map.entry(new_key)
                .or_insert_with(Vec::new)
                .push(id.to_string());
        }

        // Update the object's parent field.
        if let Some(e) = self.inner.objects.get(id) {
            e.meta.write().parent = new_parent.clone();
        }

        // Recompute scene_path for the moved subtree.
        let parent_path = new_parent
            .as_deref()
            .and_then(|pid| self.inner.objects.get(pid))
            .map(|p| p.meta.read().scene_path.clone())
            .unwrap_or_default();
        let name = self
            .inner
            .objects
            .get(id)
            .map(|e| e.meta.read().name.clone())
            .unwrap_or_default();
        let new_path = if parent_path.is_empty() {
            name
        } else {
            format!("{}/{}", parent_path, name)
        };
        self.update_subtree_path(id, &new_path);

        self.bump_render_revision();
        true
    }

    pub fn duplicate_object(&self, id: &str) -> Option<ObjectId> {
        let mut snap = self.get_object(id)?;
        let parent = snap.parent.clone();
        snap.id = String::new(); // auto-assign
        snap.name = format!("{} Copy", snap.name);
        snap.position[0] += 1.0;
        snap.children.clear();
        snap.scene_path = String::new(); // recomputed by add_object
        Some(self.add_object(snap, parent))
    }

    pub fn clear(&self) {
        self.inner.objects.clear();
        self.inner.children_map.write().clear();
        *self.inner.selected.write() = None;
        *self.inner.gizmo_state.write() = GizmoState::default();
        self.bump_render_revision();
    }

    // ── Gizmo API ─────────────────────────────────────────────────────────

    /// Get the current gizmo state
    pub fn get_gizmo_state(&self) -> GizmoState {
        self.inner.gizmo_state.read().clone()
    }

    /// Set the gizmo type (called when user switches tools)
    pub fn set_gizmo_type(&self, gizmo_type: GizmoType) {
        let mut state = self.inner.gizmo_state.write();
        state.gizmo_type = gizmo_type;
    }

    /// Set the highlighted axis (for hover feedback)
    pub fn set_gizmo_highlighted_axis(&self, axis: Option<GizmoAxis>) {
        let mut state = self.inner.gizmo_state.write();
        state.highlighted_axis = axis;
    }

    /// Set the gizmo scale factor (for camera-distance scaling)
    pub fn set_gizmo_scale(&self, scale: f32) {
        let mut state = self.inner.gizmo_state.write();
        state.scale_factor = scale;
    }

    // ── Helpers ───────────────────────────────────────────────────────────

    /// Return the ordered child ids of `parent_id`, or root ids if `None`.
    pub fn get_children(&self, parent_id: Option<&str>) -> Vec<ObjectId> {
        let key = parent_id.unwrap_or("");
        self.inner
            .children_map
            .read()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// Recursively update scene_path for `id` and all its descendants.
    fn update_subtree_path(&self, id: &str, new_path: &str) {
        if let Some(entry) = self.inner.objects.get(id) {
            entry.meta.write().scene_path = new_path.to_string();
        }
        // entry ref dropped — safe to recurse
        for child_id in self.get_children(Some(id)) {
            let child_name = self
                .inner
                .objects
                .get(&child_id)
                .map(|e| e.meta.read().name.clone())
                .unwrap_or_default();
            let child_path = format!("{}/{}", new_path, child_name);
            self.update_subtree_path(&child_id, &child_path);
        }
    }

    fn is_ancestor_of(&self, potential_ancestor: &str, of: &str) -> bool {
        let mut current = of.to_string();
        loop {
            if let Some(e) = self.inner.objects.get(&current) {
                if let Some(ref pid) = e.meta.read().parent.clone() {
                    if pid == potential_ancestor {
                        return true;
                    }
                    current = pid.clone();
                } else {
                    return false;
                }
            } else {
                return false;
            }
        }
    }

    #[inline]
    fn bump_render_revision(&self) {
        self.inner.render_revision.fetch_add(1, Ordering::Release);
    }
}

impl Default for SceneDb {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object() -> SceneObjectSnapshot {
        SceneObjectSnapshot {
            id: "object".into(),
            name: "Object".into(),
            scene_path: String::new(),
            object_type: ObjectType::Empty,
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            parent: None,
            children: Vec::new(),
            visible: true,
            locked: false,
            props: HashMap::new(),
            component_instances: None,
        }
    }

    #[test]
    fn render_revision_advances_only_when_render_data_changes() {
        let db = SceneDb::new();
        let initial = db.render_revision();
        db.add_object(object(), None);
        let added = db.render_revision();
        assert!(added > initial);

        assert!(db.set_position("object", [0.0; 3]));
        assert_eq!(db.render_revision(), added);
        assert!(db.set_position("object", [1.0, 0.0, 0.0]));
        assert!(db.render_revision() > added);

        let moved = db.render_revision();
        assert!(db.update_render_data("object", |meta| {
            meta.props
                .insert("material".into(), serde_json::json!("rock"));
        }));
        assert!(db.render_revision() > moved);
    }
}
