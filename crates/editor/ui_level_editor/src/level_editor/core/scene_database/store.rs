//! Scene store handle, property-change tracking, world subscriptions, and
//! component-hydration helpers.
//!
//! Split out of `scene_database.rs`; these are inherent `SceneDatabase`
//! methods, so the type's private fields stay reachable (child module).

use super::*;

impl SceneDatabase {
    pub fn new() -> Self {
        let store = Arc::new(RwLock::new(WorldSceneStore::new()));
        Self {
            component_store: Arc::new(SceneComponentStore::new(Arc::clone(&store))),
            store,
            property_changes: Arc::new(parking_lot::Mutex::new(PropertyChangeSet::default())),
            revision_tracker: Arc::new(RevisionTracker::default()),
        }
    }

    /// The underlying shared store handle -- for consumers that must hold the
    /// same world the editor mutates (the PIE host handing its world to the
    /// guest, #635; renderer construction, #637). Cloning is cheap; readers
    /// take `.read()`, writers `.write()`.
    pub fn shared_store(&self) -> Arc<RwLock<WorldSceneStore>> {
        Arc::clone(&self.store)
    }

    // ── Property change tracking ─────────────────────────────────────────

    /// Snapshot and clear the accumulated property changes.  Called exactly
    /// once per properties-panel render to decide which values need re-reading
    /// from World.
    pub fn drain_property_changes(&self) -> PropertyChangeSet {
        std::mem::take(&mut *self.property_changes.lock())
    }

    // ── World subscriptions (Pulsar-Native#575, SceneDB#47) ────────────────

