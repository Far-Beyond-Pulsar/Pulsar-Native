//! Scene Database - In-memory scene management system
//! 
//! This module provides a centralized, thread-safe scene database that manages
//! all objects in the level editor. It handles:
//! - Object creation, deletion, modification
//! - Hierarchical relationships (parent/child)
//! - Transform management
//! - Object selection state
//! - Undo/Redo history
//! - Syncing with the Bevy renderer

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use serde::{Deserialize, Serialize};

pub type ObjectId = String;

/// The central scene database - thread-safe and shared across the editor
#[derive(Clone)]
pub struct SceneDatabase {
    inner: Arc<RwLock<SceneDatabaseInner>>,
}

struct SceneDatabaseInner {
    /// All objects in the scene, indexed by ID
    objects: HashMap<ObjectId, SceneObjectData>,
    /// Root-level object IDs (objects with no parent)
    root_objects: Vec<ObjectId>,
    /// Currently selected object ID
    selected_object: Option<ObjectId>,
    /// Undo/redo history
    history: UndoHistory,
    /// Counter for generating unique IDs
    next_id: u64,
}

/// Full scene object data stored in the database
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
    /// Component data (materials, scripts, etc.)
    pub components: Vec<Component>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    Empty,
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 3],  // Euler angles in degrees
    pub scale: [f32; 3],
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ColliderShape {
    Box { size: [f32; 3] },
    Sphere { radius: f32 },
    Capsule { radius: f32, height: f32 },
}

/// Undo/Redo history management
struct UndoHistory {
    undo_stack: Vec<SceneCommand>,
    redo_stack: Vec<SceneCommand>,
    max_history: usize,
}

#[derive(Clone, Debug)]
enum SceneCommand {
    AddObject {
        object: SceneObjectData,
    },
    RemoveObject {
        object: SceneObjectData,
    },
    ModifyObject {
        old_object: SceneObjectData,
        new_object: SceneObjectData,
    },
    ModifyTransform {
        object_id: ObjectId,
        old_transform: Transform,
        new_transform: Transform,
    },
}

