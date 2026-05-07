//! Scene metadata database that bridges UI to Helio Scene
//!
//! This is the main database that ties together metadata, components, and hierarchy
//! while keeping Helio Scene as the single source of truth for render data.

use super::component_db::ComponentDb;
use super::hierarchy::HierarchyManager;
use super::metadata::{
    EditorObjectId, HelioActorHandle, HelioLightId, HelioObjectId, SceneObjectMetadata,
};
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Scene metadata database - the bridge between UI and Helio Scene
///
/// This database provides organizational features (folders, names, hierarchy)
/// while delegating transform and rendering data to Helio Scene. It manages:
/// - Object metadata (names, types, visibility, hierarchy)
/// - Component instances (physics, rendering, gameplay)
/// - Hierarchy relationships (parent-child)
///
/// Transform data lives in Helio Scene and is accessed via Helio APIs.
#[derive(Clone)]
pub struct SceneMetadataDb {
    /// Object metadata by editor ID
    ///
    /// Uses DashMap for lock-free concurrent access.
    objects: Arc<DashMap<EditorObjectId, SceneObjectMetadata>>,

    /// Component storage
    components: ComponentDb,

    /// Hierarchy management
    hierarchy: HierarchyManager,

    /// Auto-incrementing ID counter for new objects
    next_id: Arc<AtomicU64>,

    /// Currently selected object
    selected: Arc<RwLock<Option<EditorObjectId>>>,
}

impl SceneMetadataDb {
    /// Create a new metadata database
    pub fn new() -> Self {
        Self {
            objects: Arc::new(DashMap::new()),
            components: ComponentDb::new(),
            hierarchy: HierarchyManager::new(),
            next_id: Arc::new(AtomicU64::new(1)),
            selected: Arc::new(RwLock::new(None)),
        }
    }

    // ── Object Management ─────────────────────────────────────────────────

    /// Add a new object to the database
    ///
    /// If the object's editor_id is empty, a unique ID is auto-assigned.
    /// The scene_path is automatically computed from the parent chain.
    pub fn add_object(
        &self,
        mut metadata: SceneObjectMetadata,
        parent_id: Option<EditorObjectId>,
    ) -> EditorObjectId {
        // Auto-assign ID if needed
        if metadata.editor_id.is_empty() {
            let n = self.next_id.fetch_add(1, Ordering::Relaxed);
            metadata.editor_id = format!("object_{}", n);
        }

        let editor_id = metadata.editor_id.clone();

        // Compute scene path from parent chain
        metadata.scene_path = self.compute_scene_path(&metadata.name, parent_id.as_deref());
        metadata.parent = parent_id.clone();

        // Add to hierarchy
        self.hierarchy.add_object(editor_id.clone(), parent_id);

        // Store metadata
        self.objects.insert(editor_id.clone(), metadata);

        editor_id
    }

    /// Remove an object and all its descendants
    ///
    /// This recursively removes children and their components.
    /// Returns true if the object was removed.
    pub fn remove_object(&self, object_id: &EditorObjectId) -> bool {
        // Collect descendants before removing anything
        let descendants = self.hierarchy.get_descendants_dfs(object_id);

        // Remove the object itself
        if self.objects.remove(object_id).is_none() {
            return false;
        }

        // Remove from hierarchy
        self.hierarchy.remove_object(object_id);

        // Remove components
        self.components.clear_components(object_id);

        // Recursively remove descendants
        for descendant_id in descendants {
            self.objects.remove(&descendant_id);
            self.hierarchy.remove_object(&descendant_id);
            self.components.clear_components(&descendant_id);
        }

        // Deselect if this was selected
        let mut selected = self.selected.write();
        if selected.as_ref() == Some(object_id) {
            *selected = None;
        }

        true
    }

    /// Get object metadata
    pub fn get_object(&self, object_id: &EditorObjectId) -> Option<SceneObjectMetadata> {
        self.objects.get(object_id).map(|entry| entry.clone())
    }

