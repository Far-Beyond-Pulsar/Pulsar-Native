//! Production Scene Database
//!
//! Primary scene storage backed by the concurrency-safe `SceneDb` (atomic
//! transforms, lock-free renderer reads) with an additional `SceneMetadataDb`
//! layer for the reflection-based component system.

use engine_backend::scene::SceneObjectSnapshot;
use engine_backend::{ComponentInstance, EditorObjectId, SceneMetadataDb};
use engine_fs::virtual_fs;
use pulsar_reflection::{apply_scene_props_for_class, registered_scene_props_classes};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

// ── Public re-exports for UI layer compatibility ───────────────────────────

pub use engine_backend::scene::{LightType, MeshType, ObjectId, ObjectType, SceneDb};

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
    pub scene_path: String,
    /// Type-specific properties that round-trip through the level file.
    /// Lights: `"color_r"`, `"color_g"`, `"color_b"`, `"intensity"`, `"range"`.
    ///
    /// ⚠ This field does **not** contain `__component_instances`.  Component
    /// data flows exclusively through `SceneDatabase::add_component` / etc.
    #[serde(default)]
    pub props: std::collections::HashMap<String, serde_json::Value>,
    /// Reflection-based component instances (synced from metadata_db).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_instances: Option<serde_json::Value>,
}

impl SceneObjectData {
    fn from_snapshot(mut snap: SceneObjectSnapshot) -> Self {
        // Strip legacy __component_instances from props — it now lives in
        // the dedicated component_instances field.  Setting it via props
        // has no effect on the rendering subsystem.
        let legacy = snap.props.remove("__component_instances");
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
            scene_path: snap.scene_path,
            props: snap.props,
            component_instances: snap.component_instances.or(legacy),
        }
    }

    fn into_snapshot(mut self) -> SceneObjectSnapshot {
        // Never write __component_instances into props — it goes in the
        // dedicated field so the renderer cannot be bypassed.
        self.props.remove("__component_instances");
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
            scene_path: self.scene_path,
            props: self.props,
            component_instances: self.component_instances,
        }
    }
}

// ── Production Scene Database ──────────────────────────────────────────────

/// Production-ready scene database — the single source of truth for all scene state.
///
/// Wraps `SceneDb` (the concurrency-safe object store shared with the renderer)
/// and `SceneMetadataDb` for the reflection-based component system.
///
/// Helio is reconciled exclusively by `sync_scene()` on every render frame.
/// All UI panels and AI tools interact through `SceneDatabase` only.
#[derive(Clone)]
pub struct SceneDatabase {
    /// Primary store: lock-free atomic transforms + hierarchy.
    scene_db: Arc<SceneDb>,
    /// Reflection-based component store.
    metadata_db: Arc<SceneMetadataDb>,
}

impl SceneDatabase {
    pub fn new() -> Self {
        Self {
            scene_db: Arc::new(SceneDb::new()),
            metadata_db: Arc::new(SceneMetadataDb::new()),
        }
    }

    /// Create using a caller-supplied `SceneDb` Arc that is shared with the renderer.
    pub fn with_shared_db(scene_db: Arc<SceneDb>) -> Self {
        Self {
            scene_db,
            metadata_db: Arc::new(SceneMetadataDb::new()),
        }
    }

    // ── Object CRUD ───────────────────────────────────────────────────────
    //
    // SceneDb is the single source of truth. sync_scene() in the renderer
    // reconciles Helio state every frame — no immediate write-through needed.

    /// Add an object. Returns the assigned `ObjectId`.
    ///
    /// Blueprint objects always receive a `ScriptComponent` in `metadata_db`
    /// pointing at their blueprint directory.  `sync_registered_component_props_to_scene_db`
    /// rebuilds `__component_instances` from `metadata_db`, so the component
    /// must live there — setting it only in `props` would be immediately overwritten.
    pub fn add_object(&self, obj: SceneObjectData, parent: Option<ObjectId>) -> ObjectId {
        let blueprint_script_path = if obj.object_type == ObjectType::Blueprint {
            Some(find_script_path(
                &obj.props,
                obj.component_instances.as_ref(),
            ))
        } else {
            None
        };

        let object_id = self.scene_db.add_object(obj.into_snapshot(), parent);

        if let Some(script_path) = blueprint_script_path {
            let already_has = self
                .metadata_db
                .get_components(&object_id)
                .iter()
                .any(|c| c.class_name == "ScriptComponent");

            if !already_has {
                self.metadata_db.add_component(
                    &object_id,
                    "ScriptComponent".to_string(),
                    serde_json::json!({ "script_asset": script_path }),
                );
            }
        }

        self.sync_registered_component_props_to_scene_db(&object_id);
        object_id
    }

    /// Remove an object and all of its descendants. Returns `true` if found.
    pub fn remove_object(&self, id: &ObjectId) -> bool {
        let mut ids_to_clear = vec![id.clone()];
        Self::collect_descendant_ids(&self.scene_db, id, &mut ids_to_clear);

        let removed = self.scene_db.remove_object(id);
        if removed {
            for object_id in ids_to_clear {
                self.metadata_db.clear_components(&object_id);
            }
        }
        removed
    }

