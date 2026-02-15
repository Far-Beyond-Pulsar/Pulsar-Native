//! Scene Database - wraps the engine_backend atomic SceneDb.
//!
//! The `SceneDatabase` is the UI-layer wrapper that adds undo/redo history
//! and JSON save/load on top of the lock-free `engine_backend::scene::SceneDb`.
//!
//! The renderer holds the SAME `Arc<SceneDb>` directly and reads from it
//! every frame without ever acquiring a lock — transforms are stored as atomics.

use std::path::Path;
use std::sync::Arc;
use std::fs;
use serde::{Deserialize, Serialize};

// Re-export the shared types from engine_backend so downstream code
// that imports from this module still compiles unchanged.
pub use engine_backend::scene::{
    ObjectId, ObjectType, LightType, MeshType, Component, ColliderShape,
    SceneDb, SceneObjectSnapshot,
};

// ─── Transform (kept for backwards compat with existing UI code) ──────────────

/// Per-object transform. Euler angles in degrees.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
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

impl Transform {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_position(mut self, position: [f32; 3]) -> Self { self.position = position; self }
    pub fn with_rotation(mut self, rotation: [f32; 3]) -> Self { self.rotation = rotation; self }
    pub fn with_scale(mut self, scale: [f32; 3]) -> Self { self.scale = scale; self }
}

// ─── SceneObjectData (backwards-compat UI representation) ────────────────────

/// Full UI representation of a scene object — used by panels, undo/redo, and save/load.
/// Reading this from `SceneDatabase` always returns the latest live values from the atomics.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneObjectData {
    pub id: ObjectId,
    pub name: String,
    pub object_type: ObjectType,
    pub transform: Transform,
    pub parent: Option<ObjectId>,
    pub children: Vec<ObjectId>,
    pub visible: bool,
    pub locked: bool,
    pub components: Vec<Component>,
}

impl SceneObjectData {
    /// Convert to the engine_backend snapshot format.
    fn into_snapshot(self) -> SceneObjectSnapshot {
        SceneObjectSnapshot {
            id: self.id,
            name: self.name,
            object_type: self.object_type,
            position: self.transform.position,
            rotation: self.transform.rotation,
            scale: self.transform.scale,
            parent: self.parent,
            children: self.children,
            visible: self.visible,
            locked: self.locked,
            components: self.components,
        }
    }
}

fn snapshot_to_data(s: SceneObjectSnapshot) -> SceneObjectData {
    SceneObjectData {
        id: s.id,
        name: s.name,
        object_type: s.object_type,
        transform: Transform {
            position: s.position,
            rotation: s.rotation,
            scale: s.scale,
        },
        parent: s.parent,
        children: s.children,
        visible: s.visible,
        locked: s.locked,
        components: s.components,
    }
}

// ─── Undo/Redo ────────────────────────────────────────────────────────────────

struct UndoHistory {
    undo_stack: Vec<SceneCommand>,
    redo_stack: Vec<SceneCommand>,
    max_history: usize,
}

#[derive(Clone, Debug)]
enum SceneCommand {
    AddObject { object: SceneObjectData },
    RemoveObject { object: SceneObjectData },
    ModifyObject { old: SceneObjectData, new: SceneObjectData },
    ModifyTransform { object_id: ObjectId, old: Transform, new: Transform },
}

// ─── SceneDatabase ────────────────────────────────────────────────────────────

/// UI-layer scene database. Clone-able — all clones share the same live data.
///
/// Internally holds an `Arc<SceneDb>` from `engine_backend`. The renderer
/// also holds a clone of that same Arc and reads transforms atomically
/// without any coordination with the UI.
#[derive(Clone)]
pub struct SceneDatabase {
    /// The shared, lock-free live data. Also held by the renderer.
    pub db: Arc<SceneDb>,
    /// Undo/redo history — only in the UI layer, not needed by the renderer.
    history: Arc<parking_lot::Mutex<UndoHistory>>,
}

impl SceneDatabase {
    pub fn new() -> Self {
        Self::from_db(Arc::new(SceneDb::new()))
    }

    /// Wrap an existing SceneDb Arc (used when the renderer already created one).
    pub fn from_db(db: Arc<SceneDb>) -> Self {
        Self {
            db,
            history: Arc::new(parking_lot::Mutex::new(UndoHistory {
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
                max_history: 100,
            })),
        }
    }

