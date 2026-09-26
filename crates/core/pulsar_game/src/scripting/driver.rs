//! The script driver: the script runtime follows the world (#922).
//!
//! A placed class is an entity carrying a [`ClassInstance`] component. The
//! [`ScriptDriver`] watches that component through a SceneDB change journal
//! and keeps exactly one script instance per class instance:
//!
//! - **added** (level load, editor placement during Play-in-Editor, a
//!   script's `world::spawn`): the class's compiled module is loaded if it
//!   is not yet, a script instance is spawned bound to the entity with the
//!   instance's `variable_overrides`, its component-slot handles are bound
//!   ([`bind_class_slots`]) and its `begin_play` is queued;
//! - **removed**, or its entity despawned: `end_play` runs (if `begin_play`
//!   did) and the instance is dropped with its waiting calls;
//! - **re-written** with the same class (the editor rebuilding the
//!   instance's components): the slot handles are bound again, and the
//!   script keeps its state; with a different class, the old instance ends
//!   and a new one starts.
//!
//! The first reconcile opens the journal and scans every `ClassInstance`,
//! which is how a level's instances start, whether the level was loaded
//! before the driver existed (Play-in-Editor adopts the editor's world) or
//! after (standalone). A journal overflow rescans the same way.
//!
//! # One frame
//!
//! [`ScriptDriver::run_frame`] is the script phase:
//!
//! 1. class reloads requested since the last frame are applied;
//! 2. **reconcile**: start and stop instances as above;
//! 3. `begin_play` of every instance started in step 2, in start order;
//! 4. event handler calls queued by the event hub since the last script
//!    phase, in delivery order (see [`super::events`]);
//! 5. `tick` of every running instance, in start order, then due timers;
//! 6. world changes scripts queued ([`WorldCommand`]s: spawn, destroy) are
//!    applied, in the order they were queued.
//!
//! With an event hub attached ([`ScriptDriver::attach_events`], done by
//! the tick loop), instances subscribe to their class's events when they
//! start and drop the subscriptions when they stop, and the driver
//! publishes the lifecycle built-ins: `BeginPlay` after an instance's
//! `begin_play`, `EndPlay` after its `end_play`, `LevelLoaded` once after
//! the first frame's instances began, `EntitySpawned` / `EntityDestroyed`.
//!
//! # Ordering (deterministic)
//!
//! - **Global scripts** ([`ScriptingConfig::global_scripts`]) start first,
//!   in the order the config lists them.
//! - **Instances found by a scan** (the level, or a rescan) start by
//!   hierarchy depth (parents before children), then by StableId.
//! - **Instances added later** start in the order the world recorded them,
//!   so objects spawned by scripts start at the next frame's reconcile in
//!   spawn order.
//! - `end_play` for `world::destroy` runs before the object's components
//!   are removed. When something else despawns an entity (the editor
//!   deleting it during Play-in-Editor), the journal only reports it after
//!   the fact, so that `end_play` runs at the next reconcile with `self`
//!   already dead: component access resolves to nothing (#888), it never
//!   panics.
//!
//! # Identity
//!
//! A bound instance's id is `<entity StableId>::<class GUID>`
//! ([`instance_id_for`]), stable across loads, PIE and standalone runs; a
//! global script's is `global::<class GUID>`. [`ScriptDriver::instance_of`]
//! maps an entity to its instance id (for save games and events).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use engine_backend::scene::{ObjectType, SceneWorldExt, SpawnObject, Transform};
use pulsar_class::{ClassEntry, ClassId, ClassInstance, ClassRegistry, LocalTransform};
use pulsar_scenedb::{ChangeCursor, ChangeRead, ComponentChange, Entity, World};
use pulsar_script_runtime::{RuntimeError, ScriptRuntime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::bind_class_slots;
use super::commands::{CommandScope, WorldCommand};
use super::events::ScriptEvents;
use pulsar_events::builtin::{BeginPlay, EndPlay, EntityDestroyed, EntitySpawned};
use pulsar_events::{entity_channel, EventHub};

/// Rounds of queued world commands applied per frame: `end_play` of a
/// destroyed object may queue more, but a chain never runs forever.
const MAX_COMMAND_ROUNDS: usize = 16;

/// The project's scripting config file, relative to the project root.
pub const SCRIPTING_CONFIG_FILE: &str = "Pulsar/scripting.json";

/// `<project>/Pulsar/scripting.json`.
pub fn scripting_config_path(project_root: &Path) -> PathBuf {
    project_root.join(SCRIPTING_CONFIG_FILE)
}

/// Project scripting settings (`Pulsar/scripting.json`, next to the native
/// prefab config `Pulsar/level.json`). A missing file means the defaults.
///
/// ```json
/// { "global_scripts": ["GameRules", "5b0c…-class-guid"] }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptingConfig {
    /// Classes (GUID or name) that get exactly one instance at startup,
    /// bound to no entity: logic that belongs to the game, not to an
    /// object. Default: none.
    #[serde(default)]
    pub global_scripts: Vec<String>,
}

