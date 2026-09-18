//! Reflection component system: attach/remove/enable/duplicate/reorder/parent
//! typed component instances and keep the world side hydrated.

use super::*;

impl SceneDatabase {
    // ── Reflection component system ────────────────────────────────────────

    pub fn add_component(
        &self,
        object_id: &EditorObjectId,
        class_name: String,
        data: serde_json::Value,
    ) {
        self.attach_component_instance(
            object_id,
            ComponentInstance {
                class_name,
                enabled: true,
                data,
            },
            true,
        );
    }

    /// Add a fully specified component instance.
    pub fn add_component_instance(&self, object_id: &EditorObjectId, component: ComponentInstance) {
        self.attach_component_instance(object_id, component, true);
    }

    pub fn remove_component(&self, object_id: &EditorObjectId, component_index: usize) {
        // Preserve the old representative before changing instance order.
        let mut components = self.get_components(object_id);
        if component_index >= components.len() {
            return;
        }
        let class_name = components.remove(component_index).class_name;
        remap_component_parents(&mut components, |parent| {
            if parent == component_index {
                None
            } else {
                Some(if parent > component_index {
                    parent - 1
                } else {
                    parent
                })
            }
        });
        self.component_store
            .replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
        self.record_structural_change(object_id, &class_name);
    }

    /// Enable or disable a component by index.
    pub fn set_component_enabled(
        &self,
        object_id: &EditorObjectId,
        component_index: usize,
        enabled: bool,
    ) -> bool {
        let class_name = self
            .component_store
            .get_components(object_id)
            .get(component_index)
            .map(|c| c.class_name.clone());
        let mut components = self.get_components(object_id);
        let Some(component) = components.get_mut(component_index) else {
            return false;
        };
        if component.enabled == enabled {
            return true;
        }
        if is_scenedb_authority_class(&component.class_name) && component.enabled {
            if let Some(live) = self.get_components(object_id).get(component_index) {
                component.data = live.data.clone();
            }
        }
        component.enabled = enabled;
        self.component_store
            .replace_components(object_id, components);
        let changed = true;
        if changed {
            self.sync_registered_component_props_to_scene_db(object_id);
            if let Some(name) = class_name {
                self.record_structural_change(object_id, &name);
            }
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
        let class_name = component.class_name.clone();
        components.insert(insert_index, component);
        remap_component_parents(&mut components, |parent| {
            Some(if parent >= insert_index {
                parent + 1
            } else {
                parent
            })
        });
        self.component_store
            .replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
        self.record_structural_change(object_id, &class_name);
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
        let class_name = component.class_name.clone();
        components.insert(to_index, component);
        remap_component_parents(&mut components, |parent| {
            Some(if parent == from_index {
                to_index
            } else if from_index < to_index && parent > from_index && parent <= to_index {
                parent - 1
            } else if to_index < from_index && parent >= to_index && parent < from_index {
                parent + 1
            } else {
                parent
            })
        });
        self.component_store
            .replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
        self.record_structural_change(object_id, &class_name);
    }

    /// Every component instance attached to `object_id`, with `data`
    /// resolved *live* off `World` for any class that has a live value
    /// there (Pulsar-Native#561) -- `component_store`'s stored JSON is no
    /// longer trusted for those classes' current field values, only for
    /// which components are attached, their order, and their `enabled`
    /// flag. This is the one choke point both the properties panel
    /// (`attached`) and save-to-disk (`save_to_file_with_editor_camera`)
    /// go through, so fixing it here is enough to make `World` the actual
    /// source of truth for both, without either one needing its own sync
    /// step: `update_live_component_property` writes straight to `World`
    /// and stops there (no component_store write-back at all), and this method
    /// is what makes that edit visible everywhere else that reads
    /// component data, including what eventually gets serialized to disk.
    /// Metadata-only view of the object's attached components: class names,
    /// order, enabled flags and stored JSON, with NO live-World overlay.
    ///
    /// [`Self::get_components`] serializes every World-registered component
    /// (`to_json()`) to overlay fresh values — the right thing for save-to-
    /// disk, but pure waste for callers that only need structure (the
    /// component tree's parent indices) or that read live values themselves
    /// (the property cards batch-read straight from World). Per-render cost
    /// of the properties panel used to scale with C × serialization for
    /// exactly this reason.
    pub fn get_components_metadata(&self, object_id: &EditorObjectId) -> Vec<ComponentInstance> {
        self.component_store.get_components(object_id)
    }

    pub fn get_components(&self, object_id: &EditorObjectId) -> Vec<ComponentInstance> {
        let store = self.store.read();
        Self::components_from_store(&store, object_id)
    }

    pub(super) fn components_from_store(store: &WorldSceneStore, object_id: &str) -> Vec<ComponentInstance> {
        let Some(entity) = store.entity_for(object_id) else {
            return Vec::new();
        };
        let mut components = store
            .world()
            .get::<engine_backend::scene::ComponentAttachments>(entity)
            .map(|attachments| attachments.0.clone())
            .unwrap_or_default();
        // Overlay ONLY onto each class's one live-typed instance
        // (Pulsar-Native#519): `World` holds a single typed value per
        // `(entity, ComponentId)` -- the first enabled instance -- so
        // stamping it onto EVERY instance of the class used to clobber the
        // other duplicates' own stored field values on every read. A
        // duplicate's `data` is its own blob; if it becomes the live-typed
        // one later, re-hydration adopts exactly that blob.
        let mut live_index_of_class: HashMap<String, usize> = HashMap::new();
        for (idx, component) in components.iter().enumerate() {
            if !component.enabled {
                continue;
            }
            if pulsar_world_registry::component_id_for_class(&component.class_name).is_some() {
                live_index_of_class
                    .entry(component.class_name.clone())
                    .or_insert(idx);
            }
        }
        for (idx, component) in components.iter_mut().enumerate() {
            if live_index_of_class.get(component.class_name.as_str()) != Some(&idx) {
                continue;
            }
            if let Some(live) = pulsar_world_registry::get_world_component_as_engine_class(
                component.class_name.as_str(),
                store.world(),
                entity,
            ) {
                match live.to_json() {
                    Ok(json) => component.data = overlay_live_data(&component.data, json),
                    Err(error) => tracing::warn!(
                        "[GET_COMPONENTS] '{}' on '{object_id}' has a live World value but \
                         failed to serialize it, keeping the last-known-good stored copy: {error}",
                        component.class_name
                    ),
                }
            }
        }
        components
    }

    /// Check if a component is a descendant of another component
    pub(super) fn is_component_descendant(
        components: &[ComponentInstance],
        potential_descendant: usize,
        potential_ancestor: usize,
    ) -> bool {
        let mut current = potential_descendant;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current) {
                return true;
            }
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
        self.component_store
            .replace_components(object_id, components);
        self.sync_registered_component_props_to_scene_db(object_id);
    }
}