    /// Get all root-level objects
    pub fn get_root_objects(&self) -> Vec<SceneObjectMetadata> {
        self.hierarchy
            .get_roots()
            .into_iter()
            .filter_map(|id| self.get_object(&id))
            .collect()
    }

    /// Get all objects in depth-first order (for serialization)
    pub fn get_all_objects_dfs(&self) -> Vec<SceneObjectMetadata> {
        let mut result = Vec::new();
        self.collect_dfs_recursive(None, &mut result);
        result
    }

    fn collect_dfs_recursive(
        &self,
        parent_id: Option<&str>,
        result: &mut Vec<SceneObjectMetadata>,
    ) {
        for child_id in self.hierarchy.get_children(parent_id) {
            if let Some(metadata) = self.get_object(&child_id) {
                result.push(metadata);
                self.collect_dfs_recursive(Some(&child_id), result);
            }
        }
    }

    // ── Object Properties ─────────────────────────────────────────────────

    /// Set object name and update scene path for entire subtree
    pub fn set_name(&self, object_id: &EditorObjectId, new_name: String) -> bool {
        if let Some(mut entry) = self.objects.get_mut(object_id) {
            entry.name = new_name.clone();
            let parent_id = entry.parent.clone();
            drop(entry); // Release lock before recursive call

            // Recompute scene path for this object and descendants
            let new_path = self.compute_scene_path(&new_name, parent_id.as_deref());
            self.update_scene_path_recursive(object_id, &new_path);
            true
        } else {
            false
        }
    }

    /// Set object visibility
    pub fn set_visible(&self, object_id: &EditorObjectId, visible: bool) -> bool {
        if let Some(mut entry) = self.objects.get_mut(object_id) {
            entry.visible = visible;
            true
        } else {
            false
        }
    }

    /// Set object locked state
    pub fn set_locked(&self, object_id: &EditorObjectId, locked: bool) -> bool {
        if let Some(mut entry) = self.objects.get_mut(object_id) {
            entry.locked = locked;
            true
        } else {
            false
        }
    }

    /// Reparent an object to a new parent
    ///
    /// Returns false if the operation would create a cycle.
    pub fn reparent_object(
        &self,
        object_id: &EditorObjectId,
        new_parent: Option<EditorObjectId>,
    ) -> bool {
        // Try to reparent in hierarchy (this checks for cycles)
        if !self
            .hierarchy
            .reparent_object(object_id, new_parent.clone())
        {
            return false;
        }

        // Update metadata
        if let Some(mut entry) = self.objects.get_mut(object_id) {
            entry.parent = new_parent.clone();
            let name = entry.name.clone();
            drop(entry);

            // Recompute scene path for entire subtree
            let new_path = self.compute_scene_path(&name, new_parent.as_deref());
            self.update_scene_path_recursive(object_id, &new_path);
        }

        true
    }

    /// Reorder two sibling objects by swapping their positions
    ///
    /// Both objects must have the same parent. Returns false if they don't share a parent
    /// or if the operation fails.
    pub fn reorder_object_siblings(
        &self,
        object_id: &EditorObjectId,
        target_id: &EditorObjectId,
    ) -> bool {
        // Get the parent of both objects
        let object_parent = self.get_parent(object_id);
        let target_parent = self.get_parent(target_id);

        // Both must have the same parent
        if object_parent != target_parent {
            return false;
        }

        // Get the children list of the shared parent
        let parent_key = object_parent.as_deref();
        let children = self.hierarchy.get_children(parent_key);

        // Find indices of both objects in the children list
        let from_index = children.iter().position(|id| id == object_id);
        let to_index = children.iter().position(|id| id == target_id);

        match (from_index, to_index) {
            (Some(from), Some(to)) => self.hierarchy.reorder_child(parent_key, from, to),
            _ => false,
        }
    }

    // ── Selection ─────────────────────────────────────────────────────────

    /// Select an object
    pub fn select_object(&self, object_id: Option<EditorObjectId>) {
        *self.selected.write() = object_id;
    }

