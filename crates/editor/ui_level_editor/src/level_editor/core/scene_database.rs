//! Production Scene Database
//!
//! Primary scene storage backed by `WorldSceneStore` (`pulsar_scenedb::World`
//! wrapped in one `parking_lot::RwLock`, shared with the renderer) with an
//! additional `SceneMetadataDb` layer for the reflection-based component
//! system.
//!
//! ## B1 migration note
//!
//! This used to wrap `SceneDb` (lock-free atomic transforms, `DashMap`
//! object storage). That's gone -- `WorldSceneStore` has no lock-free
//! per-entry design (`pulsar_scenedb::World` mutation needs `&mut self`
//! throughout), so the concurrency model is now one `RwLock` shared between
//! this type and `HelioRenderer`, which reads it every frame
//! (`sync_scene`/`sync_scene_delta`) and also writes to it directly from the
//! render thread (gizmo-drag transform on release, click-to-select). See
//! `WorldSceneStore`'s own module doc (`engine_backend::scene::world_store`)
//! for the full rationale and what's still deliberately deferred (typed
//! per-component storage -- Pulsar-Native#555/#556).
//!
//! `SceneObjectData`'s shape and every public method signature on
//! `SceneDatabase` are unchanged from the `SceneDb`-backed version -- this
//! is an internal storage swap, not an API redesign, so the ~250 call sites
//! across the editor and AI tools don't need to change.

use engine_backend::scene::{
    Transform as WorldTransform, Visibility as WorldVisibility, WorldSceneStoreError,
};
use engine_backend::{ComponentInstance, EditorObjectId, SceneMetadataDb};
use engine_fs::virtual_fs;
use parking_lot::RwLock;
use pulsar_reflection::{apply_scene_props_for_class, registered_scene_props_classes};
use pulsar_scenedb::Entity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

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
    /// Reflection-based component instances (synced from metadata_db).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_instances: Option<serde_json::Value>,
}

// ── Production Scene Database ──────────────────────────────────────────────

/// Production-ready scene database — the single source of truth for all scene state.
///
/// Wraps `WorldSceneStore` (the `RwLock`-guarded object store shared with the
/// renderer) and `SceneMetadataDb` for the reflection-based component system.
///
/// Helio is reconciled exclusively by `sync_scene()` on every render frame.
/// All UI panels and AI tools interact through `SceneDatabase` only.
#[derive(Clone)]
pub struct SceneDatabase {
    /// Primary store: transforms + hierarchy, behind one `RwLock` shared
    /// with the renderer.
    store: Arc<RwLock<WorldSceneStore>>,
    /// Reflection-based component store.
    metadata_db: Arc<SceneMetadataDb>,
}

