//! Showing level instances: what `SubLevelDisplayComponent` does.
//!
//! The component names a level file. [`sync_level_instances`] makes the world
//! match: for each object with an enabled display it instantiates that file's
//! objects under the object (`RuntimeLevel::load_under`), as generated children
//! (`LevelInstanceMember`): rebuilt from the file, never saved with the level
//! that shows them. The object becomes a rigid frame (a [`SpaceDefiner`]
//! registered here), so moving it moves the whole sublevel with one matrix.
//!
//! The same file may be shown by any number of objects; each gets its own copy
//! of the objects (ids prefixed by the object's id). A file that contains a
//! display of itself, directly or through others, is refused rather than
//! expanded forever.
//!
//! Run it whenever displays may have changed: after a load, after an edit (the
//! editor's command executor does), and from the game's tick. It is idempotent
//! and [`needs_level_instance_sync`] is a cheap read-only check.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use helio_component::components::SubLevelDisplayComponent;
use pulsar_scene_model::attachments::enabled_components;
use pulsar_scene_model::frames::{sync_spaces, SpaceDefiner};
use pulsar_scene_model::level_instances::{clear_members, mark_member};
use pulsar_scene_model::world_ext::SceneWorldExt;
use pulsar_scene_model::Parent;
use pulsar_scenedb::{Entity, World};

use crate::scene::runtime_level::{PreparedLevelFile, RuntimeLevel};

/// How deep displays may nest (a level that shows a level that shows a level ...).
const MAX_DEPTH: usize = 8;

/// What a display object has instantiated: the level it was built from, and
/// whether that worked (a failed load is not retried until the path changes).
/// Runtime only; never saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelInstanceState {
    pub level: String,
    pub loaded: bool,
}

/// The enabled displays: `(owner, level)`. An empty level shows nothing.
fn desired(world: &World) -> HashMap<Entity, String> {
    let mut found = HashMap::new();
    for (_, owner, display) in enabled_components::<SubLevelDisplayComponent>(world) {
        if display.enabled {
            found
                .entry(owner)
                .or_insert_with(|| display.level.trim().to_string());
        }
    }
    found
}

fn display_owners(world: &World) -> Vec<Entity> {
    desired(world).into_keys().collect()
}

inventory::submit! {
    SpaceDefiner {
        name: "sublevel display",
        entities: display_owners,
        is_normalized: None,
        normalize: None,
    }
}

/// The objects whose state differs from what their display asks for.
fn out_of_date(world: &World) -> Vec<Entity> {
    let want = desired(world);
    let mut stale = Vec::new();
    for (owner, level) in &want {
        let have = world.get::<LevelInstanceState>(*owner);
        let up_to_date = match (level.is_empty(), have) {
            (true, None) => true,
            (true, Some(_)) => false,
            (false, Some(state)) => state.level == *level,
            (false, None) => false,
        };
        if !up_to_date {
            stale.push(*owner);
        }
    }
    // A display that was removed or disabled leaves its contents behind.
    for (owner, _) in world.query::<&LevelInstanceState>() {
        if !want.contains_key(&owner) {
            stale.push(owner);
        }
    }
    stale.sort_by_key(|entity| entity.bits());
    stale.dedup();
    stale
}

/// A level file a display needs, on its way from the disk. The reading,
/// parsing, migration and class scan happen on a worker thread; the world is
/// touched only to apply the result.
enum Slot {
    Pending,
    Ready(Arc<PreparedLevelFile>),
    Failed(String),
}

type Fetched = Result<Arc<PreparedLevelFile>, String>;

fn cache() -> MutexGuard<'static, HashMap<PathBuf, Slot>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Slot>>> = OnceLock::new();
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether the display of `level` has nothing to do yet because its file is
/// still being prepared.
fn is_waiting(level: &str) -> bool {
    resolve(level).is_some_and(|path| matches!(cache().get(&path), Some(Slot::Pending)))
}

/// Start preparing `path` on a worker (once: the slot is already `Pending`).
fn start_prepare(path: PathBuf) {
    let worker_path = path.clone();
    let spawned = editor_task_queue::spawn_thread(
        editor_task_queue::ThreadSpec::new("sublevel-prepare", editor_task_queue::ThreadRole::Io)
            .owner("engine_backend"),
        move || {
            let slot = match RuntimeLevel::prepare_for_project(&worker_path) {
                Ok(prepared) => Slot::Ready(Arc::new(prepared)),
                Err(error) => Slot::Failed(error.to_string()),
            };
            cache().insert(worker_path, slot);
        },
    );
    if let Err(error) = spawned {
        cache().insert(path, Slot::Failed(format!("no thread to load it: {error}")));
    }
}

