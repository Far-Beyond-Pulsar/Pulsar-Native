//! Production Scene Database
//!
//! Primary scene storage backed by the concurrency-safe `SceneDb` (atomic
//! transforms, lock-free renderer reads) with an additional `SceneMetadataDb`
//! layer for the reflection-based component system.

use engine_backend::scene::SceneObjectSnapshot;
use engine_backend::{ComponentInstance, EditorObjectId, SceneMetadataDb};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::Arc;

// ── Public re-exports for UI layer compatibility ───────────────────────────

pub use engine_backend::scene::{Component, LightType, MeshType, ObjectId, ObjectType, SceneDb};

// ── Transform ─────────────────────────────────────────────────────────────

/// Editor transform: position, Euler rotation (degrees), and scale.
///
/// Stored inline in `SceneObjectData` for easy UI access.  The underlying
/// `SceneDb` stores the same values as lock-free atomics so the renderer can
/// read them without acquiring any mutex.
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
/// `update_object`.  Transform data is stored both here (for easy editing) and
/// in the underlying `SceneDb` (for atomic renderer reads); calling
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
    /// Direct children (populated by `SceneDb` on read, ignored on write).
    pub children: Vec<ObjectId>,
    pub components: Vec<Component>,
    pub scene_path: String,
}

impl SceneObjectData {
    fn from_snapshot(snap: SceneObjectSnapshot) -> Self {
        Self {
            id: snap.id,
            name: snap.name,
            object_type: snap.object_type,
            transform: Transform {
                position: snap.position,
                rotation: snap.rotation,
                scale: snap.scale,
            },
            visible: snap.visible,
            locked: snap.locked,
            parent: snap.parent,
            children: snap.children,
            components: snap.components,
            scene_path: snap.scene_path,
        }
    }

    fn into_snapshot(self) -> SceneObjectSnapshot {
        SceneObjectSnapshot {
            id: self.id,
            name: self.name,
            object_type: self.object_type,
            position: self.transform.position,
            rotation: self.transform.rotation,
            scale: self.transform.scale,
            visible: self.visible,
            locked: self.locked,
            parent: self.parent,
            children: self.children,
            components: self.components,
            scene_path: self.scene_path,
        }
    }
}

// ── Production Scene Database ──────────────────────────────────────────────

/// Production-ready scene database.
///
/// Wraps `SceneDb` (the concurrency-safe object store used by the renderer)
/// and adds the `SceneMetadataDb` layer for the new reflection-based component
/// system.  All UI panels interact exclusively with `SceneDatabase`; they never
/// talk to `SceneDb` or `SceneMetadataDb` directly.
#[derive(Clone)]
pub struct SceneDatabase {
    /// Primary store: lock-free atomic transforms + hierarchy.
    pub scene_db: Arc<SceneDb>,
    /// Reflection-based component store (new system).
    pub metadata_db: Arc<SceneMetadataDb>,
}

impl SceneDatabase {
    pub fn new() -> Self {
        Self {
            scene_db: Arc::new(SceneDb::new()),
            metadata_db: Arc::new(SceneMetadataDb::new()),
        }
    }

    /// Create with the default demo scene objects.
    pub fn with_default_scene() -> Self {
        let this = Self::new();
        this.populate_default_scene();
        this
    }

    /// Create using a caller-supplied `SceneDb` that is shared with the renderer.
    ///
    /// Populates the default demo scene into the provided database.
    pub fn with_default_scene_on(scene_db: Arc<SceneDb>) -> Self {
        let this = Self {
            scene_db,
            metadata_db: Arc::new(SceneMetadataDb::new()),
        };
        this.populate_default_scene();
        this
    }

