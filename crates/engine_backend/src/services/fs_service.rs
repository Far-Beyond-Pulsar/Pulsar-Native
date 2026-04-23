//! Shared scene database for the Pulsar engine.
//!
//! This module provides a single `SceneDb` that is shared by:
//! - The Helio renderer (reads transforms lock-free via atomics)
//! - The hierarchy panel (reads/writes object list)
//! - The properties panel (writes transforms atomically)
//!
//! ## Design
//!
//! Hot-path data (transforms, visibility) is stored as atomics so the
//! render thread never blocks on a lock. Structural changes (add/remove objects,
//! reparenting) use a fast `parking_lot::RwLock` that is only held briefly.
//!
//! Object storage uses `dashmap::DashMap` which provides concurrent access
//! without a global lock — reads on different shards proceed in parallel.

use dashmap::DashMap;
use glam::{Mat4, Vec3};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Component {
    Material {
        id: String,
        color: [f32; 4],
        metallic: f32,
        roughness: f32,
    },
    Script {
        path: String,
    },
    Collider {
        shape: ColliderShape,
    },
    RigidBody {
        mass: f32,
        kinematic: bool,
    },
}

impl Component {
    /// Get field metadata for this component variant
    pub fn get_field_metadata(&self) -> Vec<ComponentFieldMetadata> {
        match self {
            Component::Material {
                id,
                color,
                metallic,
                roughness,
            } => vec![
                ComponentFieldMetadata::String {
                    name: "id",
                    value: id,
                },
                ComponentFieldMetadata::Color {
                    name: "color",
                    value: color,
                },
                ComponentFieldMetadata::F32 {
                    name: "metallic",
                    value: metallic,
                },
                ComponentFieldMetadata::F32 {
                    name: "roughness",
                    value: roughness,
                },
            ],
            Component::Script { path } => vec![ComponentFieldMetadata::String {
                name: "path",
                value: path,
            }],
            Component::Collider { shape } => vec![
                // TODO: Handle nested enums like ColliderShape
            ],
            Component::RigidBody { mass, kinematic } => vec![
                ComponentFieldMetadata::F32 {
                    name: "mass",
                    value: mass,
                },
                ComponentFieldMetadata::Bool {
                    name: "kinematic",
                    value: kinematic,
                },
            ],
        }
    }

    /// Get a field value by name and type
    pub fn get_field_f32(&self, field_name: &str) -> Option<f32> {
        match (self, field_name) {
            (Component::Material { metallic, .. }, "metallic") => Some(*metallic),
            (Component::Material { roughness, .. }, "roughness") => Some(*roughness),
            (Component::RigidBody { mass, .. }, "mass") => Some(*mass),
            _ => None,
        }
    }

    pub fn set_field_f32(&mut self, field_name: &str, value: f32) {
        match (self, field_name) {
            (Component::Material { metallic, .. }, "metallic") => *metallic = value,
            (Component::Material { roughness, .. }, "roughness") => *roughness = value,
            (Component::RigidBody { mass, .. }, "mass") => *mass = value,
            _ => {}
        }
    }

    pub fn get_field_bool(&self, field_name: &str) -> Option<bool> {
        match (self, field_name) {
            (Component::RigidBody { kinematic, .. }, "kinematic") => Some(*kinematic),
            _ => None,
        }
    }

    pub fn set_field_bool(&mut self, field_name: &str, value: bool) {
        match (self, field_name) {
            (Component::RigidBody { kinematic, .. }, "kinematic") => *kinematic = value,
            _ => {}
        }
    }

    pub fn get_field_string(&self, field_name: &str) -> Option<String> {
        match (self, field_name) {
            (Component::Material { id, .. }, "id") => Some(id.clone()),
            (Component::Script { path }, "path") => Some(path.clone()),
            _ => None,
        }
    }

    pub fn set_field_string(&mut self, field_name: &str, value: String) {
        match (self, field_name) {
            (Component::Material { id, .. }, "id") => *id = value,
            (Component::Script { path }, "path") => *path = value,
            _ => {}
        }
    }

    pub fn get_field_color_component(&self, field_name: &str, index: usize) -> Option<f32> {
        match (self, field_name) {
            (Component::Material { color, .. }, "color") if index < 4 => Some(color[index]),
            _ => None,
        }
    }

