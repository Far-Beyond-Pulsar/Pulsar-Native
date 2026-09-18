use super::super::*;

impl SceneDatabase {

    /// Update a single component's data by index.
    ///
    /// This is the correct entry point for component edits; World-authoritative
    /// classes are hydrated directly and legacy classes retain the existing
    /// metadata-backed behavior. Callers must not access `component_store`
    /// directly.
    pub fn update_component(
        &self,
        object_id: &ObjectId,
        component_index: usize,
        data: serde_json::Value,
    ) {
        let component = self
            .component_store
            .get_components(object_id)
            .get(component_index)
            .cloned();
        if let Some(component) = component.as_ref() {
            if component.enabled
                && is_scenedb_authority_class(&component.class_name)
                && self.live_typed_component_index(object_id, &component.class_name)
                    == Some(component_index)
                && self.hydrate_canonical_component(object_id, &component.class_name, &data)
            {
                self.component_store.update_component(
                    object_id,
                    component_index,
                    attachment_data(&component.data),
                );
                self.sync_registered_component_props_to_scene_db(object_id);
                self.record_structural_change(object_id, &component.class_name);
                return;
            }
        }

        let ok = self
            .component_store
            .update_component(object_id, component_index, data);
        if !ok {
            tracing::warn!(
                "[UPDATE_COMPONENT] component_store.update_component returned false for {object_id} idx={component_index}"
            );
        }
        self.sync_registered_component_props_to_scene_db(object_id);
    }

