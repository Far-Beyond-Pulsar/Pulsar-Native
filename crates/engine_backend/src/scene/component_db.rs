//! Component database for managing component instances per scene object

use super::metadata::{ComponentInstance, EditorObjectId};
use dashmap::DashMap;
use std::sync::Arc;

/// Manages component instances attached to scene objects
///
/// This database stores the component data (physics, rendering, gameplay, etc.)
/// that's attached to each editor object. Components are defined using the
/// reflection system and can be from any crate (physics, rendering, gameplay).
pub struct ComponentDb {
    /// Maps EditorObjectId → Vec<ComponentInstance>
    ///
    /// Uses DashMap for lock-free concurrent access. Multiple systems can
    /// query components simultaneously without blocking.
    components: Arc<DashMap<EditorObjectId, Vec<ComponentInstance>>>,
}

impl ComponentDb {
    /// Create a new component database
    pub fn new() -> Self {
        Self {
            components: Arc::new(DashMap::new()),
        }
    }

    /// Add a component to an object
    ///
    /// The component is stored as serialized JSON. In the full implementation,
    /// this would accept `Box<dyn EngineClass>` and serialize it automatically.
    pub fn add_component(
        &self,
        object_id: &EditorObjectId,
        class_name: String,
        data: serde_json::Value,
    ) {
        let instance = ComponentInstance { class_name, data };

        self.components
            .entry(object_id.clone())
            .or_insert_with(Vec::new)
            .push(instance);
    }

    /// Remove a component from an object by index
    ///
    /// Returns true if the component was removed, false if the index was invalid.
    pub fn remove_component(&self, object_id: &EditorObjectId, component_index: usize) -> bool {
        if let Some(mut components) = self.components.get_mut(object_id) {
            if component_index < components.len() {
                components.remove(component_index);
                return true;
            }
        }
        false
    }

    /// Remove a component from an object by class name
    ///
    /// Removes the first component matching the class name. Returns true if
    /// a component was removed.
    pub fn remove_component_by_class(
        &self,
        object_id: &EditorObjectId,
        class_name: &str,
    ) -> bool {
        if let Some(mut components) = self.components.get_mut(object_id) {
            if let Some(pos) = components.iter().position(|c| c.class_name == class_name) {
                components.remove(pos);
                return true;
            }
        }
        false
    }

    /// Get all components for an object
    ///
    /// Returns a clone of the component list. In a more optimized implementation,
    /// this could return references or use Arc for shared access.
    pub fn get_components(&self, object_id: &EditorObjectId) -> Vec<ComponentInstance> {
        self.components
            .get(object_id)
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// Get a specific component by index
    pub fn get_component(
        &self,
        object_id: &EditorObjectId,
        component_index: usize,
    ) -> Option<ComponentInstance> {
        self.components
            .get(object_id)
            .and_then(|components| components.get(component_index).cloned())
    }

    /// Get a component by class name
    ///
    /// Returns the first component matching the class name.
    pub fn get_component_by_class(
        &self,
        object_id: &EditorObjectId,
        class_name: &str,
    ) -> Option<ComponentInstance> {
        self.components
            .get(object_id)
            .and_then(|components| {
                components
                    .iter()
                    .find(|c| c.class_name == class_name)
                    .cloned()
            })
    }

    /// Check if an object has a specific component class
    pub fn has_component(&self, object_id: &EditorObjectId, class_name: &str) -> bool {
        self.components
            .get(object_id)
            .map(|components| components.iter().any(|c| c.class_name == class_name))
            .unwrap_or(false)
    }

    /// Get the number of components on an object
    pub fn component_count(&self, object_id: &EditorObjectId) -> usize {
        self.components
            .get(object_id)
            .map(|c| c.len())
            .unwrap_or(0)
    }

    /// Update a component's data
    ///
    /// Returns true if the component was updated, false if the index was invalid.
    pub fn update_component(
        &self,
        object_id: &EditorObjectId,
        component_index: usize,
        data: serde_json::Value,
    ) -> bool {
        if let Some(mut components) = self.components.get_mut(object_id) {
            if let Some(component) = components.get_mut(component_index) {
                component.data = data;
                return true;
            }
        }
        false
    }

    /// Clear all components from an object
    pub fn clear_components(&self, object_id: &EditorObjectId) {
        self.components.remove(object_id);
    }

    /// Remove all components from all objects (scene clear)
    pub fn clear_all(&self) {
        self.components.clear();
    }

    /// Get all object IDs that have components
    pub fn get_objects_with_components(&self) -> Vec<EditorObjectId> {
        self.components
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Query all objects that have a specific component class
    ///
    /// This is useful for systems that need to iterate over all objects with
    /// a particular component (e.g., physics system finding all objects with PhysicsComponent).
    pub fn query_objects_with_component(&self, class_name: &str) -> Vec<EditorObjectId> {
        self.components
            .iter()
            .filter(|entry| entry.value().iter().any(|c| c.class_name == class_name))
            .map(|entry| entry.key().clone())
            .collect()
    }
}

impl Default for ComponentDb {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ComponentDb {
    fn clone(&self) -> Self {
        Self {
            components: Arc::clone(&self.components),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_components() {
        let db = ComponentDb::new();
        let obj_id = "test_object".to_string();

        // Add a component
        db.add_component(
            &obj_id,
            "PhysicsComponent".to_string(),
            serde_json::json!({"mass": 10.0}),
        );

        // Verify it was added
        assert_eq!(db.component_count(&obj_id), 1);
        assert!(db.has_component(&obj_id, "PhysicsComponent"));

        // Get the component
        let components = db.get_components(&obj_id);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].class_name, "PhysicsComponent");
    }

    #[test]
    fn test_remove_component() {
        let db = ComponentDb::new();
        let obj_id = "test_object".to_string();

        db.add_component(
            &obj_id,
            "PhysicsComponent".to_string(),
            serde_json::json!({}),
        );
        db.add_component(
            &obj_id,
            "LightComponent".to_string(),
            serde_json::json!({}),
        );

        assert_eq!(db.component_count(&obj_id), 2);

        // Remove by index
        assert!(db.remove_component(&obj_id, 0));
        assert_eq!(db.component_count(&obj_id), 1);

        // Remove by class
        assert!(db.remove_component_by_class(&obj_id, "LightComponent"));
        assert_eq!(db.component_count(&obj_id), 0);
    }

    #[test]
    fn test_query_objects_with_component() {
        let db = ComponentDb::new();

        db.add_component(
            &"obj1".to_string(),
            "PhysicsComponent".to_string(),
            serde_json::json!({}),
        );
        db.add_component(
            &"obj2".to_string(),
            "PhysicsComponent".to_string(),
            serde_json::json!({}),
        );
        db.add_component(
            &"obj3".to_string(),
            "LightComponent".to_string(),
            serde_json::json!({}),
        );

        let physics_objects = db.query_objects_with_component("PhysicsComponent");
        assert_eq!(physics_objects.len(), 2);

        let light_objects = db.query_objects_with_component("LightComponent");
        assert_eq!(light_objects.len(), 1);
    }
}