    pub fn set_field_color_component(&mut self, field_name: &str, index: usize, value: f32) {
        match (self, field_name) {
            (Component::Material { color, .. }, "color") if index < 4 => color[index] = value,
            _ => {}
        }
    }

    pub fn variant_name(&self) -> &'static str {
        match self {
            Component::Material { .. } => "Material",
            Component::Script { .. } => "Script",
            Component::Collider { .. } => "Collider",
            Component::RigidBody { .. } => "RigidBody",
        }
    }
}

/// Metadata for a component field - describes type and provides a reference
pub enum ComponentFieldMetadata<'a> {
    F32 {
        name: &'static str,
        value: &'a f32,
    },
    Bool {
        name: &'static str,
        value: &'a bool,
    },
    String {
        name: &'static str,
        value: &'a String,
    },
    Vec3 {
        name: &'static str,
        value: &'a [f32; 3],
    },
    Color {
        name: &'static str,
        value: &'a [f32; 4],
    },
    /// Custom field type - requires special rendering in UI layer
    /// The ui_key is used to look up the custom renderer
    Custom {
        name: &'static str,
        type_name: &'static str,
        ui_key: &'static str,
        value_ptr: *const (), // Type-erased pointer to the value
    },
}

impl<'a> ComponentFieldMetadata<'a> {
    pub fn name(&self) -> &'static str {
        match self {
            ComponentFieldMetadata::F32 { name, .. } => name,
            ComponentFieldMetadata::Bool { name, .. } => name,
            ComponentFieldMetadata::String { name, .. } => name,
            ComponentFieldMetadata::Vec3 { name, .. } => name,
            ComponentFieldMetadata::Color { name, .. } => name,
            ComponentFieldMetadata::Custom { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ColliderShape {
    Box { size: [f32; 3] },
    Sphere { radius: f32 },
    Capsule { radius: f32, height: f32 },
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
    pub components: Vec<Component>,
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
    pub components: Vec<Component>,
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
                components: snap.components.clone(),
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
    pub fn set_position(&self, v: [f32; 3]) {
        self.position[0].store(v[0].to_bits(), Ordering::Relaxed);
        self.position[1].store(v[1].to_bits(), Ordering::Relaxed);
        self.position[2].store(v[2].to_bits(), Ordering::Relaxed);
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
    pub fn set_rotation(&self, v: [f32; 3]) {
        self.rotation[0].store(v[0].to_bits(), Ordering::Relaxed);
        self.rotation[1].store(v[1].to_bits(), Ordering::Relaxed);
        self.rotation[2].store(v[2].to_bits(), Ordering::Relaxed);
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
    pub fn set_scale(&self, v: [f32; 3]) {
        self.scale[0].store(v[0].to_bits(), Ordering::Relaxed);
        self.scale[1].store(v[1].to_bits(), Ordering::Relaxed);
        self.scale[2].store(v[2].to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_visible(&self, v: bool) {
        self.visible.store(v, Ordering::Relaxed);
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_locked(&self, v: bool) {
        self.locked.store(v, Ordering::Relaxed);
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
            components: meta.components.clone(),
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
            f(&*entry);
        }
    }

    // ── Atomic transform writes (called by props panel) ───────────────────

    pub fn set_position(&self, id: &str, v: [f32; 3]) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            e.set_position(v);
            true
        } else {
            false
        }
    }

    pub fn set_rotation(&self, id: &str, v: [f32; 3]) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            e.set_rotation(v);
            true
        } else {
            false
        }
    }

    pub fn set_scale(&self, id: &str, v: [f32; 3]) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            e.set_scale(v);
            true
        } else {
            false
        }
    }

    pub fn set_visible(&self, id: &str, v: bool) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            e.set_visible(v);
            true
        } else {
            false
        }
    }

    /// Update all three transform components at once from a snapshot.
    pub fn apply_transform(&self, id: &str, pos: [f32; 3], rot: [f32; 3], scale: [f32; 3]) -> bool {
        if let Some(e) = self.inner.objects.get(id) {
            e.set_position(pos);
            e.set_rotation(rot);
            e.set_scale(scale);
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
}

impl Default for SceneDb {
    fn default() -> Self {
        Self::new()
    }
}