impl ScriptingConfig {
    /// Read the project's config. Missing: defaults. Unreadable: defaults,
    /// with a warning.
    pub fn load(project_root: &Path) -> Self {
        let path = scripting_config_path(project_root);
        let Ok(bytes) = engine_fs::virtual_fs::read_file(&path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), "Unreadable scripting config; using defaults: {error}");
            Self::default()
        })
    }
}

/// Id of the script instance of class `class` on the object `stable_id`.
pub fn instance_id_for(stable_id: &str, class: &ClassId) -> String {
    format!("{stable_id}::{class}")
}

/// Id of the global script instance of class `class`.
pub fn global_instance_id(class: &ClassId) -> String {
    format!("global::{class}")
}

/// File name of a class's compiled module in the editor's JSON encoding.
pub const MODULE_JSON_FILE: &str = "module.json";
/// File name of a class's compiled module in the binary encoding packaged
/// games ship (#852).
pub const MODULE_BINARY_FILE: &str = "module.pvm";

/// Where a class's compiled script module lives: the binary one when the
/// class has it (packaged content), else the editor's JSON one. Either may
/// be missing. Checked through the virtual filesystem, so a pak counts.
pub fn module_file(entry: &ClassEntry) -> PathBuf {
    let build = entry.dir.join("events").join(".build");
    let binary = build.join(MODULE_BINARY_FILE);
    if engine_fs::virtual_fs::exists(&binary).unwrap_or(false) {
        binary
    } else {
        build.join(MODULE_JSON_FILE)
    }
}

/// Read a compiled module file (JSON or binary) through the virtual
/// filesystem. `None` when it does not exist.
fn read_module_bytes(path: &Path) -> Option<std::io::Result<Vec<u8>>> {
    if !engine_fs::virtual_fs::exists(path).unwrap_or(false) {
        return None;
    }
    Some(engine_fs::virtual_fs::read_file(path).map_err(|e| std::io::Error::other(e.to_string())))
}

/// What one reconcile or frame did.
#[derive(Debug, Default)]
pub struct DriverReport {
    /// Instance ids started, in `begin_play` order.
    pub started: Vec<String>,
    /// Instance ids stopped (their `end_play` ran if they had begun).
    pub stopped: Vec<String>,
    /// Entities built by `world::spawn` / `world::spawn_child`.
    pub spawned: Vec<Entity>,
    /// Entities removed by `world::destroy`.
    pub destroyed: Vec<Entity>,
    /// Errors raised by scripts (per instance, never fatal).
    pub script_errors: Vec<RuntimeError>,
    /// Class modules that did not load or reload (link errors, unreadable
    /// modules), per class.
    pub load_errors: Vec<RuntimeError>,
    /// Waiting calls class reloads had to drop, as `(class, call)`.
    pub dropped_calls: Vec<(String, pulsar_script_runtime::DroppedCall)>,
    /// Instances or commands that could not be applied, as messages.
    pub failures: Vec<String>,
}

impl DriverReport {
    /// Whether the frame raised any error or warning worth reporting.
    pub fn has_problems(&self) -> bool {
        !self.script_errors.is_empty() || !self.load_errors.is_empty() || !self.dropped_calls.is_empty()
    }
}

/// One class instance the driver follows.
#[derive(Clone, Debug)]
struct Tracked {
    class: ClassId,
    class_name: String,
    /// `None` while the class has no compiled script (or cannot be
    /// resolved): the object is known, nothing runs.
    instance: Option<String>,
}

/// Class reloads requested from outside the script phase (asset updates),
/// applied at the start of the next frame.
pub type ReloadRequests = Arc<Mutex<Vec<pulsar_events::AssetUpdated>>>;

/// Keeps the script runtime in step with the world. See the module doc.
pub struct ScriptDriver {
    runtime: ScriptRuntime,
    project_root: PathBuf,
    registry: ClassRegistry,
    /// Rescan `project_root` when a class is not in the registry (a class
    /// created after startup). Off for a registry given by the caller.
    rescan_registry: bool,
    config: ScriptingConfig,
    cursor: Option<ChangeCursor>,
    tracked: HashMap<Entity, Tracked>,
    by_instance: HashMap<String, Entity>,
    /// Class GUID → the runtime's class name (the module name).
    loaded: HashMap<ClassId, String>,
    globals: Vec<String>,
    globals_started: bool,
    spawn_serial: u64,
    reloads: ReloadRequests,
    scratch: Vec<ComponentChange>,
    /// Event hub integration, once attached.
    events: Option<ScriptEvents>,
    /// Level name reported by `LevelLoaded`.
    level: String,
}

impl ScriptDriver {
    /// A driver for the project at `project_root`: its classes
    /// (`src/classes/*`) and scripting config.
    pub fn new(runtime: ScriptRuntime, project_root: impl Into<PathBuf>) -> Self {
        let project_root = project_root.into();
        let registry = ClassRegistry::scan(&project_root);
        let config = ScriptingConfig::load(&project_root);
        let mut driver = Self::with_parts(runtime, project_root, registry, config);
        driver.rescan_registry = true;
        driver
    }