    /// Update a single property inside a reflection-based component by class name and property name.
    ///
    /// Legacy flat-JSON path -- kept only for classes that were never
    /// migrated to `pulsar_world_registry` (no `ComponentRuntimeBehavior`,
    /// e.g. `LODComponent`/`MaterialOverrideComponent`), where JSON in
    /// `component_store` genuinely is the only representation that exists.
    /// **Do not call this for anything that supports
    /// [`Self::update_live_component_property`]** -- it writes `new_value`
    /// at the top level of the component's JSON, which is wrong for any
    /// `#[sub_props]`-nested field (silently dropped, or worse, overwrites a
    /// nested sub-struct with a bare scalar/array if the names happen to
    /// collide). See Pulsar-Native#561.
    ///
    /// TODO(Pulsar-Native#561): delete this method (and the flat-JSON
    /// fallback branches in `property_renderer.rs`/`material_section.rs`
    /// that call it) once every component -- including the props-only ones
    /// with no `ComponentRuntimeBehavior` today -- is `World`-registered.
    /// This is a shrinking legacy path for a handful of not-yet-migrated
    /// classes, not a permanent second way to edit components; the end
    /// state is that `SceneDatabase`/`World` is the only live component
    /// storage, full stop.
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
            self.record_property_change(object_id, class_name, prop_name);
        }
    }

    /// Which instance of `class_name` on `object_id` is the **live-typed**
    /// one -- the single instance whose value actually lives in `World`.
    ///
    /// `World` stores one value per `(entity, ComponentId)`, so of N
    /// instances of the same class on one entity exactly ONE can be
    /// live-typed: the first ENABLED one (the same instance
    /// [`Self::sync_registered_component_props_to_scene_db`] hydrates).
    /// Every other instance exists only as its own JSON blob in
    /// `component_store`. `None` when the class isn't `World`-registered at all,
    /// or no enabled instance of it is attached.
    ///
    /// This is Pulsar-Native#519's identity anchor: the properties panel
    /// uses it to decide, per card, whether values come from `World` (live
    /// card, subscribable) or from that card's own metadata JSON.
    pub fn live_typed_component_index(
        &self,
        object_id: &ObjectId,
        class_name: &str,
    ) -> Option<usize> {
        if pulsar_world_registry::component_id_for_class(class_name).is_none() {
            return None;
        }
        self.get_components(object_id)
            .iter()
            .enumerate()
            .find(|(_, c)| c.class_name == class_name && c.enabled)
            .map(|(idx, _)| idx)
    }

    /// Edit a single property on ONE specific component instance,
    /// correctly handling `#[sub_props]` nesting (Pulsar-Native#561) and
    /// per-instance field values (Pulsar-Native#519).
    ///
    /// `component_index` addresses the exact instance in the object's
    /// component list -- the same identity `remove_component`/
    /// `set_component_enabled`/`reorder_component` already use. An object
    /// can carry several instances of the same class, each with independent
    /// field values; routing edits by class name alone (the pre-#519
    /// behavior) made every such edit land in whichever instance sorted
    /// first.
    ///
    /// Routing by instance:
    /// - **The live-typed instance** ([`Self::live_typed_component_index`]):
    ///   the setter closure runs straight against the `World`-resident
    ///   typed value -- no JSON anywhere on this path -- then the full new
    ///   shape is persisted back into that instance's `component_store` JSON so
    ///   the two never diverge (Pulsar-Native#561, Bug B).
    /// - **Every other instance** (duplicate duplicates, disabled
    ///   representatives, and classes with no `World` registration): the
    ///   value is serialized once and merged into THAT instance's own JSON
    ///   blob. There is no `World` presence to keep in sync; if the
    ///   instance ever becomes the live-typed one (an earlier duplicate is
    ///   removed or disabled), re-hydration reads exactly this JSON.
    ///
    /// `class_name`/`prop_name` come from [`pulsar_reflection::PropertyMetadata`]
    /// (the same reflection metadata the properties panel already reads to
    /// render the row). The setter closure is looked up fresh from a
    /// throwaway `REGISTRY.create_instance` -- that instance's own field
    /// values are discarded immediately; only its *type-bound*
    /// getter/setter closures are used.
    ///
    /// `Err(new_value)` -- handing the value straight back, since nothing
    /// was written -- when the index/class pair doesn't match the object's
    /// actual component list, or the class has no reflection metadata here
    /// at all (plugin-only classes; the command layer's legacy flat-JSON
    /// fallback covers those).
    pub fn update_live_component_property(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        component_index: usize,
        prop_name: &str,
        new_value: Box<dyn Any + Send>,
    ) -> Result<(), Box<dyn Any + Send>> {
        // The index IS the identity: a stale or mismatched one must never
        // land an edit into some OTHER instance's storage.
        let components = self.get_components(object_id);
        let Some(target) = components.get(component_index) else {
            return Err(new_value);
        };
        if target.class_name != class_name {
            tracing::warn!(
                "[LIVE_PROPERTY_EDIT] index {component_index} holds '{}' not '{class_name}' -- edit refused",
                target.class_name
            );
            return Err(new_value);
        }

        let Some(prop_meta) = pulsar_reflection::REGISTRY
            .create_instance(class_name)
            .and_then(|instance| {
                instance
                    .get_properties()
                    .into_iter()
                    .find(|p| p.name == prop_name)
            })
        else {
            tracing::warn!(
                "[LIVE_PROPERTY_EDIT] no reflected property '{prop_name}' on '{class_name}'"
            );
            return Err(new_value);
        };

        // Non-live instances: apply the edit through the real typed
        // machinery against a THROWAWAY World seeded from this instance's
        // own JSON, then persist the full result back to that same blob. A
        // flat `{prop_name: value}` merge here would be wrong for any
        // `#[sub_props]`-nested leaf (Pulsar-Native#561's corruption class:
        // bare scalar landing where a nested group lives), and duplicates
        // deserve the same nesting-correct write the live card gets --
        // that's the whole point of #519.
        let is_live =
            self.live_typed_component_index(object_id, class_name) == Some(component_index);
        if !is_live {
            let mut scratch = pulsar_scenedb::World::new();
            let scratch_entity = scratch.spawn();
            let hydrated = pulsar_world_registry::hydrate_world_component_for_class(
                class_name,
                &mut scratch,
                scratch_entity,
                &target.data,
            );
            if hydrated.is_err() {
                // This instance's stored JSON doesn't deserialize for its
                // own class -- refuse the edit rather than guess.
                return Err(new_value);
            }
            let Some(instance) = pulsar_world_registry::get_world_component_as_engine_class_mut(
                class_name,
                &mut scratch,
                scratch_entity,
            ) else {
                // Hydrate was a no-op: this class has no `World` bridge at
                // all (plugin-only). Hand the value back untouched so the
                // command layer's legacy flat-JSON fallback can take it.
                return Err(new_value);
            };
            (prop_meta.setter)(instance, new_value);
            let Ok(value_json) = instance.to_json() else {
                return Err(Box::new(()));
            };
            self.component_store
                .update_component(object_id, component_index, value_json);
            self.record_property_change(object_id, class_name, prop_name);
            return Ok(());
        }

        let setter = prop_meta.setter;

        // Scoped so the `store` write-guard is dropped before the
        // `component_store` persistence step below -- that step goes through
        // `self.get_components`, which takes its own `self.store.read()`;
        // `parking_lot::RwLock` isn't reentrant, so holding this write guard
        // across that call would deadlock.
        let persisted_json = {
            let mut store = self.store.write();
            let Some(entity) = store.entity_for(object_id) else {
                return Err(new_value);
            };
            let Some(instance) = pulsar_world_registry::get_world_component_as_engine_class_mut(
                class_name,
                store.world_mut(),
                entity,
            ) else {
                return Err(new_value);
            };
            (setter)(instance, new_value);
            // Record the property change for the properties panel's change set
            // so it can skip re-reading unchanged properties.
            self.record_property_change(object_id, class_name, prop_name);
            // This was the actual bug behind "the properties panel shows the
            // right value but the light in the scene never changes": mutating
            // the live World component directly (above) is correct and
            // sufficient for anything that reads World directly (this method's
            // own read-side counterpart, `read_live_component_property`) --
            // but the renderer's per-frame sync (`sync_scene`/`sync_scene_delta`
            // in HelioRenderer) is gated entirely on `WorldSceneStore`'s own
            // dirty-tracking/`render_revision` counters, which a raw
            // `get_world_component_as_engine_class_mut` write never touches.
            // Without this, the edit is genuinely live in `World` -- correctly
            // observable by direct reads -- but invisible to the mechanism that
            // decides whether to re-sync Helio's scene at all. `mark_dirty`
            // (`WorldSceneStore::publish` under the hood) is what actually
            // signals the render thread.
            // Capture the component's full current shape while `instance` is
            // still borrowed (must happen before `mark_dirty` below, which
            // needs its own `&mut store` -- `instance` borrows `store`
            // mutably via `world_mut()`, so the two borrows can't overlap).
            let json = instance.to_json().ok();
            store.mark_dirty(
                object_id,
                ObjectDirtyFlags::PROPS | ObjectDirtyFlags::COMPONENTS,
            );
            json
        };

        // Persist back into `component_store` for legacy and non-live instances
        // (Pulsar-Native#561, Bug B). A live migrated class is deliberately
        // excluded below: World is its authority, while metadata keeps only
        // the attachment/order/enabled record. Every other instance is still
        // persisted to the EXACT edited index, not "first with this class" --
        // with duplicates those are different compatibility blobs
        // (Pulsar-Native#519).
        if let Some(json) =
            persisted_json.filter(|_| !(is_live && is_scenedb_authority_class(class_name)))
        {
            self.component_store
                .update_component(object_id, component_index, json);
        }

        Ok(())
    }

    /// Read a single property straight off the **live `World`-resident
    /// component**, correctly handling `#[sub_props]` nesting -- no JSON
    /// involved (Pulsar-Native#561's read-side counterpart to
    /// [`Self::update_live_component_property`]).
    ///
    /// `None` under the same conditions as `update_live_component_property`
    /// (not `World`-registered, no live entity, or not hydrated yet);
    /// callers should fall back to the flat-JSON path or a `Default`
    /// instance in that case.
    pub fn read_live_component_property(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        prop_name: &str,
    ) -> Option<Box<dyn Any>> {
        let getter = pulsar_reflection::REGISTRY
            .create_instance(class_name)
            .and_then(|instance| {
                instance
                    .get_properties()
                    .into_iter()
                    .find(|p| p.name == prop_name)
                    .map(|p| p.getter)
            })?;

        let store = self.store.read();
        let entity = store.entity_for(object_id)?;
        let instance = pulsar_world_registry::get_world_component_as_engine_class(
            class_name,
            store.world(),
            entity,
        )?;
        Some((getter)(instance))
    }

    /// Batch-read every property of a component in one `store.read()`
    /// acquisition.  Returns `None` if the class isn't World-registered,
    /// the entity doesn't exist, or the component isn't hydrated.
    ///
    /// Takes a pre-built property metadata slice (from the cached metadata)
    /// so we don't need to call `create_instance` + `get_properties` again.
    pub fn read_component_properties_batch(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        properties: &[pulsar_reflection::PropertyMetadata],
    ) -> Option<Vec<Box<dyn Any>>> {
        let store = self.store.read();
        let entity = store.entity_for(object_id)?;
        let instance = pulsar_world_registry::get_world_component_as_engine_class(
            class_name,
            store.world(),
            entity,
        )?;
        Some(
            properties
                .iter()
                .map(|prop| (prop.getter)(instance))
                .collect(),
        )
    }

    /// Run a closure with the live World component reference held under a
    /// single `store.read()`.  The closure can call property getters
    /// directly against the component reference without acquiring the lock
    /// again.
    ///
    /// Returns `None` if the class isn't World-registered, the entity
    /// doesn't exist, or the component isn't hydrated.
    pub fn with_world_component<T>(
        &self,
        object_id: &ObjectId,
        class_name: &str,
        f: impl FnOnce(&dyn pulsar_reflection::EngineClass) -> T,
    ) -> Option<T> {
        let store = self.store.read();
        let entity = store.entity_for(object_id)?;
        let instance = pulsar_world_registry::get_world_component_as_engine_class(
            class_name,
            store.world(),
            entity,
        )?;
        Some(f(instance))
    }

    /// Clear the entire scene.
    pub fn clear(&self) {
        // Take the complete indexed object list once. Clearing only roots
        // leaves orphaned scene entities behind when a caller has created an
        // inconsistent parent link; those stale IDs then poison subsequent
        // loads/adds. Despawn is recursive, so one pass over the snapshot is
        // sufficient and does not require a lock per object.
        let object_ids: Vec<ObjectId> = self
            .store
            .read()
            .to_snapshots()
            .into_iter()
            .map(|object| object.stable_id)
            .collect();
        if !object_ids.is_empty() {
            let mut store = self.store.write();
            for id in object_ids {
                if let Some(entity) = store.entity_for(&id) {
                    store.despawn(entity);
                }
            }
        }
        tracing::info!("Scene cleared – ready for new level");
    }

}