impl SceneDatabase {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(SceneDatabaseInner {
                objects: HashMap::new(),
                root_objects: Vec::new(),
                selected_object: None,
                history: UndoHistory {
                    undo_stack: Vec::new(),
                    redo_stack: Vec::new(),
                    max_history: 100,
                },
                next_id: 1,
            })),
        }
    }

    /// Create a new scene database with default objects
    pub fn with_default_scene() -> Self {
        let db = Self::new();
        
        // Add default camera
        db.add_object(SceneObjectData {
            id: "main_camera".to_string(),
            name: "Main Camera".to_string(),
            object_type: ObjectType::Camera,
            transform: Transform {
                position: [-3.0, 3.0, 6.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            },
            parent: None,
            children: Vec::new(),
            visible: true,
            locked: false,
            components: Vec::new(),
        }, None);

        // Add directional light
        db.add_object(SceneObjectData {
            id: "directional_light".to_string(),
            name: "Directional Light".to_string(),
            object_type: ObjectType::Light(LightType::Directional),
            transform: Transform {
                position: [4.0, 8.0, 4.0],
                rotation: [-45.0, 45.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            },
            parent: None,
            children: Vec::new(),
            visible: true,
            locked: false,
            components: Vec::new(),
        }, None);

        // Add red cube (matches Bevy renderer)
        db.add_object(SceneObjectData {
            id: "cube_red".to_string(),
            name: "Red Cube".to_string(),
            object_type: ObjectType::Mesh(MeshType::Cube),
            transform: Transform {
                position: [-2.0, 1.0, 0.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [2.0, 2.0, 2.0],
            },
            parent: None,
            children: Vec::new(),
            visible: true,
            locked: false,
            components: vec![Component::Material {
                id: "red_metal".to_string(),
                color: [0.9, 0.2, 0.2, 1.0],
                metallic: 0.8,
                roughness: 0.3,
            }],
        }, None);

        // Add blue sphere (matches Bevy renderer)
        db.add_object(SceneObjectData {
            id: "sphere_blue".to_string(),
            name: "Blue Sphere".to_string(),
            object_type: ObjectType::Mesh(MeshType::Sphere),
            transform: Transform {
                position: [2.0, 1.0, 0.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            },
            parent: None,
            children: Vec::new(),
            visible: true,
            locked: false,
            components: vec![Component::Material {
                id: "blue_metal".to_string(),
                color: [0.2, 0.5, 0.9, 1.0],
                metallic: 0.9,
                roughness: 0.1,
            }],
        }, None);

        // Add gold sphere (matches Bevy renderer)
        db.add_object(SceneObjectData {
            id: "sphere_gold".to_string(),
            name: "Gold Sphere".to_string(),
            object_type: ObjectType::Mesh(MeshType::Sphere),
            transform: Transform {
                position: [0.0, 3.0, 0.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            },
            parent: None,
            children: Vec::new(),
            visible: true,
            locked: false,
            components: vec![Component::Material {
                id: "gold_metal".to_string(),
                color: [1.0, 0.843, 0.0, 1.0],
                metallic: 0.95,
                roughness: 0.2,
            }],
        }, None);

        // Add green sphere (matches Bevy renderer)
        db.add_object(SceneObjectData {
            id: "sphere_green".to_string(),
            name: "Green Sphere".to_string(),
            object_type: ObjectType::Mesh(MeshType::Sphere),
            transform: Transform {
                position: [0.0, 1.0, 2.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            },
            parent: None,
            children: Vec::new(),
            visible: true,
            locked: false,
            components: vec![Component::Material {
                id: "green_metal".to_string(),
                color: [0.2, 0.9, 0.3, 1.0],
                metallic: 0.6,
                roughness: 0.4,
            }],
        }, None);

        db
    }

    /// Add a new object to the scene
    pub fn add_object(&self, mut object: SceneObjectData, parent_id: Option<ObjectId>) -> ObjectId {
        let mut inner = self.inner.write().unwrap();
        
        // Generate unique ID if not provided
        if object.id.is_empty() {
            object.id = format!("object_{}", inner.next_id);
            inner.next_id += 1;
        }

        let id = object.id.clone();
        object.parent = parent_id.clone();

        // Add to parent's children if parent exists
        if let Some(ref parent_id) = parent_id {
            if let Some(parent) = inner.objects.get_mut(parent_id) {
                parent.children.push(id.clone());
            }
        } else {
            // Add to root objects if no parent
            inner.root_objects.push(id.clone());
        }

        // Add to history
        inner.history.undo_stack.push(SceneCommand::AddObject {
            object: object.clone(),
        });
        inner.history.redo_stack.clear();

        // Add object to database
        inner.objects.insert(id.clone(), object);

        id
    }

    /// Remove an object from the scene
    pub fn remove_object(&self, object_id: &ObjectId) -> bool {
        let mut inner = self.inner.write().unwrap();

        if let Some(object) = inner.objects.remove(object_id) {
            // Remove from parent's children or root objects
            if let Some(ref parent_id) = object.parent {
                if let Some(parent) = inner.objects.get_mut(parent_id) {
                    parent.children.retain(|id| id != object_id);
                }
            } else {
                inner.root_objects.retain(|id| id != object_id);
            }

            // Recursively remove children
            let children = object.children.clone();
            for child_id in children {
                self.remove_object(&child_id);
            }

            // Add to history
            inner.history.undo_stack.push(SceneCommand::RemoveObject {
                object,
            });
            inner.history.redo_stack.clear();

            // Deselect if this was the selected object
            if inner.selected_object.as_ref() == Some(object_id) {
                inner.selected_object = None;
            }

            true
        } else {
            false
        }
    }

    /// Get an object by ID
    pub fn get_object(&self, object_id: &ObjectId) -> Option<SceneObjectData> {
        let inner = self.inner.read().unwrap();
        inner.objects.get(object_id).cloned()
    }

    /// Get all root-level objects
    pub fn get_root_objects(&self) -> Vec<SceneObjectData> {
        let inner = self.inner.read().unwrap();
        inner.root_objects.iter()
            .filter_map(|id| inner.objects.get(id).cloned())
            .collect()
    }

    /// Get all objects (flat list)
    pub fn get_all_objects(&self) -> Vec<SceneObjectData> {
        let inner = self.inner.read().unwrap();
        inner.objects.values().cloned().collect()
    }

    /// Update an object's transform
    pub fn update_transform(&self, object_id: &ObjectId, new_transform: Transform) -> bool {
        let mut inner = self.inner.write().unwrap();

        if let Some(object) = inner.objects.get_mut(object_id) {
            let old_transform = object.transform;
            object.transform = new_transform;

            // Add to history
            inner.history.undo_stack.push(SceneCommand::ModifyTransform {
                object_id: object_id.clone(),
                old_transform,
                new_transform,
            });
            inner.history.redo_stack.clear();

            true
        } else {
            false
        }
    }

    /// Update an entire object
    pub fn update_object(&self, object: SceneObjectData) -> bool {
        let mut inner = self.inner.write().unwrap();

        if let Some(old_object) = inner.objects.get(&object.id).cloned() {
            inner.objects.insert(object.id.clone(), object.clone());

            // Add to history
            inner.history.undo_stack.push(SceneCommand::ModifyObject {
                old_object,
                new_object: object,
            });
            inner.history.redo_stack.clear();

            true
        } else {
            false
        }
    }

    /// Select an object
    pub fn select_object(&self, object_id: Option<ObjectId>) {
        let mut inner = self.inner.write().unwrap();
        inner.selected_object = object_id;
    }

    /// Get the currently selected object
    pub fn get_selected_object(&self) -> Option<SceneObjectData> {
        let inner = self.inner.read().unwrap();
        inner.selected_object.as_ref()
            .and_then(|id| inner.objects.get(id).cloned())
    }

    /// Get the currently selected object ID
    pub fn get_selected_object_id(&self) -> Option<ObjectId> {
        let inner = self.inner.read().unwrap();
        inner.selected_object.clone()
    }

    /// Duplicate an object
    pub fn duplicate_object(&self, object_id: &ObjectId) -> Option<ObjectId> {
        let object = self.get_object(object_id)?;
        
        let mut new_object = object.clone();
        new_object.id = String::new(); // Will be auto-generated
        new_object.name = format!("{} Copy", object.name);
        // Offset position slightly
        new_object.transform.position[0] += 1.0;
        
        Some(self.add_object(new_object, object.parent.clone()))
    }

    /// Undo the last operation
    pub fn undo(&self) -> bool {
        let mut inner = self.inner.write().unwrap();

        if let Some(command) = inner.history.undo_stack.pop() {
            match command.clone() {
                SceneCommand::AddObject { object } => {
                    // Undo add by removing
                    inner.objects.remove(&object.id);
                    if object.parent.is_none() {
                        inner.root_objects.retain(|id| id != &object.id);
                    }
                },
                SceneCommand::RemoveObject { object } => {
                    // Undo remove by adding back
                    if object.parent.is_none() {
                        inner.root_objects.push(object.id.clone());
                    }
                    inner.objects.insert(object.id.clone(), object);
                },
                SceneCommand::ModifyObject { old_object, .. } => {
                    // Undo modify by restoring old object
                    inner.objects.insert(old_object.id.clone(), old_object);
                },
                SceneCommand::ModifyTransform { object_id, old_transform, .. } => {
                    // Undo transform by restoring old transform
                    if let Some(object) = inner.objects.get_mut(&object_id) {
                        object.transform = old_transform;
                    }
                },
            }

            inner.history.redo_stack.push(command);
            true
        } else {
            false
        }
    }

    /// Redo the last undone operation
    pub fn redo(&self) -> bool {
        let mut inner = self.inner.write().unwrap();

        if let Some(command) = inner.history.redo_stack.pop() {
            match command.clone() {
                SceneCommand::AddObject { object } => {
                    // Redo add
                    if object.parent.is_none() {
                        inner.root_objects.push(object.id.clone());
                    }
                    inner.objects.insert(object.id.clone(), object);
                },
                SceneCommand::RemoveObject { object } => {
                    // Redo remove
                    inner.objects.remove(&object.id);
                    if object.parent.is_none() {
                        inner.root_objects.retain(|id| id != &object.id);
                    }
                },
                SceneCommand::ModifyObject { new_object, .. } => {
                    // Redo modify
                    inner.objects.insert(new_object.id.clone(), new_object);
                },
                SceneCommand::ModifyTransform { object_id, new_transform, .. } => {
                    // Redo transform
                    if let Some(object) = inner.objects.get_mut(&object_id) {
                        object.transform = new_transform;
                    }
                },
            }

            inner.history.undo_stack.push(command);
            true
        } else {
            false
        }
    }

    /// Clear all objects from the scene
    pub fn clear(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.objects.clear();
        inner.root_objects.clear();
        inner.selected_object = None;
        inner.history.undo_stack.clear();
        inner.history.redo_stack.clear();
    }
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

    pub fn with_position(mut self, position: [f32; 3]) -> Self {
        self.position = position;
        self
    }

    pub fn with_rotation(mut self, rotation: [f32; 3]) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: [f32; 3]) -> Self {
        self.scale = scale;
        self
    }
}