    /// Build the default scene — the same objects that were in the old hard-coded default.
    pub fn with_default_scene() -> Self {
        let this = Self::new();
        this.populate_default_scene();
        this
    }

    /// Build the default scene on top of an existing SceneDb.
    pub fn with_default_scene_on(db: Arc<SceneDb>) -> Self {
        let this = Self::from_db(db);
        this.populate_default_scene();
        this
    }

    fn populate_default_scene(&self) {
        let mk = |id: &str, name: &str, ot: ObjectType, pos: [f32;3], rot: [f32;3], scale: [f32;3], comps: Vec<Component>| {
            SceneObjectData {
                id: id.to_string(), name: name.to_string(), object_type: ot,
                transform: Transform { position: pos, rotation: rot, scale },
                parent: None, children: Vec::new(), visible: true, locked: false, components: comps,
            }
        };

        // Root objects
        self.add_object(mk("main_camera",     "Main Camera",      ObjectType::Camera,
            [-3.0, 3.0, 6.0], [0.0;3], [1.0;3], vec![]), None);
        self.add_object(mk("directional_light","Directional Light",ObjectType::Light(LightType::Directional),
            [4.0, 8.0, 4.0], [-45.0, 45.0, 0.0], [1.0;3], vec![]), None);

        // Geometry folder
        self.add_object(mk("geometry_folder", "Geometry", ObjectType::Folder,
            [0.0;3], [0.0;3], [1.0;3], vec![]), None);
        self.add_object(mk("cube_red", "Red Cube", ObjectType::Mesh(MeshType::Cube),
            [-2.0, 1.0, 0.0], [0.0;3], [2.0;3],
            vec![Component::Material { id: "red_metal".into(), color: [0.9,0.2,0.2,1.0], metallic: 0.8, roughness: 0.3 }]),
            Some("geometry_folder".into()));

        // Spheres sub-folder
        self.add_object(mk("spheres_folder", "Spheres", ObjectType::Folder,
            [0.0;3], [0.0;3], [1.0;3], vec![]), Some("geometry_folder".into()));
        self.add_object(mk("sphere_blue", "Blue Sphere", ObjectType::Mesh(MeshType::Sphere),
            [2.0, 1.0, 0.0], [0.0;3], [1.0;3],
            vec![Component::Material { id: "blue_metal".into(), color: [0.2,0.5,0.9,1.0], metallic: 0.9, roughness: 0.1 }]),
            Some("spheres_folder".into()));
        self.add_object(mk("sphere_gold", "Gold Sphere", ObjectType::Mesh(MeshType::Sphere),
            [0.0, 3.0, 0.0], [0.0;3], [1.0;3],
            vec![Component::Material { id: "gold_metal".into(), color: [1.0,0.843,0.0,1.0], metallic: 0.95, roughness: 0.2 }]),
            Some("spheres_folder".into()));
        self.add_object(mk("sphere_green", "Green Sphere", ObjectType::Mesh(MeshType::Sphere),
            [4.0, 1.5, 2.0], [0.0;3], [0.8;3],
            vec![Component::Material { id: "green_metal".into(), color: [0.2,0.8,0.3,1.0], metallic: 0.7, roughness: 0.4 }]),
            Some("spheres_folder".into()));

        // Lights folder
        self.add_object(mk("lights_folder", "Lights", ObjectType::Folder,
            [0.0;3], [0.0;3], [1.0;3], vec![]), None);
        self.add_object(mk("point_light_1", "Point Light", ObjectType::Light(LightType::Point),
            [0.0, 5.0, 0.0], [0.0;3], [1.0;3], vec![]), Some("lights_folder".into()));
        self.add_object(mk("spot_light_1", "Spot Light", ObjectType::Light(LightType::Spot),
            [-5.0, 6.0, 3.0], [-30.0, 45.0, 0.0], [1.0;3], vec![]), Some("lights_folder".into()));

        // Audio folder
        self.add_object(mk("audio_folder", "Audio", ObjectType::Folder,
            [0.0;3], [0.0;3], [1.0;3], vec![]), None);
        self.add_object(mk("ambient_audio", "Ambient Sound", ObjectType::AudioSource,
            [0.0, 2.0, 0.0], [0.0;3], [1.0;3], vec![]), Some("audio_folder".into()));

        // Effects folder
        self.add_object(mk("effects_folder", "Effects", ObjectType::Folder,
            [0.0;3], [0.0;3], [1.0;3], vec![]), None);
        self.add_object(mk("particles_fire", "Fire Particles", ObjectType::ParticleSystem,
            [3.0, 0.5, -2.0], [0.0;3], [1.0;3], vec![]), Some("effects_folder".into()));
    }