    /// Store-swap generation the current `World` belongs to. Subscriptions
    /// live inside the `World` itself, so [`Self::restore_history_snapshot`]
    /// (undo/redo -- the one wholesale `*self.store.write() = new_store`
    /// site, whose epoch bump this reads straight out of `RevisionTracker`)
    /// silently kills every outstanding [`pulsar_scenedb::SubscriptionId`].
    /// A subscriber that caches snapshots against subscriptions MUST compare
    /// this value per frame and re-arm from scratch when it moves; within a
    /// stable epoch subscriptions stay valid forever (or until their entity
    /// despawns).
    pub fn subscriptions_epoch(&self) -> u64 {
        self.revision_tracker
            .epoch
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Arm a World-level change subscription for `(object_id, class_name)`'s
    /// live component -- the properties panel's subscribe-once-per-card
    /// replacement for poll-every-render. The returned
    /// [`pulsar_scenedb::SubscriptionId`] is what
    /// [`Self::take_world_component_events`] events are tagged with; drop it
    /// with [`Self::unsubscribe_component`] when the card unmounts.
    ///
    /// `None` when `class_name` has no `World`-registered component id (the
    /// legacy JSON-only classes), the object has no live entity yet, or the
    /// entity is dead -- all "nothing to subscribe to", not errors; callers
    /// keep polling those cards exactly as they did before.
    pub fn subscribe_component(
        &self,
        object_id: &ObjectId,
        class_name: &str,
    ) -> Option<pulsar_scenedb::SubscriptionId> {
        let cid = pulsar_world_registry::component_id_for_class(class_name)?;
        let mut store = self.store.write();
        let entity = store.entity_for(object_id)?;
        store.world_mut().subscribe_id(entity, cid)
    }

    /// Disarm a previously armed subscription. Idempotent no-op if it is
    /// already gone (unsubscribed, or its whole `World` was swapped out from
    /// under it by undo/redo).
    pub fn unsubscribe_component(&self, sub: pulsar_scenedb::SubscriptionId) {
        self.store.write().world_mut().unsubscribe(sub);
    }

    /// Drain every pending World component-change event (SceneDB#47's
    /// batched delivery). Call once per frame at your frame boundary --
    /// between drains the queue accumulates, bounded by SceneDB's own cap.
    ///
    /// SINGLE-DRAINER CONTRACT: like [`Self::drain_property_changes`], this
    /// empties a shared queue -- today exactly one consumer per frame does
    /// the draining and routes to whoever asked (the properties panel's
    /// component-card host). If a second live-UI subscriber ever appears,
    /// this needs to grow per-consumer fanout before both can coexist;
    /// events discarded by the wrong drainer would strand the other
    /// consumer's cache stale until something else touched it.
    pub fn take_world_component_events(&self) -> Vec<pulsar_scenedb::ComponentChangeEvent> {
        self.store
            .write()
            .world_mut()
            .take_component_change_events()
    }

    /// Record that a specific property was written.  Called from every
    /// mutating method that touches a component property.
    pub(super) fn record_property_change(&self, object_id: &str, class_name: &str, prop_name: &str) {
        let mut changes = self.property_changes.lock();
        // Soft cap: with relevance-gated rendering the set can go several
        // drains' worth of edits without being emptied (nothing forces a
        // component-card render for, say, transform-only edits). Dropping
        // the history past this point is always *safe* — an empty set just
        // makes the panel fall back to reading values it might have skipped.
        if changes.changed.len() >= MAX_PROPERTY_CHANGE_SET {
            *changes = PropertyChangeSet::default();
        }
        changes.changed.insert((
            object_id.to_string(),
            class_name.to_string(),
            prop_name.to_string(),
        ));
    }

    /// Record a structural change (add/remove/reorder/enable-disable) on a
    /// component.
    pub(super) fn record_structural_change(&self, object_id: &str, class_name: &str) {
        let mut changes = self.property_changes.lock();
        if changes.changed.len() >= MAX_PROPERTY_CHANGE_SET {
            *changes = PropertyChangeSet::default();
        }
        changes
            .structural
            .insert((object_id.to_string(), class_name.to_string()));
        changes.components_added_or_removed = true;
    }

    /// Non-consuming check: has anything touching `object_id`'s components
    /// been written since the last [`Self::drain_property_changes`]?
    ///
    /// This is the properties panel's relevance gate — see
    /// `PropertiesPanelWrapper::sync_sections`. Peeking (rather than
    /// draining) keeps the drain contract where it is today: the section's
    /// own render remains the single consumer.
    pub fn has_property_changes_for(&self, object_id: &str) -> bool {
        self.property_changes.lock().touches_object(object_id)
    }

    /// Hydrate one canonical component directly into the entity World.
    ///
    /// This hydrates an explicit edit into the authoritative typed
    /// component. The caller decides whether the attachment record should
    /// retain its input JSON as a dormant compatibility value.
    pub(super) fn hydrate_canonical_component(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        data: &Value,
    ) -> bool {
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(object_id) else {
            return false;
        };
        match pulsar_world_registry::hydrate_world_component_for_class(
            class_name,
            store.world_mut(),
            entity,
            data,
        ) {
            Ok(true) => {
                store.mark_dirty(
                    object_id,
                    ObjectDirtyFlags::PROPS | ObjectDirtyFlags::COMPONENTS,
                );
                true
            }
            Ok(false) => false,
            Err(error) => {
                tracing::error!(
                    "World hydration failed for {class_name} on '{object_id}': {error}"
                );
                false
            }
        }
    }

    /// Attach a component while keeping the public metadata-shaped API
    /// compatible. For registered classes, the first enabled instance is
    /// hydrated into World and the metadata record stores only attachment
    /// state; disabled/failed entries retain JSON for re-enable compatibility.
    pub(super) fn attach_component_instance(
        &self,
        object_id: &EditorObjectId,
        mut component: ComponentInstance,
        record_change: bool,
    ) {
        let class_name = component.class_name.clone();
        if component.enabled
            && is_scenedb_authority_class(&class_name)
            && !self
                .component_store
                .get_components(object_id)
                .iter()
                .any(|existing| existing.enabled && existing.class_name == class_name)
            && self.hydrate_canonical_component(object_id, &class_name, &component.data)
        {
            component.data = attachment_data(&component.data);
        }
        self.component_store
            .add_component_instance(object_id, component);
        self.sync_registered_component_props_to_scene_db(object_id);
        if record_change {
            self.record_structural_change(object_id, &class_name);
        }
    }

    /// Lightweight query: return the list of class names attached to
    /// `object_id`, reading only from `component_store` (no JSON clone, no
    /// `to_json()` serialization).  This replaces `get_components()` in the
    /// properties-panel hot path where only the class name + order are needed
    /// to look up cached property metadata.
    pub fn get_component_class_names(&self, object_id: &EditorObjectId) -> Vec<String> {
        self.component_store
            .get_components(object_id)
            .into_iter()
            .map(|c| c.class_name)
            .collect()
    }

    /// Cheap component count for `object_id` — avoids the full
    /// `get_components()` clone + `to_json()` serialization.
    pub fn component_count(&self, object_id: &EditorObjectId) -> usize {
        self.component_store.get_components(object_id).len()
    }
}