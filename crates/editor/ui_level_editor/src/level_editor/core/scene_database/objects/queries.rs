use super::super::*;

impl SceneDatabase {
    // ── Queries ───────────────────────────────────────────────────────────

    /// All objects in depth-first order.
    pub fn get_all_objects(&self) -> Vec<SceneObjectData> {
        let store = self.store.read();
        let mut out = Vec::new();
        Self::collect_dfs(&store, None, &mut out);
        drop(store);
        for obj in &mut out {
            self.merge_component_props(&obj.id, &mut obj.props);
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

    /// Return the hierarchy projection from one WorldSceneStore read.
    /// Keeping roots and objects together prevents the UI from observing two
    /// different revisions during a background mutation.
    pub fn get_hierarchy_snapshot(&self) -> (Vec<SceneObjectData>, Vec<ObjectId>) {
        let store = self.store.read();
        let mut objects = Vec::new();
        Self::collect_dfs(&store, None, &mut objects);
        let root_ids = store
            .children_of(None)
            .iter()
            .filter_map(|&entity| store.stable_id_of(entity).map(str::to_string))
            .collect();
        drop(store);
        for object in &mut objects {
            self.merge_component_props(&object.id, &mut object.props);
        }
        (objects, root_ids)
    }

    /// Number of root-level objects, without building their data.
    pub fn root_count(&self) -> usize {
        self.store.read().children_of(None).len()
    }

    /// Monotonic counter that advances whenever the scene content may have
    /// changed — UI-thread commands, AI tools, AND the render thread's own
    /// direct writes (gizmo-drag release, click-to-select all go through
    /// store mutators that `publish()`).
    ///
    /// The raw counter comes from the current `WorldSceneStore`; this folds
    /// it through [`RevisionTracker`] so the value stays monotonic across
    /// `restore_history_snapshot`'s wholesale store swap (undo/redo), which
    /// resets the raw counter to a fresh deterministic value. Selection is
    /// NOT covered (`select_object` deliberately doesn't publish) — poll it
    /// alongside.
    ///
    /// This is what panel frame pumps compare per tick: one read-lock
    /// acquisition + a couple of atomic ops, vs. re-deriving "did anything I
    /// render change" from content snapshots.
    pub fn store_revision(&self) -> u64 {
        let raw = self.store.read().render_revision();
        let epoch = self
            .revision_tracker
            .epoch
            .load(std::sync::atomic::Ordering::Relaxed);
        self.revision_tracker.fold(epoch, raw)
    }

    // ── Targeted component reads ──────────────────────────────────────────
    //
    // The bound-field editors refresh these values on every scene revision
    // bump under the current selection (gizmo drags, AI edits, typing). Each
    // of these used to go through `get_object`, whose cost is O(scene data):
    // a full `SceneObjectData` clone including the props map AND a
    // `merge_component_props` pass cloning every component instance plus
    // running reflection projection over it — twelve times per bump for the
    // transform + header fields alone. These accessors read exactly one
    // component under one short lock, allocate almost nothing, and never
    // touch component_store.

    /// Just the object's transform — no props merge, no path computation.
    pub fn get_object_transform(&self, id: &ObjectId) -> Option<Transform> {
        let store = self.store.read();
        let entity = store.entity_for(id)?;
        let t = store.transform(entity)?;
        Some(Transform {
            position: t.position,
            rotation: t.rotation,
            scale: t.scale,
        })
    }

    /// Just the object's name.
    pub fn get_object_name(&self, id: &ObjectId) -> Option<String> {
        let store = self.store.read();
        let entity = store.entity_for(id)?;
        Some(store.name(entity)?.to_string())
    }

    /// Just the object's `(visible, locked)` flags.
    pub fn get_object_visibility(&self, id: &ObjectId) -> Option<(bool, bool)> {
        let store = self.store.read();
        let entity = store.entity_for(id)?;
        let v = store.visibility(entity)?;
        Some((v.visible, v.locked))
    }

    /// Single object by ID, `None` if not found.
    pub fn get_object(&self, id: &ObjectId) -> Option<SceneObjectData> {
        let mut data = {
            let store = self.store.read();
            let entity = store.entity_for(id)?;
            Self::entity_to_scene_object_data(&store, entity)
        };
        self.merge_component_props(id, &mut data.props);
        Some(data)
    }

    /// Direct children of `id`.
    pub fn get_children(&self, id: &ObjectId) -> Vec<ObjectId> {
        let store = self.store.read();
        let Some(entity) = store.entity_for(id) else {
            return Vec::new();
        };
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

}