/// How a sync gets the level files: on the spot (`Blocking`: loads, tools,
/// tests), or from a worker that it asks and then moves on from (`Deferred`:
/// every frame thread and the UI).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Blocking,
    Deferred,
}

/// The prepared level at `path`, or `None` while a worker still has it (the
/// display keeps what it shows and is looked at again next sync).
fn fetch(path: &Path, mode: Mode, taken: &mut HashMap<PathBuf, Fetched>) -> Option<Fetched> {
    if let Some(done) = taken.get(path) {
        return Some(done.clone());
    }
    let result: Fetched = match mode {
        Mode::Blocking => RuntimeLevel::prepare_for_project(path)
            .map(Arc::new)
            .map_err(|error| error.to_string()),
        Mode::Deferred => {
            let mut cache = cache();
            match cache.remove(path) {
                Some(Slot::Ready(prepared)) => Ok(prepared),
                Some(Slot::Failed(error)) => Err(error),
                Some(Slot::Pending) => {
                    cache.insert(path.to_path_buf(), Slot::Pending);
                    return None;
                }
                None => {
                    cache.insert(path.to_path_buf(), Slot::Pending);
                    drop(cache);
                    start_prepare(path.to_path_buf());
                    return None;
                }
            }
        }
    };
    taken.insert(path.to_path_buf(), result.clone());
    Some(result)
}

/// Whether a sync has anything to do. Read-only and cheap. A display whose
/// level file is still being prepared does not count: there is nothing to apply
/// until it arrives, and this is polled every frame.
pub fn needs_level_instance_sync(world: &World) -> bool {
    let want = desired(world);
    out_of_date(world).into_iter().any(|owner| {
        want.get(&owner)
            .filter(|level| !level.is_empty())
            .is_none_or(|level| !is_waiting(level))
    })
}

/// The path of a level file named by a display: project-relative, or absolute.
fn resolve(level: &str) -> Option<PathBuf> {
    let path = Path::new(level);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    engine_state::get_project_path().map(|root| Path::new(&root).join(path))
}

/// A level path as comparable text: one separator, no leading `./`.
fn normalized(level: &str) -> String {
    let slashed = level.trim().replace('\\', "/");
    slashed.strip_prefix("./").unwrap_or(&slashed).to_string()
}

/// Why a display cannot show its level (other than the file failing to load).
fn refused(world: &World, owner: Entity, level: &str) -> Option<&'static str> {
    let mut depth = 0;
    let mut at = world.get::<Parent>(owner).map(|p| p.0);
    while let Some(ancestor) = at {
        if let Some(state) = world.get::<LevelInstanceState>(ancestor) {
            if normalized(&state.level) == normalized(level) {
                return Some("it would show itself");
            }
            depth += 1;
        }
        at = world.get::<Parent>(ancestor).map(|p| p.0);
    }
    (depth >= MAX_DEPTH).then_some("levels are nested too deeply")
}

/// One pass: rebuild every display that is out of date. Returns whether
/// anything changed.
fn pass(world: &mut World, mode: Mode, taken: &mut HashMap<PathBuf, Fetched>) -> bool {
    let want = desired(world);
    let mut changed = false;
    for owner in out_of_date(world) {
        let Some(level) = want.get(&owner).filter(|level| !level.is_empty()) else {
            // Removed, disabled or emptied: its contents go.
            clear_members(world, owner);
            world.remove::<LevelInstanceState>(owner);
            changed = true;
            continue;
        };
        let refusal = refused(world, owner, level);
        let path = if refusal.is_none() {
            resolve(level)
        } else {
            None
        };
        // The level file, from a worker in deferred mode. While it is not
        // there, the display keeps what it shows and is visited again.
        let fetched = match &path {
            Some(path) => match fetch(path, mode, taken) {
                Some(fetched) => Some(fetched),
                None => continue,
            },
            None => None,
        };
        clear_members(world, owner);
        world.remove::<LevelInstanceState>(owner);
        changed = true;
        let state = |loaded| LevelInstanceState {
            level: level.clone(),
            loaded,
        };
        if let Some(reason) = refusal {
            tracing::warn!(level = %level, "sublevel display refused: {reason}");
            world.insert(owner, state(false));
            continue;
        }
        let Some(fetched) = fetched else {
            tracing::warn!(level = %level, "sublevel display: no project is open to resolve the path");
            world.insert(owner, state(false));
            continue;
        };
        let prefix = format!("{}/", world.stable_id_of(owner).unwrap_or("instance"));
        let applied = fetched.and_then(|prepared| {
            RuntimeLevel::apply_prepared_under(&prepared, world, owner, &prefix)
                .map_err(|error| error.to_string())
        });
        match applied {
            Ok(spawned) => {
                for entity in spawned {
                    mark_member(world, owner, entity);
                }
                world.insert(owner, state(true));
            }
            Err(error) => {
                tracing::warn!(level = %level, "sublevel display could not load its level: {error}");
                world.insert(owner, state(false));
            }
        }
    }
    changed
}