    fn mk(name: &str, object_type: ObjectType, parent: Option<ObjectId>) -> SceneObjectData {
        SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent,
            children: vec![],
            components: vec![],
            scene_path: String::new(),
        }
    }

    fn mk_at(
        name: &str,
        object_type: ObjectType,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    ) -> SceneObjectData {
        let mut obj = Self::mk(name, object_type, None);
        obj.transform.position = position;
        obj.transform.rotation = rotation;
        obj.transform.scale = scale;
        obj
    }

    fn populate_default_scene(&self) {
        self.populate_default_scene_pub();
    }

    /// Public entry-point for in-place scene reset (called by `on_new_scene`).
    ///
    /// Populates an interesting starter scene whose objects are picked up by
    /// `sync_scene` (the renderer only draws `ObjectType::Mesh` objects, so
    /// Camera and Light are metadata-only until full Helio light/camera support
    /// is wired up).  The shared `Arc<SceneDb>` means every change here is
    /// immediately visible to the renderer.
    pub fn populate_default_scene_pub(&self) {
        // ── Camera ────────────────────────────────────────────────────────
        let mut cam = Self::mk("Main Camera", ObjectType::Camera, None);
        cam.transform.position = [0.0, 6.0, 14.0];
        cam.transform.rotation = [-18.0, 0.0, 0.0];
        self.add_object(cam, None);

        // ── Lighting ──────────────────────────────────────────────────────
        self.add_object(
            Self::mk_at(
                "Blue Light",
                ObjectType::Light(LightType::Point),
                [-8.0, 6.0, -6.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 1.0],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Red Light",
                ObjectType::Light(LightType::Point),
                [8.0, 6.0, -6.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 1.0],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Yellow Light",
                ObjectType::Light(LightType::Point),
                [0.0, 7.0, 8.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 1.0],
            ),
            None,
        );

        // ── Ground ───────────────────────────────────────────────────────
        self.add_object(
            Self::mk_at(
                "Ground",
                ObjectType::Mesh(MeshType::Plane),
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
                [3.0, 1.0, 3.0], // 30 × 30 unit floor
            ),
            None,
        );

        // ── Centre composition ────────────────────────────────────────────
        // Stepped podium: three stacked cubes of decreasing size
        self.add_object(
            Self::mk_at(
                "Podium Base",
                ObjectType::Mesh(MeshType::Cube),
                [0.0, 0.15, 0.0],
                [0.0, 0.0, 0.0],
                [3.0, 0.3, 3.0],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Podium Mid",
                ObjectType::Mesh(MeshType::Cube),
                [0.0, 0.5, 0.0],
                [0.0, 0.0, 0.0],
                [2.0, 0.2, 2.0],
            ),
            None,
        );

        // Hero sphere on top of the podium
        self.add_object(
            Self::mk_at(
                "Hero Sphere",
                ObjectType::Mesh(MeshType::Sphere),
                [0.0, 1.5, 0.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 1.0],
            ),
            None,
        );

        // ── Left wing — arch of cubes ─────────────────────────────────────
        self.add_object(
            Self::mk_at(
                "Column L1",
                ObjectType::Mesh(MeshType::Cylinder),
                [-4.0, 1.5, -2.0],
                [0.0, 0.0, 0.0],
                [0.4, 3.0, 0.4],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Column L2",
                ObjectType::Mesh(MeshType::Cylinder),
                [-4.0, 1.5, 2.0],
                [0.0, 0.0, 0.0],
                [0.4, 3.0, 0.4],
            ),
            None,
        );

        // Lintel across the two left columns
        self.add_object(
            Self::mk_at(
                "Lintel L",
                ObjectType::Mesh(MeshType::Cube),
                [-4.0, 3.2, 0.0],
                [0.0, 0.0, 0.0],
                [0.6, 0.4, 4.8],
            ),
            None,
        );

        // ── Right wing — stepped tower ────────────────────────────────────
        self.add_object(
            Self::mk_at(
                "Tower Base",
                ObjectType::Mesh(MeshType::Cube),
                [4.5, 0.75, 0.0],
                [0.0, 20.0, 0.0],
                [1.5, 1.5, 1.5],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Tower Mid",
                ObjectType::Mesh(MeshType::Cube),
                [4.5, 2.25, 0.0],
                [0.0, 35.0, 0.0],
                [1.1, 1.5, 1.1],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Tower Top",
                ObjectType::Mesh(MeshType::Cube),
                [4.5, 3.5, 0.0],
                [0.0, 50.0, 0.0],
                [0.7, 0.7, 0.7],
            ),
            None,
        );

        // ── Scattered detail props ────────────────────────────────────────
        self.add_object(
            Self::mk_at(
                "Rock A",
                ObjectType::Mesh(MeshType::Sphere),
                [-2.0, 0.3, 4.0],
                [15.0, 30.0, 0.0],
                [0.6, 0.5, 0.7],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Rock B",
                ObjectType::Mesh(MeshType::Sphere),
                [2.5, 0.25, 4.5],
                [0.0, 60.0, 20.0],
                [0.45, 0.4, 0.55],
            ),
            None,
        );

        // Ramp (tilted plane)
        self.add_object(
            Self::mk_at(
                "Ramp",
                ObjectType::Mesh(MeshType::Plane),
                [-1.0, 0.6, -4.5],
                [-25.0, 15.0, 0.0],
                [1.0, 1.0, 1.5],
            ),
            None,
        );

        // Elevated bridge segment crossing the center line
        self.add_object(
            Self::mk_at(
                "Bridge Deck",
                ObjectType::Mesh(MeshType::Cube),
                [0.0, 1.8, -6.0],
                [0.0, 0.0, 0.0],
                [4.5, 0.25, 1.0],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Bridge Pillar L",
                ObjectType::Mesh(MeshType::Cylinder),
                [-2.0, 0.9, -6.0],
                [0.0, 0.0, 0.0],
                [0.3, 1.8, 0.3],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Bridge Pillar R",
                ObjectType::Mesh(MeshType::Cylinder),
                [2.0, 0.9, -6.0],
                [0.0, 0.0, 0.0],
                [0.3, 1.8, 0.3],
            ),
            None,
        );

        // Floating accents to add silhouette variation
        self.add_object(
            Self::mk_at(
                "Float Orb A",
                ObjectType::Mesh(MeshType::Sphere),
                [-6.0, 3.8, 3.0],
                [0.0, 0.0, 0.0],
                [0.45, 0.45, 0.45],
            ),
            None,
        );

        self.add_object(
            Self::mk_at(
                "Float Orb B",
                ObjectType::Mesh(MeshType::Sphere),
                [6.0, 4.2, 2.5],
                [0.0, 0.0, 0.0],
                [0.55, 0.55, 0.55],
            ),
            None,
        );

        tracing::info!(
            "Default scene populated: three-point color lights + podium composition + bridge + tower + floating accents"
        );
    }

    // ── Object CRUD ───────────────────────────────────────────────────────

    /// Add an object. Returns the assigned `ObjectId`.
    pub fn add_object(&self, obj: SceneObjectData, parent: Option<ObjectId>) -> ObjectId {
        self.scene_db.add_object(obj.into_snapshot(), parent)
    }

    /// Remove an object and all of its descendants. Returns `true` if found.
    pub fn remove_object(&self, id: &ObjectId) -> bool {
        self.scene_db.remove_object(id)
    }

    /// Write updated data back to an existing object.
    ///
    /// Synchronises the atomic renderer transforms as well as all cold data
    /// (name, visibility, locked, components).  Returns `true` on success.
    pub fn update_object(&self, obj: SceneObjectData) -> bool {
        let id = obj.id.clone();
        if self.scene_db.get_entry(&id).is_none() {
            return false;
        }
        self.scene_db.apply_transform(
            &id,
            obj.transform.position,
            obj.transform.rotation,
            obj.transform.scale,
        );
        self.scene_db.set_name(&id, obj.name);
        self.scene_db.set_visible(&id, obj.visible);
        self.scene_db.set_locked(&id, obj.locked);
        if let Some(entry) = self.scene_db.get_entry(&id) {
            entry.meta.write().components = obj.components;
        }
        true
    }

    /// Clear the entire scene.
    pub fn clear(&self) {
        let root_ids: Vec<ObjectId> = self
            .scene_db
            .get_root_snapshots()
            .into_iter()
            .map(|s| s.id)
            .collect();
        for id in root_ids {
            self.scene_db.remove_object(&id);
        }
        tracing::info!("Scene cleared – ready for new level");
    }

    // ── Queries ───────────────────────────────────────────────────────────

    /// All objects in depth-first order.
    pub fn get_all_objects(&self) -> Vec<SceneObjectData> {
        self.scene_db
            .get_all_snapshots()
            .into_iter()
            .map(SceneObjectData::from_snapshot)
            .collect()
    }

    /// Root-level objects (no parent).
    pub fn get_root_objects(&self) -> Vec<SceneObjectData> {
        self.scene_db
            .get_root_snapshots()
            .into_iter()
            .map(SceneObjectData::from_snapshot)
            .collect()
    }

    /// Single object by ID, `None` if not found.
    pub fn get_object(&self, id: &ObjectId) -> Option<SceneObjectData> {
        self.scene_db
            .get_object(id)
            .map(SceneObjectData::from_snapshot)
    }

    /// Direct children of `id`.
    pub fn get_children(&self, id: &ObjectId) -> Vec<ObjectId> {
        self.scene_db.get_children(Some(id.as_str()))
    }

    // ── Selection ─────────────────────────────────────────────────────────

    pub fn select_object(&self, id: Option<ObjectId>) {
        self.scene_db.select_object(id);
    }

    pub fn get_selected_object_id(&self) -> Option<ObjectId> {
        self.scene_db.get_selected_id()
    }

    pub fn get_selected_object(&self) -> Option<SceneObjectData> {
        self.scene_db
            .get_selected()
            .map(SceneObjectData::from_snapshot)
    }

    // ── Properties ────────────────────────────────────────────────────────

    pub fn set_name(&self, id: &ObjectId, name: String) -> bool {
        self.scene_db.set_name(id, name)
    }

    pub fn set_visible(&self, id: &ObjectId, visible: bool) -> bool {
        self.scene_db.set_visible(id, visible)
    }

    pub fn set_locked(&self, id: &ObjectId, locked: bool) -> bool {
        self.scene_db.set_locked(id, locked)
    }

    /// Re-parent an object (cycle-safe).
    pub fn reparent_object(&self, id: &ObjectId, new_parent: Option<ObjectId>) -> bool {
        self.scene_db.reparent_object(id, new_parent)
    }

    /// Alias for `reparent_object` kept for backward compatibility.
    pub fn set_parent(&self, id: &ObjectId, new_parent: Option<ObjectId>) -> bool {
        self.reparent_object(id, new_parent)
    }

    // ── Ordering ──────────────────────────────────────────────────────────

    /// Move an object one step earlier among its siblings.
    ///
    /// Full reorder support requires a `SceneDb` API extension; this is a
    /// best-effort stub that is safe to call today.
    pub fn move_object_up(&self, _id: &str) {
        // TODO: implement when SceneDb exposes sibling reordering
    }

    /// Move an object one step later among its siblings.
    pub fn move_object_down(&self, _id: &str) {
        // TODO: implement when SceneDb exposes sibling reordering
    }

    // ── Duplication ────────────────────────────────────────────────────────

    /// Shallow-duplicate an object (children are not copied). Returns the new ID.
    pub fn duplicate_object(&self, id: &str) -> Option<ObjectId> {
        let mut obj = self.get_object(&id.to_string())?;
        obj.id = String::new(); // force auto-assign
        obj.name = format!("{} (Copy)", obj.name);
        obj.children = vec![];
        let parent = obj.parent.clone();
        Some(self.add_object(obj, parent))
    }

    // ── Folder helper ──────────────────────────────────────────────────────

    pub fn add_folder(&self, name: &str, parent: Option<ObjectId>) -> ObjectId {
        self.add_object(Self::mk(name, ObjectType::Folder, parent.clone()), parent)
    }

    // ── Reflection component system ────────────────────────────────────────

    pub fn add_component(
        &self,
        object_id: &EditorObjectId,
        class_name: String,
        data: serde_json::Value,
    ) {
        self.metadata_db.add_component(object_id, class_name, data);
    }

    pub fn remove_component(&self, object_id: &EditorObjectId, component_index: usize) {
        self.metadata_db
            .remove_component(object_id, component_index);
    }

    pub fn reorder_component(
        &self,
        object_id: &EditorObjectId,
        from_index: usize,
        to_index: usize,
    ) {
        // Get all components
        let mut components = self.get_components(object_id);
        if from_index >= components.len() || to_index >= components.len() || from_index == to_index {
            return;
        }

        // Reorder the components in the vector
        let component = components.remove(from_index);
        components.insert(to_index, component);

        // Clear all existing components
        while !self.get_components(object_id).is_empty() {
            self.remove_component(object_id, 0);
        }

        // Re-add all components in the new order
        for component in components {
            self.add_component(
                object_id,
                component.class_name.clone(),
                component.data.clone(),
            );
        }
    }

    pub fn get_components(&self, object_id: &EditorObjectId) -> Vec<ComponentInstance> {
        self.metadata_db.get_components(object_id)
    }

    // ── Persistence ────────────────────────────────────────────────────────

    /// Serialize the scene to a JSON level file.
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        if let Some(parent_dir) = path.as_ref().parent() {
            fs::create_dir_all(parent_dir)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        let objects = self.get_all_objects();
        let now = chrono::Utc::now().to_rfc3339();
        let level_file = LevelFile {
            version: "2.0".into(),
            objects,
            metadata: LevelMetadata {
                created: now.clone(),
                modified: now,
                editor_version: env!("CARGO_PKG_VERSION").into(),
            },
        };
        let json = serde_json::to_string_pretty(&level_file)
            .map_err(|e| format!("Failed to serialize: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("Failed to write file: {e}"))?;
        tracing::info!("Scene saved to: {}", path.as_ref().display());
        Ok(())
    }

    /// Load a scene from a JSON level file (replaces the current scene).
    pub fn load_from_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let json = fs::read_to_string(&path).map_err(|e| format!("Failed to read file: {e}"))?;
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
        for obj in level_file.objects {
            let parent = obj.parent.clone();
            self.add_object(obj, parent);
        }
        tracing::info!(
            "Scene loaded from: {} (version: {})",
            path.as_ref().display(),
            level_file.version
        );
        Ok(())
    }
}

impl Default for SceneDatabase {
    fn default() -> Self {
        Self::new()
    }
}

// ── Level File Format ──────────────────────────────────────────────────────

/// JSON level file (version 2.0).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelFile {
    pub version: String,
    pub objects: Vec<SceneObjectData>,
    pub metadata: LevelMetadata,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelMetadata {
    pub created: String,
    pub modified: String,
    pub editor_version: String,
}
