//! Hierarchy management for scene object parent-child relationships

use super::metadata::EditorObjectId;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Manages parent-child relationships for scene objects
///
/// This provides the organizational hierarchy that allows objects to be
/// grouped into folders and parented to each other. The hierarchy is
/// independent of Helio Scene's transform hierarchy.
pub struct HierarchyManager {
    /// Maps parent_id → ordered child ids
    ///
    /// Key "" (empty string) represents root-level objects.
    /// This is the single source of truth for parent-child relationships.
    children_map: Arc<RwLock<HashMap<String, Vec<EditorObjectId>>>>,

    /// Reverse mapping: child_id → parent_id
    ///
    /// Cached for efficient parent lookups. Kept in sync with children_map.
    parent_map: Arc<RwLock<HashMap<EditorObjectId, EditorObjectId>>>,
}

impl HierarchyManager {
    /// Create a new hierarchy manager
    pub fn new() -> Self {
        Self {
            children_map: Arc::new(RwLock::new(HashMap::new())),
            parent_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add an object to the hierarchy
    ///
    /// If parent_id is None, the object becomes a root-level object.
    pub fn add_object(&self, object_id: EditorObjectId, parent_id: Option<EditorObjectId>) {
        let parent_key = parent_id.as_deref().unwrap_or("").to_string();

        // Add to children_map
        self.children_map
            .write()
            .entry(parent_key)
            .or_insert_with(Vec::new)
            .push(object_id.clone());

        // Add to parent_map if has parent
        if let Some(pid) = parent_id {
            self.parent_map.write().insert(object_id, pid);
        }
    }

    /// Remove an object from the hierarchy
    ///
    /// This removes the object from its parent's children list and removes
    /// the object's own children list. It does NOT recursively delete children
    /// - that's handled by the higher-level metadata database.
    pub fn remove_object(&self, object_id: &EditorObjectId) {
        // Get parent before modifying anything
        let parent = self.get_parent(object_id);
        let parent_key = parent.as_deref().unwrap_or("").to_string();

        // Remove from parent's children list
        if let Some(siblings) = self.children_map.write().get_mut(&parent_key) {
            siblings.retain(|id| id != object_id);
        }

        // Remove object's own children list
        self.children_map.write().remove(object_id);

        // Remove from parent_map
        self.parent_map.write().remove(object_id);
    }

    /// Reparent an object to a new parent
    ///
    /// Returns false if the operation would create a cycle.
    pub fn reparent_object(
        &self,
        object_id: &EditorObjectId,
        new_parent: Option<EditorObjectId>,
    ) -> bool {
        // Prevent cycles: can't parent an object to itself or its descendants
        if let Some(ref new_parent_id) = new_parent {
            if object_id == new_parent_id || self.is_ancestor_of(object_id, new_parent_id) {
                return false;
            }
        }

        // Get old parent
        let old_parent = self.get_parent(object_id);
        let old_parent_key = old_parent.as_deref().unwrap_or("").to_string();
        let new_parent_key = new_parent.as_deref().unwrap_or("").to_string();

        // Remove from old parent's children
        if let Some(siblings) = self.children_map.write().get_mut(&old_parent_key) {
            siblings.retain(|id| id != object_id);
        }

        // Add to new parent's children
        self.children_map
            .write()
            .entry(new_parent_key)
            .or_insert_with(Vec::new)
            .push(object_id.clone());

        // Update parent_map
        if let Some(new_parent_id) = new_parent {
            self.parent_map
                .write()
                .insert(object_id.clone(), new_parent_id);
        } else {
            self.parent_map.write().remove(object_id);
        }

        true
    }

    /// Get the parent of an object
    ///
    /// Returns None if the object is at root level or doesn't exist.
    pub fn get_parent(&self, object_id: &EditorObjectId) -> Option<EditorObjectId> {
        self.parent_map.read().get(object_id).cloned()
    }

    /// Get the children of an object (or root objects if None)
    ///
    /// Returns an empty vec if the object has no children.
    pub fn get_children(&self, parent_id: Option<&str>) -> Vec<EditorObjectId> {
        let key = parent_id.unwrap_or("");
        self.children_map
            .read()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// Get root-level objects
    pub fn get_roots(&self) -> Vec<EditorObjectId> {
        self.get_children(None)
    }

    /// Check if `potential_ancestor` is an ancestor of `object_id`
    ///
    /// Used for cycle detection when reparenting.
    pub fn is_ancestor_of(
        &self,
        potential_ancestor: &EditorObjectId,
        object_id: &EditorObjectId,
    ) -> bool {
        let mut current = object_id.clone();

        loop {
            if let Some(parent_id) = self.get_parent(&current) {
                if &parent_id == potential_ancestor {
                    return true;
                }
                current = parent_id;
            } else {
                return false;
            }
        }
    }

    /// Get all ancestors of an object, ordered from immediate parent to root
    ///
    /// Useful for building scene paths.
    pub fn get_ancestors(&self, object_id: &EditorObjectId) -> Vec<EditorObjectId> {
        let mut ancestors = Vec::new();
        let mut current = object_id.clone();

        while let Some(parent_id) = self.get_parent(&current) {
            ancestors.push(parent_id.clone());
            current = parent_id;
        }

        ancestors
    }

    /// Get all descendants of an object in depth-first order
    ///
    /// Useful for recursive operations like deletion.
    pub fn get_descendants_dfs(&self, object_id: &EditorObjectId) -> Vec<EditorObjectId> {
        let mut result = Vec::new();

        for child_id in self.get_children(Some(object_id)) {
            result.push(child_id.clone());
            let mut descendants = self.get_descendants_dfs(&child_id);
            result.append(&mut descendants);
        }

        result
    }

    /// Move an object within its parent's children list (reorder)
    ///
    /// Returns true if the object was moved, false if the indices were invalid.
    pub fn reorder_child(
        &self,
        parent_id: Option<&str>,
        from_index: usize,
        to_index: usize,
    ) -> bool {
        let key = parent_id.unwrap_or("").to_string();
        let mut children = self.children_map.write();

        if let Some(child_list) = children.get_mut(&key) {
            if from_index < child_list.len() && to_index < child_list.len() {
                let item = child_list.remove(from_index);
                child_list.insert(to_index, item);
                return true;
            }
        }

        false
    }

    /// Clear all hierarchy data
    pub fn clear(&self) {
        self.children_map.write().clear();
        self.parent_map.write().clear();
    }

    /// Get the depth of an object in the hierarchy (0 = root)
    pub fn get_depth(&self, object_id: &EditorObjectId) -> usize {
        let mut depth = 0;
        let mut current = object_id.clone();

        while let Some(parent_id) = self.get_parent(&current) {
            depth += 1;
            current = parent_id;
        }

        depth
    }
}

impl Default for HierarchyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HierarchyManager {
    fn clone(&self) -> Self {
        Self {
            children_map: Arc::clone(&self.children_map),
            parent_map: Arc::clone(&self.parent_map),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_children() {
        let hierarchy = HierarchyManager::new();

        hierarchy.add_object("obj1".to_string(), None);
        hierarchy.add_object("obj2".to_string(), Some("obj1".to_string()));
        hierarchy.add_object("obj3".to_string(), Some("obj1".to_string()));

        let roots = hierarchy.get_roots();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0], "obj1");

        let children = hierarchy.get_children(Some("obj1"));
        assert_eq!(children.len(), 2);
        assert!(children.contains(&"obj2".to_string()));
        assert!(children.contains(&"obj3".to_string()));
    }

    #[test]
    fn test_reparent() {
        let hierarchy = HierarchyManager::new();

        hierarchy.add_object("obj1".to_string(), None);
        hierarchy.add_object("obj2".to_string(), None);
        hierarchy.add_object("obj3".to_string(), Some("obj1".to_string()));

        // Reparent obj3 from obj1 to obj2
        assert!(hierarchy.reparent_object(&"obj3".to_string(), Some("obj2".to_string())));

        assert_eq!(hierarchy.get_children(Some("obj1")).len(), 0);
        assert_eq!(hierarchy.get_children(Some("obj2")).len(), 1);
        assert_eq!(hierarchy.get_parent(&"obj3".to_string()), Some("obj2".to_string()));
    }

    #[test]
    fn test_cycle_prevention() {
        let hierarchy = HierarchyManager::new();

        hierarchy.add_object("obj1".to_string(), None);
        hierarchy.add_object("obj2".to_string(), Some("obj1".to_string()));
        hierarchy.add_object("obj3".to_string(), Some("obj2".to_string()));

        // Try to create a cycle: obj1 → obj2 → obj3 → obj1
        assert!(!hierarchy.reparent_object(&"obj1".to_string(), Some("obj3".to_string())));

        // Try to parent to self
        assert!(!hierarchy.reparent_object(&"obj1".to_string(), Some("obj1".to_string())));
    }

    #[test]
    fn test_get_descendants() {
        let hierarchy = HierarchyManager::new();

        hierarchy.add_object("root".to_string(), None);
        hierarchy.add_object("child1".to_string(), Some("root".to_string()));
        hierarchy.add_object("child2".to_string(), Some("root".to_string()));
        hierarchy.add_object("grandchild".to_string(), Some("child1".to_string()));

        let descendants = hierarchy.get_descendants_dfs(&"root".to_string());
        assert_eq!(descendants.len(), 3);
        assert!(descendants.contains(&"child1".to_string()));
        assert!(descendants.contains(&"child2".to_string()));
        assert!(descendants.contains(&"grandchild".to_string()));
    }

    #[test]
    fn test_get_depth() {
        let hierarchy = HierarchyManager::new();

        hierarchy.add_object("root".to_string(), None);
        hierarchy.add_object("child".to_string(), Some("root".to_string()));
        hierarchy.add_object("grandchild".to_string(), Some("child".to_string()));

        assert_eq!(hierarchy.get_depth(&"root".to_string()), 0);
        assert_eq!(hierarchy.get_depth(&"child".to_string()), 1);
        assert_eq!(hierarchy.get_depth(&"grandchild".to_string()), 2);
    }
}
