//! Editor attachment state stored on the same SceneDB entities as typed components.
use std::sync::Arc;

use parking_lot::RwLock;

use super::{ComponentInstance, ObjectDirtyFlags, WorldSceneStore};

/// Order, enabled state, and dormant/unregistered component payloads.
/// Live registered payloads are read from their typed World components.
#[derive(Clone, Default)]
pub struct ComponentAttachments(pub Vec<ComponentInstance>);

#[derive(Clone)]
pub struct SceneComponentStore {
    store: Arc<RwLock<WorldSceneStore>>,
}

impl SceneComponentStore {
    pub fn new(store: Arc<RwLock<WorldSceneStore>>) -> Self {
        Self { store }
    }

    pub fn get_components(&self, id: &str) -> Vec<ComponentInstance> {
        let store = self.store.read();
        store
            .entity_for(id)
            .and_then(|entity| store.world().get::<ComponentAttachments>(entity))
            .map(|attachments| attachments.0.clone())
            .unwrap_or_default()
    }

    fn edit(&self, id: &str, edit: impl FnOnce(&mut Vec<ComponentInstance>) -> bool) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let mut attachments = store
            .world()
            .get::<ComponentAttachments>(entity)
            .cloned()
            .unwrap_or_default();
        if !edit(&mut attachments.0) {
            return false;
        }
        store.world_mut().insert(entity, attachments);
        store.mark_dirty(id, ObjectDirtyFlags::COMPONENTS | ObjectDirtyFlags::PROPS);
        true
    }

    pub fn add_component_instance(&self, id: &str, component: ComponentInstance) {
        self.edit(id, |items| {
            items.push(component);
            true
        });
    }

    pub fn replace_components(&self, id: &str, components: Vec<ComponentInstance>) {
        self.edit(id, |items| {
            *items = components;
            true
        });
    }

    pub fn remove_component(&self, id: &str, index: usize) -> bool {
        self.edit(id, |items| {
            if index >= items.len() {
                return false;
            }
            items.remove(index);
            true
        })
    }

    pub fn update_component(&self, id: &str, index: usize, data: serde_json::Value) -> bool {
        self.edit(id, |items| {
            let Some(component) = items.get_mut(index) else {
                return false;
            };
            component.data = data;
            true
        })
    }

    pub fn clear_components(&self, id: &str) {
        self.replace_components(id, Vec::new());
    }
}