impl SceneDatabase {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(WorldSceneStore::new())),
            metadata_db: Arc::new(SceneMetadataDb::new()),
        }
    }

    /// Create using a caller-supplied store `Arc` that is shared with the renderer.
    pub fn with_shared_store(store: Arc<RwLock<WorldSceneStore>>) -> Self {
        Self {
            store,
            metadata_db: Arc::new(SceneMetadataDb::new()),
        }
    }

    // ── Object CRUD ───────────────────────────────────────────────────────
    //
    // WorldSceneStore is the single source of truth. sync_scene() in the
    // renderer reconciles Helio state every frame — no immediate
    // write-through needed.

    /// Add an object. Returns the assigned `ObjectId`.
    ///
    /// Blueprint objects always receive a `ScriptComponent` in `metadata_db`
    /// pointing at their blueprint directory. `sync_registered_component_props_to_scene_db`
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
                WorldVisibility { visible: obj.visible, locked: obj.locked },
            );
            store.set_object_type(entity, obj.object_type);
            let id = store.stable_id_of(entity).unwrap_or_default().to_string();
            store.update_render_props(&id, |render_props| {
                render_props.props = obj.props;
                render_props.component_instances = obj.component_instances;
            });
            id
        };

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
        let ids_to_clear = {
            let mut store = self.store.write();
            let Some(entity) = store.entity_for(id) else { return false };
            let mut ids_to_clear = vec![id.clone()];
            Self::collect_descendant_ids(&store, entity, &mut ids_to_clear);
            store.despawn(entity);
            ids_to_clear
        };
        for object_id in ids_to_clear {
            self.metadata_db.clear_components(&object_id);
        }
        true
    }

    /// Write updated transform, name, visibility, and component data back to an existing object.
    pub fn update_object(&self, obj: SceneObjectData) -> bool {
        let id = obj.id.clone();
        {
            let mut store = self.store.write();
            let Some(entity) = store.entity_for(&id) else { return false };
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
                WorldVisibility { visible: obj.visible, locked: obj.locked },
            );
            store.update_render_props(&id, |render_props| render_props.props = obj.props);
        }
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
        let root_ids: Vec<ObjectId> = {
            let store = self.store.read();
            store
                .children_of(None)
                .iter()
                .filter_map(|&e| store.stable_id_of(e).map(str::to_string))
                .collect()
        };
        for id in root_ids {
            self.remove_object(&id);
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
            Self::merge_component_props(&obj.id, &mut obj.props, &self.metadata_db);
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

    /// Single object by ID, `None` if not found.
    pub fn get_object(&self, id: &ObjectId) -> Option<SceneObjectData> {
        let mut data = {
            let store = self.store.read();
            let entity = store.entity_for(id)?;
            Self::entity_to_scene_object_data(&store, entity)
        };
        Self::merge_component_props(id, &mut data.props, &self.metadata_db);
        Some(data)
    }

    /// Direct children of `id`.
    pub fn get_children(&self, id: &ObjectId) -> Vec<ObjectId> {
        let store = self.store.read();
        let Some(entity) = store.entity_for(id) else { return Vec::new() };
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
        let Some(entity) = store.entity_for(id) else { return false };
        let mut visibility = store.visibility(entity).unwrap_or_default();
        visibility.visible = visible;
        store.set_visibility(entity, visibility)
    }

    pub fn set_locked(&self, id: &ObjectId, locked: bool) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else { return false };
        let mut visibility = store.visibility(entity).unwrap_or_default();
        visibility.locked = locked;
        store.set_visibility(entity, visibility)
    }

    /// Re-parent an object (cycle-safe).
    pub fn reparent_object(&self, id: &ObjectId, new_parent: Option<ObjectId>) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else { return false };
        let new_parent_entity = match new_parent {
            Some(ref parent_id) => match store.entity_for(parent_id) {
                Some(e) => Some(e),
                None => return false,
            },
            None => None,
        };
        store.reparent(entity, new_parent_entity).is_ok()
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
    /// Full reorder support requires a `WorldSceneStore` API extension; this
    /// is a best-effort stub that is safe to call today.
    pub fn move_object_up(&self, _id: &str) {
        // TODO: implement when WorldSceneStore exposes sibling reordering
    }

    /// Move an object one step later among its siblings.
    pub fn move_object_down(&self, _id: &str) {
        // TODO: implement when WorldSceneStore exposes sibling reordering
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

    // ── Internal conversion helpers ──────────────────────────────────────

    /// Build a `SceneObjectData` for `entity` directly off `WorldSceneStore` --
    /// transform/name/visibility/object_type/render_props plus the derived
    /// `parent`/`children`/`scene_path` fields. Does NOT merge live
    /// `metadata_db` component props on top (see [`Self::merge_component_props`]
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

    fn collect_dfs(store: &WorldSceneStore, parent: Option<Entity>, out: &mut Vec<SceneObjectData>) {
        for &entity in store.children_of(parent) {
            out.push(Self::entity_to_scene_object_data(store, entity));
            Self::collect_dfs(store, Some(entity), out);
        }
    }

    fn merge_component_props(
        object_id: &str,
        props: &mut HashMap<String, Value>,
        metadata_db: &SceneMetadataDb,
    ) {
        let components = metadata_db.get_components(&object_id.to_string());
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
        let components = self.metadata_db.get_components(&object_id.to_string());
        let mut store = self.store.write();
        store.update_render_props(object_id, |render_props| {
            for class_name in registered_scene_props_classes() {
                let data = components
                    .iter()
                    .find(|c| c.class_name == class_name && c.enabled)
                    .map(|c| &c.data);
                apply_scene_props_for_class(class_name, &mut render_props.props, data);
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
            render_props.component_instances = Some(Value::Array(instances));
        });

        // Phase B4/B5 (Pulsar-Native#555/#556): hydrate or remove each
        // World-backed component's typed value to match this object's
        // current enabled component list, so HelioRenderer::sync_scene can
        // dispatch ComponentRuntimeBehavior::sync_component directly off
        // World -- no per-frame JSON deserialize for migrated classes. Runs
        // over every *registered* class (not just ones this object
        // currently has) so a component that was just removed or disabled
        // gets its stale typed World value dropped, not merely skipped on
        // the next hydration.
        if let Some(entity) = store.entity_for(object_id) {
            for class_name in pulsar_world_registry::registered_world_component_classes() {
                let enabled_data = components
                    .iter()
                    .find(|c| c.class_name == class_name && c.enabled)
                    .map(|c| &c.data);
                match enabled_data {
                    Some(data) => {
                        if let Err(error) = pulsar_world_registry::hydrate_world_component_for_class(
                            class_name,
                            store.world_mut(),
                            entity,
                            data,
                        ) {
                            tracing::warn!(
                                "World hydration failed for {class_name} on '{object_id}': {error}"
                            );
                        }
                    }
                    None => {
                        pulsar_world_registry::remove_world_component_for_class(
                            class_name,
                            store.world_mut(),
                            entity,
                        );
                    }
                }
            }
        }
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
    /// plus every object's reflection component data from `metadata_db`
    /// (the two are captured together so a restore can't reintroduce one
    /// half stale relative to the other). Treat the result as opaque; pass
    /// it back to [`Self::restore_history_snapshot`] only.
    pub fn capture_history_snapshot(&self) -> SceneHistorySnapshot {
        let objects = self.store.read().to_snapshots();
        let components = objects
            .iter()
            .map(|obj| (obj.stable_id.clone(), self.metadata_db.get_components(&obj.stable_id)))
            .filter(|(_, components)| !components.is_empty())
            .collect();
        SceneHistorySnapshot { objects, components }
    }

    /// Restore a previously captured snapshot, replacing the current scene
    /// entirely (`WorldSceneStore` is swapped for a fresh one built from the
    /// snapshot; `metadata_db` is cleared and repopulated). Entity identity
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
        let new_store =
            WorldSceneStore::load_from_snapshots(&snapshot.objects).map_err(|e| e.to_string())?;
        *self.store.write() = new_store;
        self.metadata_db.clear();
        for (object_id, components) in &snapshot.components {
            for component in components {
                self.metadata_db
                    .add_component_instance(object_id, component.clone());
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
        db.add_component(&id, "LightComponent".to_string(), serde_json::json!({"intensity": 5.0}));
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
    use pulsar_rendering::StaticMeshComponent;

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
            serde_json::json!({"mesh_asset": "meshes/primitives/SM_Cube.fbx"}),
        );

        let store = db.store.read();
        let entity = store.entity_for(&id).unwrap();
        let hydrated = store.world().get::<StaticMeshComponent>(entity).unwrap();
        assert_eq!(hydrated.mesh_asset.as_str(), "meshes/primitives/SM_Cube.fbx");
    }

    #[test]
    fn update_component_property_re_hydrates_the_typed_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            serde_json::json!({"mesh_asset": "meshes/primitives/SM_Cube.fbx"}),
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
        assert_eq!(hydrated.mesh_asset.as_str(), "meshes/primitives/SM_Sphere.fbx");
    }

    #[test]
    fn remove_component_drops_the_typed_world_value() {
        let db = SceneDatabase::new();
        let id = db.add_object(object("Cube"), None);
        db.add_component(
            &id,
            "StaticMeshComponent".to_string(),
            serde_json::json!({"mesh_asset": "meshes/primitives/SM_Cube.fbx"}),
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
            serde_json::json!({"mesh_asset": "meshes/primitives/SM_Cube.fbx"}),
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
}
