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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use dashmap::DashMap;
use glam::{Mat4, Vec3};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

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
            Component::Material { id, color, metallic, roughness } => vec![
                ComponentFieldMetadata::String { name: "id", value: id },
                ComponentFieldMetadata::Color { name: "color", value: color },
                ComponentFieldMetadata::F32 { name: "metallic", value: metallic },
                ComponentFieldMetadata::F32 { name: "roughness", value: roughness },
            ],
            Component::Script { path } => vec![
                ComponentFieldMetadata::String { name: "path", value: path },
            ],
            Component::Collider { shape } => vec![
                // TODO: Handle nested enums like ColliderShape
            ],
            Component::RigidBody { mass, kinematic } => vec![
                ComponentFieldMetadata::F32 { name: "mass", value: mass },
                ComponentFieldMetadata::Bool { name: "kinematic", value: kinematic },
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
            _ => {},
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
            _ => {},
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
            _ => {},
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
            _ => {},
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
    F32 { name: &'static str, value: &'a f32 },
    Bool { name: &'static str, value: &'a bool },
    String { name: &'static str, value: &'a String },
    Vec3 { name: &'static str, value: &'a [f32; 3] },
    Color { name: &'static str, value: &'a [f32; 4] },
}

impl<'a> ComponentFieldMetadata<'a> {
    pub fn name(&self) -> &'static str {
        match self {
            ComponentFieldMetadata::F32 { name, .. } => name,
            ComponentFieldMetadata::Bool { name, .. } => name,
            ComponentFieldMetadata::String { name, .. } => name,
            ComponentFieldMetadata::Vec3 { name, .. } => name,
            ComponentFieldMetadata::Color { name, .. } => name,
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
    pub object_type: ObjectType,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub parent: Option<ObjectId>,
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
    pub children: Vec<ObjectId>,
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
                children: snap.children.clone(),
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
    pub fn snapshot(&self) -> SceneObjectSnapshot {
        let meta = self.meta.read();
        SceneObjectSnapshot {
            id: self.id.clone(),
            name: meta.name.clone(),
            object_type: self.object_type,
            position: self.get_position(),
            rotation: self.get_rotation(),
            scale: self.get_scale(),
            parent: meta.parent.clone(),
            children: meta.children.clone(),
            visible: self.is_visible(),
            locked: self.is_locked(),
            components: meta.components.clone(),
        }
    }
}

// ─── SceneDb ─────────────────────────────────────────────────────────────────

struct SceneDbInner {
    /// All objects — concurrent reads, no global lock.
    objects: DashMap<ObjectId, Arc<SceneEntry>>,
    /// Root-level object ordering — only locked for structural changes.
    roots: RwLock<Vec<ObjectId>>,
    /// Auto-incrementing id counter.
    next_id: AtomicU64,
    /// Currently selected object id.
    selected: RwLock<Option<ObjectId>>,
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
                roots: RwLock::new(Vec::new()),
                next_id: AtomicU64::new(1),
                selected: RwLock::new(None),
            }),
        }
    }

    // ── Object creation / deletion ────────────────────────────────────────

    /// Add an object. Returns its id.
    pub fn add_object(&self, mut snap: SceneObjectSnapshot, parent_id: Option<ObjectId>) -> ObjectId {
        // Assign id if empty
        if snap.id.is_empty() {
            let n = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
            snap.id = format!("object_{}", n);
        }
        let id = snap.id.clone();
        snap.parent = parent_id.clone();

        // Update parent's children list
        if let Some(ref pid) = parent_id {
            if let Some(parent_entry) = self.inner.objects.get(pid) {
                parent_entry.meta.write().children.push(id.clone());
            }
        } else {
            self.inner.roots.write().push(id.clone());
        }

        self.inner.objects.insert(id.clone(), Arc::new(SceneEntry::new(&snap)));
        id
    }

    /// Remove an object (and recursively its children).
    pub fn remove_object(&self, id: &str) -> bool {
        if let Some((_, entry)) = self.inner.objects.remove(id) {
            // Detach from parent or roots
            {
                let meta = entry.meta.read();
                if let Some(ref pid) = meta.parent {
                    if let Some(p) = self.inner.objects.get(pid) {
                        p.meta.write().children.retain(|c| c != id);
                    }
                } else {
                    self.inner.roots.write().retain(|r| r != id);
                }
                // Recurse into children (clone to avoid holding meta read lock during recursion)
                let children = meta.children.clone();
                drop(meta);
                for child in children {
                    self.remove_object(&child);
                }
            }
            // Deselect if needed
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
        self.inner.objects.get(id).map(|e| e.snapshot())
    }

    /// Get a direct Arc to the live entry. The renderer uses this for atomic transform reads.
    pub fn get_entry(&self, id: &str) -> Option<Arc<SceneEntry>> {
        self.inner.objects.get(id).map(|e| e.clone())
    }

    pub fn get_root_snapshots(&self) -> Vec<SceneObjectSnapshot> {
        let roots = self.inner.roots.read();
        roots.iter()
            .filter_map(|id| self.inner.objects.get(id).map(|e| e.snapshot()))
            .collect()
    }

    pub fn get_all_snapshots(&self) -> Vec<SceneObjectSnapshot> {
        self.inner.objects.iter().map(|e| e.snapshot()).collect()
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
        if let Some(e) = self.inner.objects.get(id) {
            e.meta.write().name = name;
            true
        } else {
            false
        }
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

        let old_parent = {
            if let Some(e) = self.inner.objects.get(id) {
                e.meta.read().parent.clone()
            } else {
                return false;
            }
        };

        // Remove from old parent
        if let Some(ref old_pid) = old_parent {
            if let Some(p) = self.inner.objects.get(old_pid) {
                p.meta.write().children.retain(|c| c != id);
            }
        } else {
            self.inner.roots.write().retain(|r| r != id);
        }

        // Add to new parent
        if let Some(ref new_pid) = new_parent {
            if let Some(p) = self.inner.objects.get(new_pid) {
                p.meta.write().children.push(id.to_string());
            } else {
                return false;
            }
        } else {
            self.inner.roots.write().push(id.to_string());
        }

        // Update the object's own parent field
        if let Some(e) = self.inner.objects.get(id) {
            e.meta.write().parent = new_parent;
        }
        true
    }

    pub fn duplicate_object(&self, id: &str) -> Option<ObjectId> {
        let mut snap = self.get_object(id)?;
        let parent = snap.parent.clone();
        snap.id = String::new(); // auto-assign
        snap.name = format!("{} Copy", snap.name);
        snap.position[0] += 1.0;
        snap.children.clear();
        Some(self.add_object(snap, parent))
    }

    pub fn clear(&self) {
        self.inner.objects.clear();
        self.inner.roots.write().clear();
        *self.inner.selected.write() = None;
    }

    // ── Helpers ───────────────────────────────────────────────────────────

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