    /// A driver over an explicit registry and config (tools, tests).
    pub fn with_parts(
        runtime: ScriptRuntime,
        project_root: impl Into<PathBuf>,
        registry: ClassRegistry,
        config: ScriptingConfig,
    ) -> Self {
        Self {
            runtime,
            project_root: project_root.into(),
            registry,
            rescan_registry: false,
            config,
            cursor: None,
            tracked: HashMap::new(),
            by_instance: HashMap::new(),
            loaded: HashMap::new(),
            globals: Vec::new(),
            globals_started: false,
            spawn_serial: 0,
            reloads: Arc::new(Mutex::new(Vec::new())),
            scratch: Vec::new(),
            events: None,
            level: String::new(),
        }
    }

    // ---- events -------------------------------------------------------------

    /// Put the scripts on `hub` (the session's event hub): declared events
    /// register there, handlers link against it, natives publish to it and
    /// every instance, running or started later, subscribes its handlers.
    /// See [`super::events`].
    pub fn attach_events(&mut self, hub: EventHub) {
        if self.events.as_ref().is_some_and(|e| e.hub().ptr_eq(&hub)) {
            return;
        }
        if let Some(mut old) = self.events.take() {
            old.clear();
        }
        let events = ScriptEvents::new(hub);
        for entry in self.registry.entries() {
            events.bridge().add_class(&entry.name, entry.id.as_str());
        }
        let report = self.runtime.set_event_host(Some(events.host()));
        for (class, error) in report.failed {
            tracing::warn!(class = %class, "script class does not link against the event hub: {error}");
        }
        self.events = Some(events);
        self.predeclare_project_events();
        self.resubscribe(None);
    }

    /// Declare the events of every compiled class in the project on the
    /// hub, so a class can subscribe to another class's events whichever
    /// loads first.
    fn predeclare_project_events(&self) {
        if self.events.is_none() {
            return;
        }
        for entry in self.registry.entries() {
            let path = module_file(entry);
            let Some(Ok(bytes)) = read_module_bytes(&path) else { continue };
            match pulsar_script_vm::Module::decode(&bytes) {
                Ok(module) => {
                    if let Err(error) = self.runtime.declare_events(&module) {
                        tracing::warn!("{error}");
                    }
                }
                Err(error) => tracing::debug!(path = %path.display(), "unreadable script module: {error}"),
            }
        }
    }

    /// The attached event integration.
    pub fn events(&self) -> Option<&ScriptEvents> {
        self.events.as_ref()
    }

    /// The attached event hub.
    pub fn event_hub(&self) -> Option<&EventHub> {
        self.events.as_ref().map(ScriptEvents::hub)
    }

    /// Name the level `LevelLoaded` reports.
    pub fn set_level_name(&mut self, level: impl Into<String>) {
        self.level = level.into();
    }

    /// (Re)subscribe the live instances of `class` (the runtime's class
    /// name), or of every class when `None`.
    fn resubscribe(&mut self, class: Option<&str>) {
        let Some(events) = self.events.as_mut() else { return };
        let targets: Vec<(String, String, String, Option<Entity>)> = self
            .runtime
            .instance_ids()
            .iter()
            .filter_map(|id| {
                let class_name = self.runtime.class_of(id)?;
                if class.is_some_and(|c| c != class_name) {
                    return None;
                }
                let guid = self
                    .loaded
                    .iter()
                    .find(|(_, name)| name.as_str() == class_name)
                    .map(|(guid, _)| guid.as_str().to_owned())
                    .unwrap_or_default();
                Some((id.clone(), class_name.to_owned(), guid, self.by_instance.get(id).copied()))
            })
            .collect();
        for (id, class_name, guid, entity) in targets {
            // Timers and queued handler calls survive a class reload.
            for failure in events.resubscribe(&self.runtime, &id, &class_name, &guid, entity) {
                tracing::warn!("{failure}");
            }
        }
    }

    fn subscribe_instance(&mut self, id: &str, class: &str, guid: &str, entity: Option<Entity>, report: &mut DriverReport) {
        if let Some(events) = self.events.as_mut() {
            events.bridge().add_class(class, guid);
            for failure in events.subscribe(&self.runtime, id, class, guid, entity) {
                tracing::warn!("{failure}");
                report.failures.push(failure);
            }
        }
    }

    fn publish<T: pulsar_events::gamma::Event + Send>(&self, channel: pulsar_events::gamma::Channel, event: T) {
        if let Some(events) = &self.events {
            events.hub().publish(channel, event);
        }
    }