static APPLIED: AtomicU64 = AtomicU64::new(0);

/// How many syncs changed a world's displayed levels, ever. A level that comes
/// from a worker lands during some frame's sync, not in the call that asked
/// for it, so a view of the hierarchy compares this to know it must refresh.
pub fn applied_generation() -> u64 {
    APPLIED.load(Ordering::Relaxed)
}

fn sync(world: &mut World, mode: Mode) -> bool {
    let changed = sync_passes(world, mode);
    if changed {
        APPLIED.fetch_add(1, Ordering::Relaxed);
    }
    changed
}

fn sync_passes(world: &mut World, mode: Mode) -> bool {
    let mut changed = false;
    let mut taken = HashMap::new();
    // A shown level may itself contain displays: follow them, to a bounded depth.
    for _ in 0..=MAX_DEPTH {
        if !pass(world, mode, &mut taken) {
            break;
        }
        changed = true;
        sync_spaces(world);
    }
    changed
}

/// Make every display show its level, reading the files on the spot.
/// **Blocking**: for loads that already wait, tools and tests. Returns whether
/// anything changed.
pub fn sync_level_instances(world: &mut World) -> bool {
    sync(world, Mode::Blocking)
}

/// Make every display show its level without ever waiting for a file: a level
/// that is not read yet is requested from a worker thread and applied by a
/// later call (the frame threads call this every frame while
/// [`needs_level_instance_sync`] says there is work). Returns whether
/// anything changed.
pub fn sync_level_instances_deferred(world: &mut World) -> bool {
    sync(world, Mode::Deferred)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::level_instances::{is_member, members_of};
    use pulsar_scene_model::world_ext::SpawnObject;
    use pulsar_scene_model::Transform;

    fn display_on(world: &mut World, name: &str, level: &str, at: [f64; 3]) -> Entity {
        let object = world
            .spawn_object(SpawnObject::new(name).with_transform(Transform {
                position: at,
                ..Transform::default()
            }))
            .unwrap();
        pulsar_world_registry::attach_value(
            world,
            object,
            SubLevelDisplayComponent {
                level: level.into(),
                ..SubLevelDisplayComponent::default()
            },
        )
        .unwrap();
        object
    }

    /// A level file with a root object, a child of it and a loose object.
    fn write_level(dir: &Path, name: &str, extra: &str) -> String {
        let json = format!(
            r#"{{
            "version": "2.2",
            "objects": [
                {{"id": "hall", "name": "Hall", "object_type": "Folder",
                  "transform": {{"position": [5.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}},
                  "parent": null, "visible": true, "locked": false, "props": {{}}}},
                {{"id": "lamp", "name": "Lamp", "object_type": "Empty",
                  "transform": {{"position": [1.0, 2.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}},
                  "parent": "hall", "visible": true, "locked": false, "props": {{}}}}
                {extra}
            ],
            "components": {{}}, "metadata": {{}}
        }}"#
        );
        let file = dir.join(name);
        std::fs::write(&file, json).unwrap();
        file.to_string_lossy().into_owned()
    }

    #[test]
    fn a_display_shows_its_level_at_the_objects_origin_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let level = write_level(dir.path(), "room.level", "");
        let mut world = World::new();
        let shown = display_on(&mut world, "door", &level, [100.0, 0.0, 0.0]);
        let other = display_on(&mut world, "window", &level, [-50.0, 0.0, 0.0]);

        assert!(needs_level_instance_sync(&world));
        assert!(sync_level_instances(&mut world));
        assert!(!needs_level_instance_sync(&world));

        // Each display got its own copy of the level's two objects.
        assert_eq!(members_of(&world, shown).len(), 2);
        assert_eq!(members_of(&world, other).len(), 2);
        let door_id = world.stable_id_of(shown).unwrap().to_string();
        let hall = world
            .entity_for(&format!("{door_id}/hall"))
            .expect("ids are prefixed by the display's id");
        assert!(is_member(&world, hall));
        assert_eq!(
            world.parent_of(hall),
            Some(shown),
            "the level's origin is the object's origin"
        );
        // The level's own transforms are kept, relative to that origin.
        assert_eq!(
            world.get::<Transform>(hall).unwrap().position,
            [5.0, 0.0, 0.0]
        );
        let lamp = world.entity_for(&format!("{door_id}/lamp")).unwrap();
        assert_eq!(world.parent_of(lamp), Some(hall));
        // Placed by one matrix: the display object is a space.
        assert!(world.get::<pulsar_scene_model::SpaceSlot>(shown).is_some());

        // Disabling the display removes its contents.
        let attached = pulsar_scene_model::attachments::instances(&world, shown)[0];
        world
            .get_mut::<SubLevelDisplayComponent>(attached)
            .unwrap()
            .enabled = false;
        assert!(sync_level_instances(&mut world));
        assert!(members_of(&world, shown).is_empty());
        assert_eq!(
            members_of(&world, other).len(),
            2,
            "the other display is untouched"
        );
        assert!(world.entity_for(&format!("{door_id}/hall")).is_none());
    }

    /// The frame-thread sync never waits for the file: the first call only asks a
    /// worker, nothing is shown yet, and the display shows its level once a
    /// later call finds the file ready.
    #[test]
    fn a_deferred_sync_asks_a_worker_and_applies_the_level_when_it_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let level = write_level(dir.path(), "deferred.level", "");
        let mut world = World::new();
        let shown = display_on(&mut world, "door", &level, [0.0; 3]);

        assert!(
            !sync_level_instances_deferred(&mut world),
            "the file is not there on the first call"
        );
        assert!(members_of(&world, shown).is_empty());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while members_of(&world, shown).is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "the worker never delivered the level"
            );
            if needs_level_instance_sync(&world) {
                sync_level_instances_deferred(&mut world);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(members_of(&world, shown).len(), 2);
        assert!(!needs_level_instance_sync(&world));
    }

    #[test]
    fn a_missing_level_is_remembered_not_retried_until_the_path_changes() {
        let mut world = World::new();
        let object = display_on(&mut world, "door", "no/such.level", [0.0; 3]);
        assert!(sync_level_instances(&mut world));
        assert_eq!(
            world.get::<LevelInstanceState>(object),
            Some(&LevelInstanceState {
                level: "no/such.level".into(),
                loaded: false
            })
        );
        assert!(
            !needs_level_instance_sync(&world),
            "not retried every frame"
        );
    }

    #[test]
    fn a_level_that_shows_itself_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loop.level");
        // The level's own object holds a display of the very same file.
        let level = path.to_string_lossy().replace('\\', "/");
        let json = format!(
            r#"{{"version": "2.2", "objects": [
                {{"id": "self", "name": "Self", "object_type": "Empty",
                  "transform": {{"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}},
                  "parent": null, "visible": true, "locked": false, "props": {{}}}}],
               "components": {{"self": [{{"class_name": "SubLevelDisplayComponent", "enabled": true,
                  "data": {{"enabled": true, "level": "{level}"}}}}]}}, "metadata": {{}}}}"#
        );
        std::fs::write(&path, json).unwrap();

        let mut world = World::new();
        let door = display_on(&mut world, "door", &path.to_string_lossy(), [0.0; 3]);
        sync_level_instances(&mut world);
        // One level of contents, and the nested display refused rather than endless.
        let door_id = world.stable_id_of(door).unwrap().to_string();
        let inner = world
            .entity_for(&format!("{door_id}/self"))
            .expect("the first level loaded");
        assert!(is_member(&world, inner));
        assert!(
            members_of(&world, inner).is_empty(),
            "the nested display did not expand"
        );
        assert_eq!(members_of(&world, door).len(), 1);
    }
}
