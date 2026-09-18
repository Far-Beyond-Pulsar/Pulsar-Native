//! Undo/redo history: opaque full-scene capture and restore.

use super::*;

impl SceneDatabase {
    // ── Undo/redo (Pulsar-Native#554) ────────────────────────────────────
    //
    // Deliberately NOT built on `pulsar_scenedb::replication::Snapshot`
    // (`capture_full`/`restore_to_world`): that machinery is shaped for
    // network replication -- every component needs a registered
    // `ReplicationRegistry` schema with per-field encode/decode `FieldOps`,
    // which is real setup cost and a poor fit for `RenderProps`' free-form
    // JSON (`HashMap<String, serde_json::Value>`, `Option<Value>`) -- not
    // impossible, but a whole schema-registration subsystem to stand up for
    // something `WorldSceneStore`'s own bridge already does. `to_snapshots`/
    // `load_from_snapshots` already capture and round-trip everything a
    // scene needs (proven by `world_store.rs`'s own tests), so undo/redo
    // reuses that directly instead.

    /// Capture a full, restorable snapshot of the current scene --
    /// `WorldSceneStore`'s object/transform/hierarchy/render-props state
    /// plus every object's reflection component data from `component_store`
    /// (the two are captured together so a restore can't reintroduce one
    /// half stale relative to the other). Treat the result as opaque; pass
    /// it back to [`Self::restore_history_snapshot`] only.
    pub fn capture_history_snapshot(&self) -> SceneHistorySnapshot {
        let store = self.store.read();
        let objects = store.to_snapshots();
        let components = objects
            .iter()
            .map(|obj| {
                (
                    obj.stable_id.clone(),
                    Self::components_from_store(&store, &obj.stable_id),
                )
            })
            .filter(|(_, components)| !components.is_empty())
            .collect();
        SceneHistorySnapshot {
            objects,
            components,
        }
    }

    /// Restore a previously captured snapshot, replacing the current scene
    /// entirely (`WorldSceneStore` is swapped for a fresh one built from the
    /// snapshot; `component_store` is cleared and repopulated). Entity identity
    /// is NOT preserved across a restore -- nothing outside `WorldSceneStore`
    /// holds a raw `Entity` across calls (every `SceneDatabase` method
    /// resolves `entity_for` fresh), so this is safe. Selection is cleared
    /// (the fresh store has no `selected` entity) -- not preserving it is a
    /// deliberate v1 simplification, not an oversight.
    ///
    /// Returns `Err` (leaving the live scene untouched) only if `snapshot`
    /// itself is malformed -- a forward parent reference, which shouldn't
    /// happen for a snapshot this type itself produced, but is surfaced
    /// rather than silently no-op'd or panicking, since restoring a
    /// generation-old snapshot after intervening schema changes is exactly
    /// the kind of thing that's cheap to guard here and expensive to debug
    /// if it silently corrupted the scene instead.
    pub fn restore_history_snapshot(&self, snapshot: &SceneHistorySnapshot) -> Result<(), String> {
        let mut new_store =
            WorldSceneStore::load_from_snapshots(&snapshot.objects).map_err(|e| e.to_string())?;
        {
            let mut old_store = self.store.write();
            if let Some(mirror) = old_store.world().gpu_mirror().cloned() {
                // Helio retains this handle. Remove old GPU rows before
                // reusing entity indices, then keep that same mirror attached.
                let old_entities: Vec<_> = old_store
                    .world()
                    .query::<&engine_backend::scene::StableId>()
                    .map(|(entity, _)| entity)
                    .collect();
                for entity in old_entities {
                    old_store.despawn(entity);
                }
                new_store.world_mut().attach_gpu_mirror(mirror);
                engine_backend::scene::install_scenedb_inspector(new_store.world_mut());
                let transforms: Vec<_> = new_store
                    .world()
                    .query::<&WorldTransform>()
                    .map(|(entity, transform)| (entity, *transform))
                    .collect();
                for (entity, transform) in transforms {
                    new_store.world_mut().insert(entity, transform);
                }
            }
            *old_store = new_store;
        }
        // The swapped-in store's raw revision counter is unrelated to the
        // old one's (it restarts at a deterministic value) — tell the
        // monotonicizer so `store_revision` keeps advancing across undo/redo.
        self.revision_tracker.note_swap();
        for (object_id, components) in &snapshot.components {
            for component in components {
                self.attach_component_instance(object_id, component.clone(), false);
            }
        }

        Ok(())
    }
}

/// Opaque capture produced by [`SceneDatabase::capture_history_snapshot`],
/// consumed by [`SceneDatabase::restore_history_snapshot`]. See that pair's
/// docs for what it carries and why.
#[derive(Clone, Debug)]
pub struct SceneHistorySnapshot {
    objects: Vec<engine_backend::scene::ObjectSnapshot>,
    components: HashMap<ObjectId, Vec<ComponentInstance>>,
}