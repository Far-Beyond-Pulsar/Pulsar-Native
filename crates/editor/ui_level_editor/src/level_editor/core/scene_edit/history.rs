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