    /// Get selected object ID
    pub fn get_selected_id(&self) -> Option<EditorObjectId> {
        self.selected.read().clone()
    }

    /// Get selected object metadata
    pub fn get_selected(&self) -> Option<SceneObjectMetadata> {
        let id = self.selected.read().clone()?;
        self.get_object(&id)
    }

    // ── Component Access ──────────────────────────────────────────────────

    /// Add a component to an object
    pub fn add_component(
        &self,
        object_id: &EditorObjectId,
        class_name: String,
        data: serde_json::Value,
    ) {
        self.components.add_component(object_id, class_name, data);
    }

    /// Remove a component by index
    pub fn remove_component(&self, object_id: &EditorObjectId, component_index: usize) -> bool {
        self.components.remove_component(object_id, component_index)
    }

    /// Get all components for an object
    pub fn get_components(
        &self,
        object_id: &EditorObjectId,
    ) -> Vec<super::metadata::ComponentInstance> {
        self.components.get_components(object_id)
    }

    /// Get component database for direct access
    pub fn components(&self) -> &ComponentDb {
        &self.components
    }

    // ── Hierarchy Access ──────────────────────────────────────────────────

    /// Get children of an object
    pub fn get_children(&self, parent_id: Option<&str>) -> Vec<EditorObjectId> {
        self.hierarchy.get_children(parent_id)
    }

    /// Get parent of an object
    pub fn get_parent(&self, object_id: &EditorObjectId) -> Option<EditorObjectId> {
        self.hierarchy.get_parent(object_id)
    }

    /// Get hierarchy manager for direct access
    pub fn hierarchy(&self) -> &HierarchyManager {
        &self.hierarchy
    }

    // ── Helio Integration ─────────────────────────────────────────────────

    /// Get Helio object ID for an editor object
    pub fn get_helio_object_id(&self, editor_id: &EditorObjectId) -> Option<HelioObjectId> {
        self.objects.get(editor_id)?.helio_object_id()
    }

    /// Get Helio light ID for an editor object
    pub fn get_helio_light_id(&self, editor_id: &EditorObjectId) -> Option<HelioLightId> {
        self.objects.get(editor_id)?.helio_light_id()
    }

    /// Find editor object by Helio object ID
    pub fn find_by_helio_object(&self, helio_id: HelioObjectId) -> Option<EditorObjectId> {
        self.objects
            .iter()
            .find(|entry| matches!(entry.helio_handle, HelioActorHandle::Object(id) if id == helio_id))
            .map(|entry| entry.key().clone())
    }

    /// Find editor object by Helio light ID
    pub fn find_by_helio_light(&self, helio_id: HelioLightId) -> Option<EditorObjectId> {
        self.objects
            .iter()
            .find(
                |entry| matches!(entry.helio_handle, HelioActorHandle::Light(id) if id == helio_id),
            )
            .map(|entry| entry.key().clone())
    }

    // ── Utility ───────────────────────────────────────────────────────────

    /// Clear all data
    pub fn clear(&self) {
        self.objects.clear();
        self.components.clear_all();
        self.hierarchy.clear();
        *self.selected.write() = None;
    }

    /// Compute scene path from name and parent
    fn compute_scene_path(&self, name: &str, parent_id: Option<&str>) -> String {
        if let Some(pid) = parent_id {
            if let Some(parent) = self.objects.get(pid) {
                let parent_path = &parent.scene_path;
                if parent_path.is_empty() {
                    name.to_string()
                } else {
                    format!("{}/{}", parent_path, name)
                }
            } else {
                name.to_string()
            }
        } else {
            name.to_string()
        }
    }

