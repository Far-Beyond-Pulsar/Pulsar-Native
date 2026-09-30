//! Undo/redo history: opaque full-scene capture and restore.
//!
//! SceneDB has no undo of its own (its README leaves undo history to the editor),
//! and its replication `Snapshot` is shaped for network resync -- every component
//! needs a registered per-field schema, a poor fit for the free-form JSON in
//! `RenderProps`. The editor therefore keeps its own snapshots: a capture is the
//! object list plus every object's component instances, and a restore rebuilds the
//! scene *in place* in the same world, so the GPU mirror, inspector bridge and the
//! world's change counter carry straight through.

use std::collections::HashMap;

use engine_backend::scene::SceneWorldExt;
use pulsar_scenedb::World;

use super::components::{attach_component_instance, get_components};
use super::objects::{clear, collect_dfs, spawn_raw};
use super::{ComponentInstance, ObjectId, SceneObjectData};

/// Opaque capture produced by [`capture_history_snapshot`], consumed by
/// [`restore_history_snapshot`].
#[derive(Clone, Debug)]
pub struct SceneHistorySnapshot {
    objects: Vec<SceneObjectData>,
    components: HashMap<ObjectId, Vec<ComponentInstance>>,
}

/// The before/after state of only the objects touched by one command.
#[derive(Clone, Debug)]
pub struct SceneHistoryDelta {
    pub(crate) before: SceneHistorySnapshot,
    pub(crate) after: SceneHistorySnapshot,
}

impl SceneHistoryDelta {
    pub(crate) fn ids(&self) -> Vec<ObjectId> {
        self.before
            .objects
            .iter()
            .chain(self.after.objects.iter())
            .map(|object| object.id.clone())
            .collect()
    }
}

/// Capture a full, restorable snapshot of the current scene: every object's
/// transform / hierarchy / render props plus its component instances, taken
/// together so a restore can't reintroduce one half stale relative to the other.
pub fn capture_history_snapshot(world: &World) -> SceneHistorySnapshot {
    profiling::profile_scope!("scene_edit::capture_history_snapshot");
    let mut objects = Vec::new();
    collect_dfs(world, None, &mut objects);
    let components = objects
        .iter()
        .map(|obj| (obj.id.clone(), get_components(world, &obj.id)))
        .filter(|(_, components)| !components.is_empty())
        .collect();
    SceneHistorySnapshot {
        objects,
        components,
    }
}

/// Capture only the requested objects and their descendants. This is the edit
/// history path; unlike the play-mode snapshot it never traverses unrelated
/// scene branches.
pub fn capture_history_subset(world: &World, ids: &[ObjectId]) -> SceneHistorySnapshot {
    let mut wanted = std::collections::HashSet::new();
    let mut entities = Vec::new();
    for id in ids {
        let Some(entity) = world.entity_for(id) else {
            continue;
        };
        let mut subtree = vec![entity];
        collect_subtree(world, entity, &mut subtree);
        for entity in subtree {
            if wanted.insert(entity) {
                entities.push(entity);
            }
        }
    }
    let objects = entities
        .iter()
        .map(|&entity| super::objects::entity_to_scene_object_data(world, entity))
        .collect::<Vec<_>>();
    let components = objects
        .iter()
        .map(|object| (object.id.clone(), get_components(world, &object.id)))
        .filter(|(_, components)| !components.is_empty())
        .collect();
    SceneHistorySnapshot {
        objects,
        components,
    }
}

fn collect_subtree(
    world: &World,
    entity: pulsar_scenedb::Entity,
    out: &mut Vec<pulsar_scenedb::Entity>,
) {
    for child in world.children_of(Some(entity)) {
        out.push(child);
        collect_subtree(world, child, out);
    }
}

/// Apply a scoped before/after state. Only entities in the delta are touched.
pub fn restore_history_delta(
    world: &mut World,
    snapshot: &SceneHistorySnapshot,
    scope_ids: &[ObjectId],
) -> Result<(), String> {
    let desired: std::collections::HashSet<_> = snapshot
        .objects
        .iter()
        .map(|object| object.id.as_str())
        .collect();
    for id in scope_ids {
        if !desired.contains(id.as_str()) && world.entity_for(id).is_some() {
            super::objects::remove_object(world, id);
        }
    }

    // The caller supplies a snapshot containing every affected object. Objects
    // absent from it are removed by the delta wrapper before this function.
    for object in &snapshot.objects {
        if world.entity_for(&object.id).is_none() {
            super::objects::spawn_raw(world, object)?;
        } else {
            super::objects::update_object(world, object.clone());
            super::objects::reparent_object(world, &object.id, object.parent.clone());
        }
        super::components::clear_components(world, &object.id);
        if let Some(components) = snapshot.components.get(&object.id) {
            for component in components {
                attach_component_instance(world, &object.id, component.clone(), false);
            }
        }
    }
    Ok(())
}

/// Restore a previously captured snapshot, replacing the current scene entirely.
///
/// Entity identity is NOT preserved across a restore -- objects are addressed by
/// stable id everywhere, so nothing holds a raw `Entity` across calls. Selection is
/// cleared; not preserving it is a deliberate simplification.
///
/// Returns `Err` (leaving the live scene untouched) only if `snapshot` is
/// malformed: a parent that doesn't precede its child, or a duplicate id.
pub fn restore_history_snapshot(
    world: &mut World,
    snapshot: &SceneHistorySnapshot,
) -> Result<(), String> {
    // Validate before touching the world.
    let mut seen = std::collections::HashSet::with_capacity(snapshot.objects.len());
    for obj in &snapshot.objects {
        if !seen.insert(obj.id.as_str()) {
            return Err(format!("duplicate object id '{}'", obj.id));
        }
        if let Some(parent) = &obj.parent {
            if !seen.contains(parent.as_str()) {
                return Err(format!(
                    "object '{}' references parent '{parent}' before it is available",
                    obj.id
                ));
            }
        }
    }

    clear(world);
    for obj in &snapshot.objects {
        spawn_raw(world, obj)?;
    }
    for (object_id, components) in &snapshot.components {
        for component in components {
            attach_component_instance(world, object_id, component.clone(), false);
        }
    }
    Ok(())
}
