//! Placed class instances follow their class in every world (SceneDB#84).
//!
//! A placed class is a root entity with a `ClassInstance` component and the
//! children built from the class (see `pulsar_class::world`). When a class
//! asset changes (an [`AssetUpdated`](pulsar_events::AssetUpdated) of kind
//! Blueprint), every instance of it is rebuilt from the new definition,
//! keeping the instance's overrides. This lives here, beside the other asset
//! reloads, so a game loop's world and a headless one follow class edits as
//! the editor's does: [`super::reload_updated_asset`] calls it.
//!
//! The editor wraps these with its object ids and its UI bookkeeping
//! (`ui_level_editor::scene_edit::classes`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use pulsar_class::world as class_world;
use pulsar_class::{ClassDefinition, ClassId, ClassInstance, ClassRegistry};
use pulsar_scenedb::{Entity, World};

use super::{SceneWorldExt, StableId};

/// The current project's classes (scanned from `src/classes`). Empty when
/// no project is open.
pub fn project_class_registry() -> ClassRegistry {
    let root = engine_state::get_project_path()
        .map(PathBuf::from)
        .or_else(|| {
            fallback_project_root()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
        });
    match root {
        Some(root) => ClassRegistry::scan(&root),
        None => ClassRegistry::default(),
    }
}

/// Project root used by [`project_class_registry`] when no engine context
/// has one (command-line tools, tests).
fn fallback_project_root() -> &'static Mutex<Option<PathBuf>> {
    static ROOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    ROOT.get_or_init(Default::default)
}

/// Set the project root [`project_class_registry`] falls back to when the
/// engine context has none.
pub fn set_fallback_project_root(root: Option<PathBuf>) {
    *fallback_project_root()
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = root;
}

/// The registry for a class directory: every class of the project it
/// lives in (`<project>/src/classes/<Name>` or a `<Name>.class/` folder
/// anywhere in the project), else just its parent directory's classes.
pub fn registry_for_class_dir(class_dir: &Path) -> ClassRegistry {
    if let Some(root) = pulsar_class::project_root_of_class_dir(class_dir) {
        let registry = ClassRegistry::scan(&root);
        if registry
            .entries()
            .iter()
            .any(|e| same_dir(&e.dir, class_dir))
        {
            return registry;
        }
    }
    match class_dir.parent() {
        Some(parent) => {
            let mut dirs: Vec<PathBuf> = std::fs::read_dir(parent)
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| pulsar_class::registry::is_class_dir(p))
                        .collect()
                })
                .unwrap_or_default();
            if !dirs.iter().any(|d| same_dir(d, class_dir))
                && pulsar_class::registry::is_class_dir(class_dir)
            {
                dirs.push(class_dir.to_path_buf());
            }
            ClassRegistry::from_dirs(dirs)
        }
        None => ClassRegistry::default(),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

// ── Definitions instances were built from ───────────────────────────────────

type BuiltDefinitions = Mutex<HashMap<(usize, ClassId), ClassDefinition>>;

/// Definitions instances were last built from, per world and class, so a
/// class update can tell an instance's own edits from old class defaults.
/// Keyed by world as well (Pulsar-Native#931): with two worlds holding
/// instances of one class, the first one rebuilt must not record the new
/// definition for the other, whose instances were still built from the old
/// one. A world is keyed by its address, stable while it lives; every way
/// an instance appears (placement, rebuild, level load) records its
/// definition, so an address a later world reuses never yields a stale
/// entry for a live instance.
fn built_definitions() -> &'static BuiltDefinitions {
    static BUILT: OnceLock<BuiltDefinitions> = OnceLock::new();
    BUILT.get_or_init(Default::default)
}

fn world_key(world: &World) -> usize {
    world as *const World as usize
}

/// Record that `world`'s instances of `def`'s class are built from `def`.
pub fn remember_built_definition(world: &World, def: &ClassDefinition) {
    built_definitions()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert((world_key(world), def.id.clone()), def.clone());
}

