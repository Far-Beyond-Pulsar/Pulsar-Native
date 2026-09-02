//! Component-instance storage bridging `SceneDatabase` to Helio Scene.
//!
//! Historically this type also owned its own object registry, hierarchy, and
//! selection state (each duplicating what `WorldSceneStore` -- the real,
//! live-editor store -- already does). That surface was never actually
//! reachable from production code (`SceneDatabase` only ever called into
//! this type for component-instance storage; its own object/hierarchy/
//! selection methods had zero external callers, confirmed by a full
//! call-site audit) and was deleted rather than kept "just in case" -- see
//! Pulsar-Native's level-editor state-system cleanup. What's left here is
//! exactly what's real: the JSON component-instance list per object
//! (`ComponentDb`), which is genuinely load-bearing storage for any
//! component class that isn't yet a typed `World` component via
//! `pulsar_world_registry` (Phase B4/B5).

use super::component_db::ComponentDb;
use super::metadata::EditorObjectId;

/// Component-instance database -- the bridge between the editor's
/// `SceneDatabase` and per-object JSON component storage.
///
/// Object identity, hierarchy, transforms, and selection all live in
/// `WorldSceneStore`/`pulsar_scenedb::World` now; this type owns nothing
/// about any of that.
#[derive(Clone)]
pub struct SceneMetadataDb {
    components: ComponentDb,
}

impl SceneMetadataDb {
    /// Create a new metadata database.
    pub fn new() -> Self {
        Self {
            components: ComponentDb::new(),
        }
    }

    // ── Component Access ──────────────────────────────────────────────────

    /// Add a component to an object.
    pub fn add_component(
        &self,
        object_id: &EditorObjectId,
        class_name: String,
        data: serde_json::Value,
    ) {
        self.components.add_component(object_id, class_name, data);
    }

    /// Remove a component by index.
    pub fn remove_component(&self, object_id: &EditorObjectId, component_index: usize) -> bool {
        self.components.remove_component(object_id, component_index)
    }

    /// Get all components for an object.
    pub fn get_components(
        &self,
        object_id: &EditorObjectId,
    ) -> Vec<super::metadata::ComponentInstance> {
        self.components.get_components(object_id)
    }

    /// Add a fully specified component instance.
    pub fn add_component_instance(
        &self,
        object_id: &EditorObjectId,
        component: super::metadata::ComponentInstance,
    ) {
        self.components.add_component_instance(object_id, component);
    }

    /// Get component database for direct access.
    pub fn components(&self) -> &ComponentDb {
        &self.components
    }

    /// Clear all components from an object.
    pub fn clear_components(&self, object_id: &EditorObjectId) {
        self.components.clear_components(object_id);
    }

    /// Update a component's enabled flag.
    pub fn set_component_enabled(
        &self,
        object_id: &EditorObjectId,
        component_index: usize,
        enabled: bool,
    ) -> bool {
        self.components
            .set_component_enabled(object_id, component_index, enabled)
    }

    /// Replace all components on an object.
    pub fn replace_components(
        &self,
        object_id: &EditorObjectId,
        components: Vec<super::metadata::ComponentInstance>,
    ) {
        self.components.replace_components(object_id, components);
    }

    /// Clear all component data.
    pub fn clear(&self) {
        self.components.clear_all();
    }
}

impl Default for SceneMetadataDb {
    fn default() -> Self {
        Self::new()
    }
}
