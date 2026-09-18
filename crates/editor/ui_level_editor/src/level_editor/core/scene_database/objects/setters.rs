use super::super::*;

impl SceneDatabase {
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
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let mut visibility = store.visibility(entity).unwrap_or_default();
        visibility.visible = visible;
        store.set_visibility(entity, visibility)
    }

    pub fn set_locked(&self, id: &ObjectId, locked: bool) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let mut visibility = store.visibility(entity).unwrap_or_default();
        visibility.locked = locked;
        store.set_visibility(entity, visibility)
    }

    /// Narrow transform update -- writes only `WorldSceneStore`'s own
    /// transform, no full-object `SceneObjectData` round trip. Unlike
    /// `update_object(SceneObjectData)`, this does NOT call
    /// `sync_registered_component_props_to_scene_db` -- a transform never
    /// needs a component re-hydration, so the old whole-object path (used
    /// by `SceneCommand::SetTransform`'s handler before Pulsar-Native#561's
    /// properties-panel rewrite) was re-serializing/re-hydrating every
    /// `World`-registered component on the object on every keystroke of a
    /// position/rotation/scale field, for no reason. `None` fields are left
    /// unchanged; returns `false` if nothing actually changed or the object
    /// doesn't exist.
    pub fn set_transform(
        &self,
        id: &ObjectId,
        position: Option<[f32; 3]>,
        rotation: Option<[f32; 3]>,
        scale: Option<[f32; 3]>,
    ) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let mut transform = store.transform(entity).unwrap_or_default();
        let mut changed = false;
        if let Some(p) = position {
            if transform.position != p {
                transform.position = p;
                changed = true;
            }
        }
        if let Some(r) = rotation {
            if transform.rotation != r {
                transform.rotation = r;
                changed = true;
            }
        }
        if let Some(s) = scale {
            if transform.scale != s {
                transform.scale = s;
                changed = true;
            }
        }
        if !changed {
            return false;
        }
        store.set_transform(entity, transform)
    }

    /// Re-parent an object (cycle-safe).
    pub fn reparent_object(&self, id: &ObjectId, new_parent: Option<ObjectId>) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(id) else {
            return false;
        };
        let new_parent_entity = match new_parent {
            Some(ref parent_id) => match store.entity_for(parent_id) {
                Some(e) => Some(e),
                None => return false,
            },
            None => None,
        };
        store.reparent(entity, new_parent_entity).is_ok()
    }

    /// Reorder two sibling objects by swapping their positions.
    ///
    /// Both objects must have the same parent. Returns false if they don't
    /// share a parent or either id is unknown.
    pub fn reorder_object_siblings(&self, object_id: &ObjectId, target_id: &ObjectId) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(object_id) else {
            return false;
        };
        let Some(target) = store.entity_for(target_id) else {
            return false;
        };
        store.reorder_sibling(entity, target)
    }

    // ── Ordering ──────────────────────────────────────────────────────────

    /// Move an object one step earlier among its siblings (swaps with the
    /// preceding sibling). No-op (returns without effect) if already first.
    pub fn move_object_up(&self, id: &str) {
        let mut store = self.store.write();
        if let Some(entity) = store.entity_for(id) {
            store.move_sibling_up(entity);
        }
    }

    /// Move an object one step later among its siblings (swaps with the
    /// following sibling). No-op if already last.
    pub fn move_object_down(&self, id: &str) {
        let mut store = self.store.write();
        if let Some(entity) = store.entity_for(id) {
            store.move_sibling_down(entity);
        }
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

        self.component_store.clear_components(&new_id);
        for component in source_components {
            self.attach_component_instance(&new_id, component, false);
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
}