/// The definition `world`'s instances of class `id` were last built from.
pub fn built_definition(world: &World, id: &ClassId) -> Option<ClassDefinition> {
    built_definitions()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(world_key(world), id.clone()))
        .cloned()
}

// ── Rebuilding ──────────────────────────────────────────────────────────────

/// `root` and its generated children.
pub fn instance_entities(world: &World, root: Entity) -> Vec<Entity> {
    std::iter::once(root)
        .chain(class_world::generated_children(world, root))
        .collect()
}

/// Rebuild the instance at `root` from its class's current definition,
/// keeping its overrides: generated children are removed and recreated.
/// Returns `root` and its new children, or `None` when the class cannot be
/// resolved.
pub fn rebuild_class_instance(
    world: &mut World,
    root: Entity,
    registry: &ClassRegistry,
) -> Option<Vec<Entity>> {
    let instance = class_world::class_instance_of(world, root)?;
    let def = registry.definition_for(&instance)?;
    for child in class_world::generated_children(world, root)
        .into_iter()
        .rev()
    {
        // Despawning the tree detaches its component instances, and despawn
        // clears each entity's GPU rows.
        world.despawn_tree(child);
    }
    if instance.class != def.id {
        let mut fixed = instance;
        fixed.class = def.id.clone();
        fixed.class_name = def.name.clone();
        class_world::store_class_instance(world, root, &fixed);
    }
    class_world::expand_class_instance(world, root, &def);
    remember_built_definition(world, &def);
    Some(instance_entities(world, root))
}

/// The class instance roots in `world`.
pub fn class_roots(world: &World) -> Vec<Entity> {
    world
        .query::<&StableId>()
        .map(|(entity, _)| entity)
        .filter(|entity| class_world::is_class_root(world, *entity))
        .collect()
}

/// Whether `event` names the class `instance` refers to (by GUID, else by
/// class directory name).
pub fn event_names_class(
    event: &pulsar_events::AssetUpdated,
    instance: &ClassInstance,
    registry: &ClassRegistry,
) -> bool {
    if let Some(id) = &event.id {
        if instance.class.as_str() == id {
            return true;
        }
    }
    let Some(dir_name) = event
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    else {
        return false;
    };
    registry
        .resolve(instance)
        .map(|entry| entry.name == dir_name)
        .unwrap_or(instance.class_name == dir_name)
}

/// Rebuild every instance in `world` of the class a Blueprint asset update
/// names from the class's new definition, keeping each instance's
/// overrides (current edits included). Returns the rebuilt entities (roots
/// and their generated children). Events for other asset kinds are ignored.
pub fn reload_class_instances(
    world: &mut World,
    event: &pulsar_events::AssetUpdated,
) -> Vec<Entity> {
    if event.kind != pulsar_events::AssetKind::Blueprint {
        return Vec::new();
    }
    let registry = match &event.path {
        Some(dir) if dir.is_dir() => registry_for_class_dir(dir),
        _ => project_class_registry(),
    };
    // First fold every matching instance's live values into its overrides,
    // diffed against the definition it was built from (the new one would
    // turn every value it did not override into one); only then rebuild,
    // which records the new definition as the one built from. Slot ids are
    // stable across class edits, so the overrides apply to the new one.
    let mut matching = Vec::new();
    for root in class_roots(world) {
        let Some(stored) = class_world::class_instance_of(world, root) else {
            continue;
        };
        if !event_names_class(event, &stored, &registry) {
            continue;
        }
        let Some(entry) = registry.resolve(&stored) else {
            continue;
        };
        if let Some(built_from) = built_definition(world, &entry.id) {
            let current = class_world::collect_overrides(world, root, &built_from);
            class_world::store_class_instance(world, root, &current);
        }
        matching.push(root);
    }
    let mut touched = Vec::new();
    for root in matching {
        if let Some(entities) = rebuild_class_instance(world, root, &registry) {
            touched.extend(entities);
        }
    }
    touched
}