    /// Recursively update scene paths for an object and its descendants
    fn update_scene_path_recursive(&self, object_id: &EditorObjectId, new_path: &str) {
        // Update this object's path
        if let Some(mut entry) = self.objects.get_mut(object_id) {
            entry.scene_path = new_path.to_string();
        }

        // Update children recursively
        for child_id in self.hierarchy.get_children(Some(object_id)) {
            if let Some(child) = self.objects.get(&child_id) {
                let child_name = child.name.clone();
                drop(child);
                let child_path = format!("{}/{}", new_path, child_name);
                self.update_scene_path_recursive(&child_id, &child_path);
            }
        }
    }
}

impl Default for SceneMetadataDb {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable scene snapshot for save/load
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneSnapshot {
    /// All objects in depth-first order
    pub objects: Vec<SceneObjectMetadata>,

    /// Component data per object
    pub components: Vec<(EditorObjectId, Vec<super::metadata::ComponentInstance>)>,
}

impl SceneMetadataDb {
    /// Create a snapshot for serialization
    pub fn create_snapshot(&self) -> SceneSnapshot {
        let objects = self.get_all_objects_dfs();

        let components: Vec<_> = objects
            .iter()
            .map(|obj| (obj.editor_id.clone(), self.get_components(&obj.editor_id)))
            .filter(|(_, comps)| !comps.is_empty())
            .collect();

        SceneSnapshot {
            objects,
            components,
        }
    }

    /// Load from a snapshot
    pub fn load_snapshot(&self, snapshot: SceneSnapshot) {
        self.clear();

        // Load objects (they're already in depth-first order, so parents before children)
        for metadata in snapshot.objects {
            self.add_object(metadata.clone(), metadata.parent.clone());
        }

        // Load components
        for (object_id, components) in snapshot.components {
            for component in components {
                self.add_component(&object_id, component.class_name, component.data);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_object() {
        let db = SceneMetadataDb::new();

        let metadata =
            SceneObjectMetadata::new_folder("test_folder".to_string(), "My Folder".to_string());

        let id = db.add_object(metadata.clone(), None);

        assert_eq!(id, "test_folder");

        let retrieved = db.get_object(&id).unwrap();
        assert_eq!(retrieved.name, "My Folder");
        assert_eq!(retrieved.scene_path, "My Folder");
    }

    #[test]
    fn test_hierarchy_and_scene_path() {
        let db = SceneMetadataDb::new();

        let parent = SceneObjectMetadata::new_folder(String::new(), "Parent".to_string());
        let parent_id = db.add_object(parent, None);

        let child = SceneObjectMetadata::new_folder(String::new(), "Child".to_string());
        let child_id = db.add_object(child, Some(parent_id.clone()));

        let retrieved_child = db.get_object(&child_id).unwrap();
        assert_eq!(retrieved_child.scene_path, "Parent/Child");
        assert_eq!(retrieved_child.parent, Some(parent_id));
    }

    #[test]
    fn test_remove_with_descendants() {
        let db = SceneMetadataDb::new();

        let parent = SceneObjectMetadata::new_folder(String::new(), "Parent".to_string());
        let parent_id = db.add_object(parent, None);

        let child = SceneObjectMetadata::new_folder(String::new(), "Child".to_string());
        let child_id = db.add_object(child, Some(parent_id.clone()));

        // Remove parent should remove child too
        assert!(db.remove_object(&parent_id));
        assert!(db.get_object(&parent_id).is_none());
        assert!(db.get_object(&child_id).is_none());
    }

    #[test]
    fn test_rename_updates_scene_path() {
        let db = SceneMetadataDb::new();

        let parent = SceneObjectMetadata::new_folder(String::new(), "OldName".to_string());
        let parent_id = db.add_object(parent, None);

        let child = SceneObjectMetadata::new_folder(String::new(), "Child".to_string());
        let child_id = db.add_object(child, Some(parent_id.clone()));

        // Rename parent
        db.set_name(&parent_id, "NewName".to_string());

        // Check paths updated
        assert_eq!(db.get_object(&parent_id).unwrap().scene_path, "NewName");
        assert_eq!(
            db.get_object(&child_id).unwrap().scene_path,
            "NewName/Child"
        );
    }
}