    // ── Object creation / deletion ────────────────────────────────────────

    pub fn add_object(&self, object: SceneObjectData, parent_id: Option<ObjectId>) -> ObjectId {
        let mut snap = object.clone().into_snapshot();
        snap.parent = parent_id.clone();

        let mut h = self.history.lock();
        h.undo_stack.push(SceneCommand::AddObject { object });
        h.redo_stack.clear();
        if h.undo_stack.len() > h.max_history { h.undo_stack.remove(0); }
        drop(h);

        self.db.add_object(snap, parent_id)
    }

    pub fn remove_object(&self, object_id: &ObjectId) -> bool {
        if let Some(snap) = self.db.get_object(object_id) {
            let object = snapshot_to_data(snap);
            let removed = self.db.remove_object(object_id);
            if removed {
                let mut h = self.history.lock();
                h.undo_stack.push(SceneCommand::RemoveObject { object });
                h.redo_stack.clear();
            }
            removed
        } else {
            false
        }
    }

    // ── Reads ─────────────────────────────────────────────────────────────

    pub fn get_object(&self, id: &ObjectId) -> Option<SceneObjectData> {
        self.db.get_object(id).map(snapshot_to_data)
    }

    pub fn get_root_objects(&self) -> Vec<SceneObjectData> {
        self.db.get_root_snapshots().into_iter().map(snapshot_to_data).collect()
    }

    pub fn get_all_objects(&self) -> Vec<SceneObjectData> {
        self.db.get_all_snapshots().into_iter().map(snapshot_to_data).collect()
    }

    // ── Selection ─────────────────────────────────────────────────────────

    pub fn select_object(&self, id: Option<ObjectId>) {
        self.db.select_object(id);
    }

    pub fn get_selected_object(&self) -> Option<SceneObjectData> {
        self.db.get_selected().map(snapshot_to_data)
    }

    pub fn get_selected_object_id(&self) -> Option<ObjectId> {
        self.db.get_selected_id()
    }

    // ── Transform writes (atomic — no lock in the renderer) ───────────────

    pub fn update_transform(&self, id: &ObjectId, t: Transform) -> bool {
        if let Some(old_snap) = self.db.get_object(id) {
            let old = Transform {
                position: old_snap.position,
                rotation: old_snap.rotation,
                scale: old_snap.scale,
            };
            let ok = self.db.apply_transform(id, t.position, t.rotation, t.scale);
            if ok {
                let mut h = self.history.lock();
                h.undo_stack.push(SceneCommand::ModifyTransform {
                    object_id: id.clone(), old, new: t,
                });
                h.redo_stack.clear();
            }
            ok
        } else {
            false
        }
    }

    /// Update all fields of an object (used by the properties panel bound fields).
    pub fn update_object(&self, object: SceneObjectData) -> bool {
        if let Some(old_snap) = self.db.get_object(&object.id) {
            let old = snapshot_to_data(old_snap);
            // Apply atomic hot-path updates
            let id = &object.id;
            self.db.apply_transform(id, object.transform.position, object.transform.rotation, object.transform.scale);
            self.db.set_visible(id, object.visible);
            self.db.set_locked(id, object.locked);
            // Apply cold updates
            self.db.set_name(id, object.name.clone());
            // Update children/parent via full snapshot
            if let Some(entry) = self.db.get_entry(id) {
                let mut meta = entry.meta.write();
                meta.components = object.components.clone();
            }
            let mut h = self.history.lock();
            h.undo_stack.push(SceneCommand::ModifyObject { old, new: object });
            h.redo_stack.clear();
            true
        } else {
            false
        }
    }

    // ── Structural operations ─────────────────────────────────────────────

    pub fn reparent_object(&self, id: &str, new_parent: Option<String>) -> bool {
        self.db.reparent_object(id, new_parent)
    }

    pub fn duplicate_object(&self, id: &ObjectId) -> Option<ObjectId> {
        self.db.duplicate_object(id)
    }