    /// Write updated transform, name, visibility, and component data back to an existing object.
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
        self.scene_db
            .update_render_data(&id, |meta| meta.props = obj.props);
        self.sync_registered_component_props_to_scene_db(&id);
        true
    }

    /// Update a single component's JSON data by index.
    ///
    /// This is the correct entry point for component edits; callers must not
    /// access `metadata_db` directly.
    pub fn update_component(
        &self,
        object_id: &ObjectId,
        component_index: usize,
        data: serde_json::Value,
    ) {
        let ok = self
            .metadata_db
            .components()
            .update_component(object_id, component_index, data);
        if !ok {
            tracing::warn!(
                "[UPDATE_COMPONENT] metadata_db.update_component returned false for {object_id} idx={component_index}"
            );
        }
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    /// Update a single property inside a reflection-based component by class name and property name.
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
        }
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
            .map(|mut snap| {
                let object_id = snap.id.clone();
                Self::merge_component_props(&object_id, &mut snap, &self.metadata_db);
                SceneObjectData::from_snapshot(snap)
            })
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
        self.scene_db.get_object(id).map(|mut snap| {
            let object_id = snap.id.clone();
            Self::merge_component_props(&object_id, &mut snap, &self.metadata_db);
            SceneObjectData::from_snapshot(snap)
        })
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

    /// Reorder two sibling objects by swapping their positions
    ///
    /// Both objects must have the same parent. Returns false if they don't share a parent.
    pub fn reorder_object_siblings(&self, object_id: &ObjectId, target_id: &ObjectId) -> bool {
        self.metadata_db
            .reorder_object_siblings(object_id, target_id)
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
        let source_id = id.to_string();
        let source_components = self.get_components(&source_id);
        let mut obj = self.get_object(&source_id)?;
        obj.id = String::new(); // force auto-assign
        obj.name = format!("{} (Copy)", obj.name);
        obj.children = vec![];
        let parent = obj.parent.clone();
        let new_id = self.add_object(obj, parent);

        self.metadata_db.clear_components(&new_id);
        for component in source_components {
            self.metadata_db.add_component_instance(&new_id, component);
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
        self.metadata_db.add_component(object_id, class_name, data);
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    /// Add a fully specified component instance.
    pub fn add_component_instance(&self, object_id: &EditorObjectId, component: ComponentInstance) {
        self.metadata_db
            .add_component_instance(object_id, component);
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    pub fn remove_component(&self, object_id: &EditorObjectId, component_index: usize) {
        self.metadata_db
            .remove_component(object_id, component_index);
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    /// Enable or disable a component by index.
    pub fn set_component_enabled(
        &self,
        object_id: &EditorObjectId,
        component_index: usize,
        enabled: bool,
    ) -> bool {
        let changed = self
            .metadata_db
            .set_component_enabled(object_id, component_index, enabled);
        if changed {
            self.sync_registered_component_props_to_scene_db(object_id);
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
        components.insert(insert_index, component);
        self.metadata_db.replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
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
        components.insert(to_index, component);
        self.metadata_db.replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    pub fn get_components(&self, object_id: &EditorObjectId) -> Vec<ComponentInstance> {
        self.metadata_db.get_components(object_id)
    }

    /// Check if a component is a descendant of another component
    fn is_component_descendant(
        components: &[ComponentInstance],
        potential_descendant: usize,
        potential_ancestor: usize,
    ) -> bool {
        let mut current = potential_descendant;
        loop {
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
        self.metadata_db.replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    // ── Persistence ────────────────────────────────────────────────────────

    /// Serialize the scene to a JSON level file.
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        self.save_to_file_with_editor_camera(path, None)
    }

    /// Serialize the scene to a JSON level file, optionally persisting editor camera state.
    pub fn save_to_file_with_editor_camera<P: AsRef<Path>>(
        &self,
        path: P,
        editor_camera: Option<LevelEditorCameraState>,
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
        let preserved_editor = if editor_camera.is_none() {
            virtual_fs::read_file(path.as_ref())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|json: String| serde_json::from_str::<LevelFile>(&json).ok())
                .and_then(|file| file.editor)
        } else {
            None
        };
        let level_file = LevelFile {
            version: "2.1".into(),
            objects,
            components,
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
                    self.add_component(&object_id, component.class_name, component.data);
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

    fn merge_component_props(
        object_id: &str,
        snap: &mut SceneObjectSnapshot,
        metadata_db: &SceneMetadataDb,
    ) {
        let components = metadata_db.get_components(&object_id.to_string());
        for component in components.into_iter().filter(|component| component.enabled) {
            if apply_scene_props_for_class(
                &component.class_name,
                &mut snap.props,
                Some(&component.data),
            ) {
                continue;
            }

            if let Value::Object(map) = component.data {
                for (k, v) in map {
                    snap.props.insert(k, v);
                }
            }
        }
    }

    fn sync_registered_component_props_to_scene_db(&self, object_id: &str) {
        let components = self.metadata_db.get_components(&object_id.to_string());

        self.scene_db.update_render_data(object_id, |meta| {
            for class_name in registered_scene_props_classes() {
                let data = components
                    .iter()
                    .find(|c| c.class_name == class_name && c.enabled)
                    .map(|c| &c.data);
                apply_scene_props_for_class(class_name, &mut meta.props, data);
            }

            let instances: Vec<serde_json::Value> = components
                .iter()
                .enumerate()
                .filter(|(_, component)| component.enabled)
                .map(|(index, component)| {
                    serde_json::json!({
                        "index": index,
                        "class_name": component.class_name,
                        "data": component.data
                    })
                })
                .collect();
            meta.component_instances = Some(Value::Array(instances));
        });
    }

    fn collect_descendant_ids(scene_db: &SceneDb, id: &str, out: &mut Vec<ObjectId>) {
        for child_id in scene_db.get_children(Some(id)) {
            out.push(child_id.clone());
            Self::collect_descendant_ids(scene_db, &child_id, out);
        }
    }
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
/// array, and finally the flat `props["script_asset"]`.  Returns an empty
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