    pub fn runtime(&self) -> &ScriptRuntime {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut ScriptRuntime {
        &mut self.runtime
    }

    pub fn registry(&self) -> &ClassRegistry {
        &self.registry
    }

    pub fn config(&self) -> &ScriptingConfig {
        &self.config
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    // ---- identity -----------------------------------------------------------

    /// The id of the script instance running on `entity`, if any.
    pub fn instance_of(&self, entity: Entity) -> Option<&str> {
        self.tracked.get(&entity)?.instance.as_deref()
    }

    /// The entity instance `instance_id` is bound to.
    pub fn entity_of_instance(&self, instance_id: &str) -> Option<Entity> {
        self.by_instance.get(instance_id).copied()
    }

    /// Ids of the global script instances, in start order.
    pub fn global_instances(&self) -> &[String] {
        &self.globals
    }

    /// Every bound instance as `(entity, instance id)`, in start order.
    pub fn bound_instances(&self) -> Vec<(Entity, String)> {
        self.runtime
            .instance_ids()
            .iter()
            .filter_map(|id| Some((self.entity_of_instance(id)?, id.clone())))
            .collect()
    }

    // ---- the script phase ---------------------------------------------------

    /// Run one script phase against `world`. See the module doc.
    pub fn run_frame(&mut self, world: &mut World, delta_time: f64) -> DriverReport {
        let mut report = DriverReport::default();
        let mut scope = CommandScope::begin();
        if let Some(events) = &self.events {
            events.bridge().set_time(self.runtime.time());
        }
        self.reconcile_into(world, &mut report);
        report.script_errors.extend(self.runtime.dispatch_pending_begin_play(world));
        for id in &report.started {
            if let Some(entity) = self.by_instance.get(id) {
                self.publish(entity_channel(entity.bits()), BeginPlay { entity: entity.bits() });
            }
        }
        if let Some(events) = self.events.as_mut() {
            events.announce_level(&self.level);
            report.script_errors.extend(events.run_calls(&mut self.runtime, world));
        }
        report.script_errors.extend(self.runtime.tick_all(world, delta_time));
        if let Some(events) = &self.events {
            events.bridge().fire_timers(self.runtime.time());
        }
        for _ in 0..MAX_COMMAND_ROUNDS {
            let commands = scope.take();
            if commands.is_empty() {
                break;
            }
            self.apply_commands(world, commands, &mut report);
        }
        let left = scope.finish();
        if !left.is_empty() {
            tracing::warn!(dropped = left.len(), "Script world commands kept queueing more; dropping the rest");
            discard_commands(world, left);
        }
        report
    }

    /// Bring the script instances in line with the world's
    /// `ClassInstance`s without running any event. World commands queued by
    /// `end_play` of stopped instances are applied.
    pub fn reconcile(&mut self, world: &mut World) -> DriverReport {
        let mut report = DriverReport::default();
        let mut scope = CommandScope::begin();
        self.reconcile_into(world, &mut report);
        let commands = scope.take();
        self.apply_commands(world, commands, &mut report);
        discard_commands(world, scope.finish());
        report
    }

    /// Run `end_play` on every running instance (shutdown). World changes
    /// scripts request now are dropped.
    ///
    /// With an event hub attached, every script subscription, queued
    /// handler call and timer is dropped and the hub's queue drained, so
    /// a stopped session leaves nothing behind on the hub.
    pub fn end_play_all(&mut self, world: &mut World) -> Vec<RuntimeError> {
        let scope = CommandScope::begin();
        if let Some(events) = self.events.as_mut() {
            // No handler runs after end_play.
            events.clear();
        }
        let errors = self.runtime.end_play_all(world);
        discard_commands(world, scope.finish());
        if let Some(events) = self.events.as_mut() {
            events.clear();
        }
        errors
    }

    fn reconcile_into(&mut self, world: &mut World, report: &mut DriverReport) {
        let reloads = std::mem::take(&mut *self.reloads.lock().unwrap_or_else(|p| p.into_inner()));
        for event in reloads {
            if let Some(class) = self.reload_class_for_asset_into(world, &event, report) {
                tracing::info!(class = %class, "Reloaded script class after an asset update");
            }
        }
        if !self.globals_started {
            self.globals_started = true;
            self.start_globals(report);
        }
        let dirty = match self.cursor.as_mut() {
            None => {
                // Opened before the scan under the same world borrow: every
                // later change is in the journal, nothing is missed.
                self.cursor = Some(world.open_change_cursor::<ClassInstance>());
                None
            }
            Some(cursor) => {
                self.scratch.clear();
                match world.read_changes(cursor, &mut self.scratch) {
                    ChangeRead::Complete => {
                        let mut seen = HashSet::new();
                        Some(
                            self.scratch
                                .iter()
                                .map(|change| change.entity)
                                .filter(|entity| seen.insert(*entity))
                                .collect::<Vec<_>>(),
                        )
                    }
                    ChangeRead::Overflowed => {
                        tracing::info!("ClassInstance change journal overflowed; rescanning");
                        None
                    }
                }
            }
        };
        match dirty {
            Some(entities) => {
                for entity in entities {
                    self.sync_entity(world, entity, report);
                }
            }
            None => self.rescan(world, report),
        }
    }

    /// Stop instances whose object is gone and start or refresh every live
    /// `ClassInstance`, by depth then StableId.
    fn rescan(&mut self, world: &mut World, report: &mut DriverReport) {
        let mut roots: Vec<(usize, String, Entity)> = world
            .query::<&ClassInstance>()
            .map(|(entity, _)| entity)
            .collect::<Vec<_>>()
            .into_iter()
            .map(|entity| {
                let stable = world.stable_id_of(entity).unwrap_or_default().to_owned();
                (depth(world, entity), stable, entity)
            })
            .collect();
        sort_roots(&mut roots);
        let live: HashSet<Entity> = roots.iter().map(|root| root.2).collect();
        let mut gone: Vec<Entity> = self.tracked.keys().copied().filter(|e| !live.contains(e)).collect();
        gone.sort_by_key(|entity| entity.bits());
        for entity in gone {
            self.stop(world, entity, report);
        }
        for (_, _, entity) in roots {
            self.sync_entity(world, entity, report);
        }
    }

    fn sync_entity(&mut self, world: &mut World, entity: Entity, report: &mut DriverReport) {
        let live = world
            .is_alive(entity)
            .then(|| world.get::<ClassInstance>(entity).cloned())
            .flatten();
        let Some(instance) = live else {
            self.stop(world, entity, report);
            return;
        };
        let Some(tracked) = self.tracked.get(&entity).cloned() else {
            self.start(world, entity, &instance, report);
            return;
        };
        let entry = self.resolve(&instance);
        let same_class = match &entry {
            Some(entry) => entry.id == tracked.class,
            None => instance.class == tracked.class && instance.class_name == tracked.class_name,
        };
        if !same_class {
            self.stop(world, entity, report);
            self.start(world, entity, &instance, report);
        } else if let Some(id) = &tracked.instance {
            // Same class, rewritten: its components may have been rebuilt.
            bind_class_slots(&mut self.runtime, id, world, entity);
        } else if let Some(entry) = entry {
            // Known object without a script yet: the module may exist now.
            self.try_spawn(world, entity, &instance.variable_overrides, &entry, report);
        }
    }

    fn start(&mut self, world: &World, entity: Entity, instance: &ClassInstance, report: &mut DriverReport) {
        let Some(entry) = self.resolve(instance) else {
            let message = format!(
                "object '{}': class '{}' ({}) is not in this project; its script does not run",
                world.stable_id_of(entity).unwrap_or("?"),
                instance.class_name,
                instance.class
            );
            tracing::warn!("{message}");
            report.failures.push(message);
            self.tracked.insert(
                entity,
                Tracked { class: instance.class.clone(), class_name: instance.class_name.clone(), instance: None },
            );
            return;
        };
        self.tracked.insert(
            entity,
            Tracked { class: entry.id.clone(), class_name: entry.name.clone(), instance: None },
        );
        self.try_spawn(world, entity, &instance.variable_overrides, &entry, report);
    }

    fn try_spawn(
        &mut self,
        world: &World,
        entity: Entity,
        overrides: &BTreeMap<String, Value>,
        entry: &ClassEntry,
        report: &mut DriverReport,
    ) {
        let Some(class) = self.ensure_class_loaded(entry, report) else {
            return;
        };
        let stable = match world.stable_id_of(entity) {
            Some(stable) => stable.to_owned(),
            None => {
                tracing::warn!(class = %entry.name, "Class instance on an entity without a StableId; its instance id is not stable");
                format!("entity:{:x}", entity.bits())
            }
        };
        let id = instance_id_for(&stable, &entry.id);
        if self.by_instance.get(&id).is_some_and(|other| *other != entity) {
            let message = format!("script instance '{id}' already runs on another entity");
            tracing::warn!("{message}");
            report.failures.push(message);
            return;
        }
        let overrides: HashMap<String, Value> = overrides.clone().into_iter().collect();
        match self.runtime.spawn_with_json(id.clone(), &class, Some(entity), &overrides) {
            Ok(()) => {
                bind_class_slots(&mut self.runtime, &id, world, entity);
                if let Some(tracked) = self.tracked.get_mut(&entity) {
                    tracked.instance = Some(id.clone());
                }
                self.by_instance.insert(id.clone(), entity);
                self.subscribe_instance(&id, &class, entry.id.as_str(), Some(entity), report);
                report.started.push(id);
            }
            Err(error) => {
                let message = format!("script instance '{id}' did not start: {error}");
                tracing::warn!("{message}");
                report.failures.push(message);
            }
        }
    }

    fn stop(&mut self, world: &mut World, entity: Entity, report: &mut DriverReport) {
        let Some(tracked) = self.tracked.remove(&entity) else {
            return;
        };
        if let Some(id) = tracked.instance {
            self.by_instance.remove(&id);
            if let Some(events) = self.events.as_mut() {
                events.unsubscribe(&id);
            }
            let begun = self.runtime.has_begun(&id);
            if let Err(error) = self.runtime.despawn(&id, world) {
                tracing::warn!("{error}");
                report.script_errors.push(error);
            }
            if begun {
                self.publish(entity_channel(entity.bits()), EndPlay { entity: entity.bits() });
            }
            report.stopped.push(id);
        }
        if !world.is_alive(entity) {
            // Removed from the world by someone else (world::destroy
            // publishes its own before despawning).
            self.publish(pulsar_events::gamma::Channel::Global, EntityDestroyed { entity: entity.bits() });
        }
    }

    fn start_globals(&mut self, report: &mut DriverReport) {
        for class_ref in self.config.global_scripts.clone() {
            let Some(entry) = self.resolve_class_ref(&class_ref) else {
                let message = format!("global script class '{class_ref}' is not in this project");
                tracing::warn!("{message}");
                report.failures.push(message);
                continue;
            };
            let Some(class) = self.ensure_class_loaded(&entry, report) else {
                let message = format!("global script class '{}' has no compiled script", entry.name);
                tracing::warn!("{message}");
                report.failures.push(message);
                continue;
            };
            let id = global_instance_id(&entry.id);
            if self.globals.contains(&id) {
                continue;
            }
            match self.runtime.spawn(id.clone(), &class, None, &[]) {
                Ok(()) => {
                    self.globals.push(id.clone());
                    self.subscribe_instance(&id, &class, entry.id.as_str(), None, report);
                    report.started.push(id);
                }
                Err(error) => {
                    let message = format!("global script '{id}' did not start: {error}");
                    tracing::warn!("{message}");
                    report.failures.push(message);
                }
            }
        }
    }

    // ---- classes ------------------------------------------------------------

    fn refresh_registry(&mut self) {
        if self.rescan_registry {
            self.registry = ClassRegistry::scan(&self.project_root);
            if let Some(events) = &self.events {
                for entry in self.registry.entries() {
                    events.bridge().add_class(&entry.name, entry.id.as_str());
                }
                self.predeclare_project_events();
            }
        }
    }

    /// The class `instance` names: by GUID, then by its name hint.
    fn resolve(&mut self, instance: &ClassInstance) -> Option<ClassEntry> {
        if let Some(entry) = self.registry.resolve(instance) {
            return Some(entry.clone());
        }
        self.refresh_registry();
        self.registry.resolve(instance).cloned()
    }

    /// The class a script names: its GUID, or else its name.
    fn resolve_class_ref(&mut self, class_ref: &str) -> Option<ClassEntry> {
        let find = |registry: &ClassRegistry| {
            registry
                .by_id(&ClassId::from(class_ref))
                .or_else(|| registry.by_name(class_ref))
                .cloned()
        };
        if let Some(entry) = find(&self.registry) {
            return Some(entry);
        }
        self.refresh_registry();
        find(&self.registry)
    }

    /// The runtime's name for `entry`'s script class, loading its module
    /// on first use. `None` when the class has no compiled script.
    fn ensure_class_loaded(&mut self, entry: &ClassEntry, report: &mut DriverReport) -> Option<String> {
        if let Some(name) = self.loaded.get(&entry.id) {
            return Some(name.clone());
        }
        if self.runtime.has_class(&entry.name) {
            self.loaded.insert(entry.id.clone(), entry.name.clone());
            return Some(entry.name.clone());
        }
        let module = module_file(entry);
        let loaded = match read_module_bytes(&module) {
            None => {
                tracing::debug!(class = %entry.name, "Class has no compiled script module; nothing runs for it");
                return None;
            }
            Some(Ok(bytes)) => self.runtime.load_class_bytes(&bytes, &module).map(|(name, _)| name),
            Some(Err(source)) => Err(RuntimeError::Io { path: module.clone(), source }),
        };
        match loaded {
            Ok(name) => {
                self.loaded.insert(entry.id.clone(), name.clone());
                Some(name)
            }
            Err(error) => {
                let message = format!("script module of class '{}' did not load: {error}", entry.name);
                tracing::warn!("{message}");
                report.failures.push(message);
                report.load_errors.push(error);
                None
            }
        }
    }

    /// Asset updates to apply at the start of the next frame. Share it with
    /// whatever receives asset events; see [`subscribe_class_reloads`](Self::subscribe_class_reloads).
    pub fn reload_requests(&self) -> ReloadRequests {
        Arc::clone(&self.reloads)
    }

    /// Queue every published Blueprint class update for the next frame's
    /// reload. The subscription never touches the driver or the world, so
    /// it is safe from any thread, even during the script phase.
    pub fn subscribe_class_reloads(&self) -> pulsar_events::AssetSubscription {
        let requests = self.reload_requests();
        pulsar_events::subscribe_asset_updates(Some(pulsar_events::AssetKind::Blueprint), move |event| {
            requests.lock().unwrap_or_else(|p| p.into_inner()).push(event.clone());
        })
    }

    /// Apply a class update now: reload the class's script module if the
    /// runtime has it (instances keep their state, see
    /// [`ScriptRuntime::reload_class`]), then bind the component slots of
    /// every live instance of the class again against its current
    /// components, and start the script of instances that had none (the
    /// class just got a compiled module). Returns the class name.
    pub fn reload_class_for_asset(&mut self, world: &World, event: &pulsar_events::AssetUpdated) -> Option<String> {
        let mut report = DriverReport::default();
        self.reload_class_for_asset_into(world, event, &mut report)
    }

    fn reload_class_for_asset_into(
        &mut self,
        world: &World,
        event: &pulsar_events::AssetUpdated,
        report: &mut DriverReport,
    ) -> Option<String> {
        if event.kind != pulsar_events::AssetKind::Blueprint {
            return None;
        }
        self.refresh_registry();
        let by_id = event
            .id
            .as_deref()
            .and_then(|id| self.registry.by_id(&ClassId::from(id)))
            .cloned();
        let by_path = || {
            let name = event.path.as_ref()?.file_name()?.to_str()?;
            self.registry.by_name(name).cloned()
        };
        let entry = by_id.or_else(by_path)?;

        if let Some(class) = self.loaded.get(&entry.id).cloned() {
            let module = module_file(&entry);
            if let Some(read) = read_module_bytes(&module) {
                let loaded = read
                    .map_err(|source| RuntimeError::Io { path: module.clone(), source })
                    .and_then(|bytes| self.runtime.load_class_bytes(&bytes, &module));
                match loaded {
                    Ok((name, reloaded)) => {
                        report
                            .dropped_calls
                            .extend(reloaded.dropped.into_iter().map(|call| (name.clone(), call)));
                        if name != class {
                            // The module was renamed: follow it.
                            tracing::warn!(class = %entry.name, module = %name, "Class module name changed on reload");
                            self.loaded.insert(entry.id.clone(), name.clone());
                        }
                        // Its subscriptions may have changed.
                        self.resubscribe(Some(&name));
                    }
                    Err(error) => {
                        // The old code keeps running.
                        tracing::warn!(class = %entry.name, "Class updated but its script module did not reload: {error}");
                        report.load_errors.push(error);
                        return None;
                    }
                }
            }
        }

        let mut roots: Vec<(usize, String, Entity)> = self
            .tracked
            .iter()
            .filter(|(_, tracked)| tracked.class == entry.id)
            .map(|(entity, _)| {
                let stable = world.stable_id_of(*entity).unwrap_or_default().to_owned();
                (depth(world, *entity), stable, *entity)
            })
            .collect();
        sort_roots(&mut roots);
        for (_, _, root) in roots {
            match self.tracked.get(&root).and_then(|t| t.instance.clone()) {
                Some(id) => {
                    bind_class_slots(&mut self.runtime, &id, world, root);
                }
                None => {
                    if let Some(instance) = world.get::<ClassInstance>(root).cloned() {
                        self.try_spawn(world, root, &instance.variable_overrides, &entry, report);
                    }
                }
            }
        }
        Some(entry.name)
    }

    // ---- problems ------------------------------------------------------------

    /// The frame's errors and dropped calls as editor problems (#854,
    /// #868): class, instance, function, graph node and the class's source
    /// file, where known.
    pub fn problems(&self, report: &DriverReport) -> Vec<pulsar_events::ScriptProblem> {
        let mut problems: Vec<_> = report
            .script_errors
            .iter()
            .chain(&report.load_errors)
            .map(|error| self.problem_for(error))
            .collect();
        for (class, call) in &report.dropped_calls {
            let mut problem = pulsar_events::ScriptProblem {
                severity: pulsar_events::ProblemSeverity::Warning,
                class: Some(class.clone()),
                instance: Some(call.object_id.clone()),
                function: Some(call.function.clone()),
                message: format!("class reload dropped a waiting call: {}", call.reason),
                ..Default::default()
            };
            self.fill_class(&mut problem, None);
            problems.push(problem);
        }
        problems
    }

    /// One error as an editor problem.
    pub fn problem_for(&self, error: &RuntimeError) -> pulsar_events::ScriptProblem {
        let details = error.details();
        let mut problem = pulsar_events::ScriptProblem {
            severity: pulsar_events::ProblemSeverity::Error,
            class: details.class,
            instance: details.object_id,
            function: details.function,
            node: details.location.as_ref().map(|l| l.node.clone()).filter(|n| !n.is_empty()),
            line: details.location.as_ref().and_then(|l| l.line),
            message: details.message,
            ..Default::default()
        };
        self.fill_class(&mut problem, details.location.as_ref().map(|l| l.file.as_str()));
        problem
    }

    /// Fill the class GUID and source path from the registry.
    fn fill_class(&self, problem: &mut pulsar_events::ScriptProblem, file: Option<&str>) {
        let Some(entry) = problem.class.as_deref().and_then(|name| {
            let guid = self.loaded.iter().find(|(_, n)| n.as_str() == name).map(|(guid, _)| guid.clone());
            guid.and_then(|g| self.registry.by_id(&g)).or_else(|| self.registry.by_name(name))
        }) else {
            return;
        };
        problem.class_id = Some(entry.id.as_str().to_owned());
        problem.path = Some(match file.filter(|f| !f.is_empty()) {
            Some(file) => entry.dir.join(file),
            None => entry.dir.clone(),
        });
    }

    // ---- world commands -----------------------------------------------------

    pub(crate) fn apply_commands(&mut self, world: &mut World, commands: Vec<WorldCommand>, report: &mut DriverReport) {
        for command in commands {
            match command {
                WorldCommand::Spawn { entity, class, parent, position } => {
                    self.apply_spawn(world, entity, &class, parent, position, report)
                }
                WorldCommand::Destroy { entity } => self.apply_destroy(world, entity, report),
            }
        }
    }

    fn apply_spawn(
        &mut self,
        world: &mut World,
        entity: Entity,
        class: &str,
        parent: Option<Entity>,
        position: [f32; 3],
        report: &mut DriverReport,
    ) {
        if !world.is_alive(entity) {
            return;
        }
        let def = self.resolve_class_ref(class).and_then(|entry| match entry.load_definition() {
            Ok(def) => Some(def),
            Err(error) => {
                tracing::warn!(class = %entry.name, "Class definition unreadable: {error}");
                None
            }
        });
        let Some(def) = def else {
            let message = format!("world::spawn: class '{class}' is not in this project");
            tracing::warn!("{message}");
            report.failures.push(message);
            world.despawn(entity);
            return;
        };
        let parent = parent.filter(|p| world.is_alive(*p));
        let transform = match parent {
            Some(parent) => {
                let local = LocalTransform { position, ..LocalTransform::default() };
                let parent_tf = world.get::<Transform>(parent).copied().unwrap_or_default();
                pulsar_class::world::compose(&parent_tf, &local)
            }
            None => Transform { position, ..Transform::default() },
        };
        let stable_id = self.next_spawn_id(world, &def.name);
        let spec = SpawnObject {
            stable_id: Some(stable_id),
            name: def.name.clone(),
            parent,
            transform,
            visibility: Default::default(),
            object_type: ObjectType::Blueprint,
        };
        match pulsar_class::world::instantiate_class_into(world, &def, ClassInstance::default(), spec, entity) {
            Ok(_) => {
                self.publish(pulsar_events::gamma::Channel::Global, EntitySpawned { entity: entity.bits() });
                report.spawned.push(entity)
            }
            Err(error) => {
                let message = format!("world::spawn of '{}' failed: {error}", def.name);
                tracing::warn!("{message}");
                report.failures.push(message);
                world.despawn(entity);
            }
        }
    }

    /// A StableId for a spawned object: `<Class>_rt<n>`, `n` counting this
    /// driver's spawns, so a run spawns the same ids every time.
    fn next_spawn_id(&mut self, world: &World, class_name: &str) -> String {
        loop {
            self.spawn_serial += 1;
            let id = format!("{class_name}_rt{}", self.spawn_serial);
            if world.entity_for(&id).is_none() {
                return id;
            }
        }
    }

    fn apply_destroy(&mut self, world: &mut World, entity: Entity, report: &mut DriverReport) {
        if !world.is_alive(entity) {
            return;
        }
        // Parents before children; every script on the tree ends while its
        // components are still there.
        let mut tree = vec![entity];
        let mut next = 0;
        while next < tree.len() {
            let children = world.children_of(Some(tree[next]));
            tree.extend(children);
            next += 1;
        }
        for &node in &tree {
            self.stop(world, node, report);
        }
        for &node in &tree {
            self.publish(pulsar_events::gamma::Channel::Global, EntityDestroyed { entity: node.bits() });
        }
        world.despawn_tree(entity);
        report.destroyed.push(entity);
    }
}

/// Release what dropped commands reserved.
fn discard_commands(world: &mut World, commands: Vec<WorldCommand>) {
    for command in commands {
        if let WorldCommand::Spawn { entity, .. } = command {
            if world.is_alive(entity) && world.stable_id_of(entity).is_none() {
                world.despawn(entity);
            }
        }
    }
}

/// Scan order: hierarchy depth, then StableId (then entity bits, for
/// objects without one).
fn sort_roots(roots: &mut [(usize, String, Entity)]) {
    roots.sort_by(|a, b| (a.0, &a.1, a.2.bits()).cmp(&(b.0, &b.1, b.2.bits())));
}

/// Hierarchy depth: 0 for a root object.
fn depth(world: &World, entity: Entity) -> usize {
    let mut depth = 0;
    let mut cursor = world.parent_of(entity);
    while let Some(parent) = cursor {
        depth += 1;
        if depth > 4096 {
            break;
        }
        cursor = world.parent_of(parent);
    }
    depth
}