    pub fn move_object_up(&self, _id: &str) -> bool {
        // Order within parent's children list — delegate to db roots/meta
        // For now a no-op; root ordering can be added to SceneDb later
        false
    }

    pub fn move_object_down(&self, _id: &str) -> bool {
        false
    }

    // ── Undo / Redo ───────────────────────────────────────────────────────

    pub fn undo(&self) -> bool {
        let mut h = self.history.lock();
        if let Some(cmd) = h.undo_stack.pop() {
            match cmd.clone() {
                SceneCommand::AddObject { object } => {
                    drop(h);
                    self.db.remove_object(&object.id);
                }
                SceneCommand::RemoveObject { object } => {
                    let parent = object.parent.clone();
                    let snap = object.into_snapshot();
                    drop(h);
                    self.db.add_object(snap, parent);
                }
                SceneCommand::ModifyObject { old, .. } => {
                    drop(h);
                    let id = old.id.clone();
                    self.db.apply_transform(&id, old.transform.position, old.transform.rotation, old.transform.scale);
                    self.db.set_visible(&id, old.visible);
                    self.db.set_name(&id, old.name);
                }
                SceneCommand::ModifyTransform { object_id, old, .. } => {
                    drop(h);
                    self.db.apply_transform(&object_id, old.position, old.rotation, old.scale);
                }
            }
            // Push to redo — need to re-lock
            let mut h = self.history.lock();
            h.redo_stack.push(cmd);
            true
        } else {
            false
        }
    }

    pub fn redo(&self) -> bool {
        let mut h = self.history.lock();
        if let Some(cmd) = h.redo_stack.pop() {
            match cmd.clone() {
                SceneCommand::AddObject { object } => {
                    let parent = object.parent.clone();
                    let snap = object.into_snapshot();
                    drop(h);
                    self.db.add_object(snap, parent);
                }
                SceneCommand::RemoveObject { object } => {
                    drop(h);
                    self.db.remove_object(&object.id);
                }
                SceneCommand::ModifyObject { new, .. } => {
                    drop(h);
                    let id = new.id.clone();
                    self.db.apply_transform(&id, new.transform.position, new.transform.rotation, new.transform.scale);
                    self.db.set_visible(&id, new.visible);
                    self.db.set_name(&id, new.name);
                }
                SceneCommand::ModifyTransform { object_id, new, .. } => {
                    drop(h);
                    self.db.apply_transform(&object_id, new.position, new.rotation, new.scale);
                }
            }
            let mut h = self.history.lock();
            h.undo_stack.push(cmd);
            true
        } else {
            false
        }
    }

    // ── Scene management ──────────────────────────────────────────────────

    pub fn clear(&self) {
        self.db.clear();
        let mut h = self.history.lock();
        h.undo_stack.clear();
        h.redo_stack.clear();
    }

    /// Save all current scene objects to JSON.
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let objects: Vec<SceneObjectData> = self.get_all_objects();
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let now = chrono::Utc::now().to_rfc3339();
        let scene_file = SceneFile {
            version: "1.0".into(),
            objects,
            metadata: SceneMetadata {
                created: now.clone(),
                modified: now,
                editor_version: "0.1.0".into(),
            },
        };
        let json = serde_json::to_string_pretty(&scene_file).map_err(|e| format!("serialize: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("write: {e}"))?;
        tracing::debug!("[SCENE-DB] 💾 Saved to {:?}", path.as_ref());
        Ok(())
    }

    /// Load scene from JSON, clearing current content first.
    pub fn load_from_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let json = fs::read_to_string(&path).map_err(|e| format!("read: {e}"))?;
        let scene_file: SceneFile = serde_json::from_str(&json).map_err(|e| format!("parse: {e}"))?;
        self.clear();
        for obj in scene_file.objects {
            let parent = obj.parent.clone();
            let snap = obj.into_snapshot();
            self.db.add_object(snap, parent);
        }
        tracing::debug!("[SCENE-DB] 📂 Loaded from {:?}", path.as_ref());
        Ok(())
    }
}

// ─── Scene file format ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneFile {
    pub version: String,
    pub objects: Vec<SceneObjectData>,
    pub metadata: SceneMetadata,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneMetadata {
    pub created: String,
    pub modified: String,
    pub editor_version: String,
}
