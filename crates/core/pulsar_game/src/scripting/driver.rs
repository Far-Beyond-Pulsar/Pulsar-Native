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

mod classes;
mod config;
mod event_hub;
mod frame;
mod native_libs;
mod problems;
mod reconcile;
mod report;
mod roles;
mod world_commands;

pub use config::{
    global_instance_id, instance_id_for, module_file, scripting_config_path, ScriptingConfig,
    MODULE_BINARY_FILE, MODULE_JSON_FILE, SCRIPTING_CONFIG_FILE,
};
pub use report::{DriverReport, LockTimes};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pulsar_class::{ClassId, ClassRegistry};
use pulsar_scenedb::{ChangeCursor, ComponentChange, Entity, World};
use pulsar_script_runtime::{InstanceRuntimeStats, ScriptRuntime};
use pulsar_script_vm::{DebugCommand, DebugSnapshot, LibraryId};

use super::events::ScriptEvents;
use super::traits::RuntimeTraits;
use pulsar_script_runtime::RuntimeError;
use roles::{Possessed, Possession, RoleHook, SessionRole};

/// Rounds of queued world commands applied per frame: `end_play` of a
/// destroyed object may queue more, but a chain never runs forever.
const MAX_COMMAND_ROUNDS: usize = 16;

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
    /// `ComponentOwner` changes: an instance enabled or disabled. Opened
    /// with `cursor`.
    owner_cursor: Option<ChangeCursor>,
    tracked: HashMap<Entity, Tracked>,
    /// Each tracked object's `ClassInstance` component instance -> the
    /// object, so a change on an instance already despawned still names
    /// its object (Pulsar-Native#1035).
    holders: HashMap<Entity, Entity>,
    by_instance: HashMap<String, Entity>,
    /// Class GUID → the runtime's class name (the module name).
    loaded: HashMap<ClassId, String>,
    /// Class GUID → its template (slot defaults decoded once), for
    /// `world::spawn`. Dropped on a class reload or registry rescan.
    templates: HashMap<ClassId, Arc<pulsar_class::ClassTemplate>>,
    globals: Vec<String>,
    globals_started: bool,
    /// The Blueprints the level assigned to the engine roles; started with
    /// the globals (see `start_roles`).
    world_roles: pulsar_traits::WorldRoles,
    roles_started: bool,
    /// `GameBegan` is announced after the start hooks queued with the roles.
    game_began_pending: bool,
    /// Trait hooks of role entities, run after `begin_play`.
    role_hooks: Vec<RoleHook>,
    /// The entities this session spawned for the roles; despawned when it
    /// ends. Tracked, not inferred (see `start_roles`).
    role_entities: Vec<SessionRole>,
    /// The local player's character, spawned but not yet running.
    pending_possession: Option<Possession>,
    /// The pawn the local controller possesses, once possession completed.
    possessed: Option<Possessed>,
    /// Set while `possessed`; read by the hosts' free cameras.
    possession_gate: crate::possession::PossessionGate,
    /// The entity each player's view follows; shared with the hosts.
    view_targets: crate::view_target::ViewTargets,
    /// Seconds the running frame advances by, for `on_input`.
    frame_delta: f64,
    /// What classes implement; see [`super::traits`].
    traits: RuntimeTraits,
    /// Contract violations already reported, so a hook that runs every frame
    /// says it once.
    noted: HashSet<String>,
    spawn_serial: u64,
    reloads: ReloadRequests,
    scratch: Vec<ComponentChange>,
    /// Event hub integration, once attached.
    events: Option<ScriptEvents>,
    /// Level name reported by `LevelLoaded`.
    level: String,
    /// Native project libraries found below `native/target/{debug,release}`.
    /// The source path remains stable while Cargo replaces the build artifact;
    /// ScriptRuntime maps a shadow copy on each reload.
    native_libraries: HashMap<PathBuf, (LibraryId, PathBuf, std::time::SystemTime)>,
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
        driver.load_project_native_libraries();
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
            owner_cursor: None,
            tracked: HashMap::new(),
            holders: HashMap::new(),
            by_instance: HashMap::new(),
            loaded: HashMap::new(),
            templates: HashMap::new(),
            globals: Vec::new(),
            globals_started: false,
            world_roles: pulsar_traits::WorldRoles::default(),
            roles_started: false,
            game_began_pending: false,
            role_hooks: Vec::new(),
            role_entities: Vec::new(),
            pending_possession: None,
            possessed: None,
            possession_gate: crate::possession::PossessionGate::new(),
            view_targets: crate::view_target::ViewTargets::new(),
            frame_delta: 0.0,
            traits: RuntimeTraits::default(),
            noted: HashSet::new(),
            spawn_serial: 0,
            reloads: Arc::new(Mutex::new(Vec::new())),
            scratch: Vec::new(),
            events: None,
            level: String::new(),
            native_libraries: HashMap::new(),
        }
    }

    pub fn runtime(&self) -> &ScriptRuntime {
        &self.runtime
    }

    /// The entity held by exported class variable `name` of the script instance
    /// running on `owner`: an entity, or the owner of a component reference. `None`
    /// when the object has no running script, no such variable or it is unset.
    /// Animation notifies aimed at a class variable resolve through this.
    pub fn entity_variable(&self, owner: Entity, name: &str) -> Option<Entity> {
        let instance = self.tracked.get(&owner)?.instance.as_deref()?;
        match self.runtime.variable(instance, name)? {
            pulsar_script_vm::Value::Entity(entity) => Some(*entity),
            pulsar_script_vm::Value::Component(reference) => Some(reference.entity),
            _ => None,
        }
    }

    pub fn runtime_mut(&mut self) -> &mut ScriptRuntime {
        &mut self.runtime
    }

    /// Per-instance lifetime instruction counters for the live script runtime.
    pub fn runtime_stats(&self) -> Vec<InstanceRuntimeStats> {
        self.runtime.instance_stats()
    }

    /// Set a bytecode breakpoint on one live script instance.
    pub fn set_breakpoint(
        &mut self,
        instance: &str,
        function: impl Into<String>,
        pc: usize,
    ) -> Result<(), RuntimeError> {
        self.runtime.set_breakpoint(instance, function, pc)
    }

    /// Set breakpoints on all instructions emitted from a graph node.
    pub fn set_node_breakpoint(
        &mut self,
        instance: &str,
        function: &str,
        file: &str,
        node: &str,
    ) -> Result<usize, RuntimeError> {
        self.runtime
            .set_node_breakpoint(instance, function, file, node)
    }

    /// Remove one bytecode breakpoint from a live script instance.
    pub fn remove_breakpoint(
        &mut self,
        instance: &str,
        function: &str,
        pc: usize,
    ) -> Result<bool, RuntimeError> {
        self.runtime.remove_breakpoint(instance, function, pc)
    }

    /// Snapshot paused script calls, by instance id and in arrival order.
    pub fn take_debug_events(&mut self) -> Vec<(String, DebugSnapshot)> {
        self.runtime.take_debug_events()
    }

    /// The stop snapshot for one paused call on `instance`.
    pub fn debug_snapshot(&self, instance: &str, index: usize) -> Option<&DebugSnapshot> {
        self.runtime.debug_snapshot(instance, index)
    }

    /// Resume one paused call with a debugger command.
    pub fn resume_paused(
        &mut self,
        instance: &str,
        index: usize,
        command: DebugCommand,
        world: &mut World,
    ) -> Result<(), RuntimeError> {
        self.runtime.resume_paused(instance, index, command, world)
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
}
