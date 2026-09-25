//! Script classes bound to entities.
//!
//! A **class** is a linked [`Module`] (compiled by any scripting frontend).
//! An **instance** is one copy of a class's variables, identified by an
//! object id and optionally bound to an [`Entity`]. The [`ScriptRuntime`]
//! owns the native registry, the loaded native libraries, the classes and
//! the instances, and runs lifecycle events:
//!
//! | event        | exported function signature | when                          |
//! |--------------|-----------------------------|-------------------------------|
//! | `begin_play` | `() -> unit`                | first dispatch after spawning |
//! | `tick`       | `(float delta_time) -> unit`| every frame                   |
//! | `end_play`   | `() -> unit`                | despawn / shutdown            |
//!
//! Any other exported function is a custom event, sent with
//! [`ScriptRuntime::send_event`]. A class need not export any of them.
//!
//! Latent calls: a function that waits (the VM's `Wait`) is suspended and
//! resumed by [`ScriptRuntime::tick_all`] once enough game time has
//! passed; an instance can have several waiting at once. Despawning an
//! instance drops its waiting calls. Reloading a class keeps a waiting call
//! when the new code has the same layout for every function it is
//! suspended in (same name, parameter, return and register types, and
//! instruction count, still waiting / calling at the same pc: see
//! `pulsar_script_vm::Continuation::rebase`), and drops it with a warning
//! naming the class and function otherwise ([`ReloadReport`]).
//!
//! Errors (#854, #868): a script error carries the class and the VM trace
//! with each frame's source location from the module's debug info; a link
//! error the function (and node) that uses what failed to link.
//! [`RuntimeError::details`] breaks either down for an editor.
//!
//! Events (#924): with an [`EventHost`] attached
//! ([`ScriptRuntime::set_event_host`]), a class's declared events are
//! registered with it before the class links, handlers are linked against
//! its catalog, and natives publish through it (`event::emit`, ...). Which
//! instance subscribes to what, and when handlers run, is the host's
//! business (the engine's script driver): the runtime only exposes each
//! class's checked [`subscriptions`](ScriptRuntime::subscriptions) and
//! [`call_function`](ScriptRuntime::call_function) to run a handler.
//!
//! Hot reload: [`ScriptRuntime::reload_class`] swaps a class's code and
//! keeps each instance's variables whose name and type are unchanged;
//! loading, reloading or unloading a native library relinks every class
//! (see [`RelinkReport`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::{
    Budget, Completion, Continuation, ErrorSite, EventCatalog, EventDecl, EventSink, FuncId, Host,
    Instance, LibraryError, LibraryId, LinkError, LinkedSubscription, Module, NativeFn,
    NativeLibraries, NativeRegistry, Program, ScriptError, SourceLoc, Type, Value, Vm,
};

/// The engine's event hub, as the runtime sees it: a sink for the event
/// natives, a catalog to link handlers against, and a registry for the
/// events classes declare.
pub trait EventHost: EventSink + EventCatalog + Send + Sync {
    /// Register event `decl`, declared by class (module) `class`.
    /// Registering the same declaration again is a no-op; a different
    /// event under the same name is an error.
    fn declare(&self, class: &str, decl: &EventDecl) -> Result<(), String>;
}

pub const BEGIN_PLAY: &str = "begin_play";
pub const TICK: &str = "tick";
pub const END_PLAY: &str = "end_play";

/// Default step budget for one event on one instance.
pub const DEFAULT_BUDGET: u64 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("no script class `{0}` is loaded")]
    UnknownClass(String),
    #[error("script class `{0}` is already loaded")]
    ClassLoaded(String),
    #[error("no script instance `{0}`")]
    UnknownInstance(String),
    #[error("script instance `{0}` already exists")]
    DuplicateInstance(String),
    #[error("`{class}::{name}` must be `{expected}` to be a lifecycle event")]
    BadEntryPoint { class: String, name: &'static str, expected: &'static str },
    #[error("`{class}` has no exported function `{name}`")]
    UnknownEvent { class: String, name: String },
    #[error("variable `{name}`: {reason}")]
    BadVariable { name: String, reason: String },
    #[error("script class `{class}`: {source}{}", site_suffix(.site.as_ref()))]
    Link {
        class: String,
        source: LinkError,
        /// Where in the class the error is, when the module can tell
        /// (with debug info: the graph node).
        site: Option<ErrorSite>,
    },
    #[error("script instance `{object_id}` of `{class}`: {source}")]
    Script { object_id: String, class: String, source: ScriptError },
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error("could not read `{path}`: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("`{path}` is not a script module: {source}")]
    Parse { path: PathBuf, source: serde_json::Error },
    #[error("script class `{class}` declares event `{event}`: {reason}")]
    EventDeclaration { class: String, event: String, reason: String },
}

struct Entries {
    begin_play: Option<FuncId>,
    tick: Option<FuncId>,
    end_play: Option<FuncId>,
}

struct Class {
    program: Program,
    entries: Entries,
}

impl Class {
    fn link(module: Arc<Module>, natives: &NativeRegistry, events: Option<&dyn EventCatalog>) -> Result<Self, RuntimeError> {
        let class = module.name.clone();
        let program = Program::link_with_events(Arc::clone(&module), natives, events).map_err(|source| {
            let site = module.locate_link_error(&source);
            RuntimeError::Link { class: class.clone(), source, site }
        })?;
        let entry = |name: &'static str, params: &[Type], expected: &'static str| {
            match program.module().function(name) {
                Some((index, f)) if f.exported => {
                    if f.params == params && f.ret == Type::Unit {
                        Ok(Some(FuncId(index)))
                    } else {
                        Err(RuntimeError::BadEntryPoint { class: class.clone(), name, expected })
                    }
                }
                _ => Ok(None),
            }
        };
        let entries = Entries {
            begin_play: entry(BEGIN_PLAY, &[], "fn begin_play()")?,
            tick: entry(TICK, &[Type::Float], "fn tick(delta_time: float)")?,
            end_play: entry(END_PLAY, &[], "fn end_play()")?,
        };
        Ok(Self { program, entries })
    }
}

/// One live instance of a class.
struct ScriptInstance {
    class: String,
    state: Instance,
    entity: Option<Entity>,
    /// Suspended calls and the game time each resumes at.
    waiting: Vec<(f64, Continuation)>,
}

/// What a class reload did with the instances' waiting calls.
#[derive(Debug, Default)]
pub struct ReloadReport {
    /// Waiting calls that continue in the new code.
    pub kept: usize,
    /// Waiting calls dropped because their code changed shape.
    pub dropped: Vec<DroppedCall>,
}

/// A waiting call a reload could not keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedCall {
    pub object_id: String,
    /// The suspended (innermost) function.
    pub function: String,
    pub reason: String,
}

/// Classes that failed to relink after the native registry changed. They
/// keep running the code they were last linked with (whose natives stay
/// loaded) until fixed and reloaded.
#[derive(Debug, Default)]
pub struct RelinkReport {
    pub relinked: Vec<String>,
    pub failed: Vec<(String, LinkError)>,
}

pub struct ScriptRuntime {
    natives: NativeRegistry,
    libraries: NativeLibraries,
    vm: Vm,
    classes: HashMap<String, Class>,
    instances: HashMap<String, ScriptInstance>,
    /// Instance ids in spawn order: events dispatch in this order.
    order: Vec<String>,
    pending_begin_play: Vec<String>,
    /// Game time in seconds: the sum of every `tick_all` delta.
    time: f64,
    /// Step budget for one event on one instance.
    pub budget: u64,
    events: Option<Arc<dyn EventHost>>,
}

impl ScriptRuntime {
    /// A runtime with the engine's natives. Native library shadow copies go
    /// in `shadow_dir`.
    pub fn new(shadow_dir: impl Into<PathBuf>) -> Self {
        Self::with_natives(NativeRegistry::with_engine_natives(), shadow_dir)
    }

    pub fn with_natives(natives: NativeRegistry, shadow_dir: impl Into<PathBuf>) -> Self {
        Self {
            natives,
            libraries: NativeLibraries::new(shadow_dir),
            vm: Vm::new(),
            classes: HashMap::new(),
            instances: HashMap::new(),
            order: Vec::new(),
            pending_begin_play: Vec::new(),
            time: 0.0,
            budget: DEFAULT_BUDGET,
            events: None,
        }
    }

    // ---- events --------------------------------------------------------

    /// Attach (or detach) the engine's event hub. Every loaded class's
    /// events are declared on it and every class relinks against its
    /// catalog.
    pub fn set_event_host(&mut self, events: Option<Arc<dyn EventHost>>) -> RelinkReport {
        self.events = events;
        if let Some(events) = &self.events {
            for class in self.classes.values() {
                if let Err(error) = declare_events(events.as_ref(), class.program.module()) {
                    tracing::error!("{error}");
                }
            }
        }
        self.relink_all()
    }

    pub fn event_host(&self) -> Option<&Arc<dyn EventHost>> {
        self.events.as_ref()
    }

    fn catalog(&self) -> Option<&dyn EventCatalog> {
        self.events.as_deref().map(|e| e as &dyn EventCatalog)
    }

    /// Declare `module`'s events on the event host now (before loading it),
    /// so classes loaded earlier can subscribe to them. No-op without a
    /// host.
    pub fn declare_events(&self, module: &Module) -> Result<(), RuntimeError> {
        match &self.events {
            Some(events) => declare_events(events.as_ref(), module),
            None => Ok(()),
        }
    }

    /// The checked event subscriptions of a loaded class.
    pub fn subscriptions(&self, class: &str) -> Option<&[LinkedSubscription]> {
        Some(self.classes.get(class)?.program.subscriptions())
    }

    /// Game time in seconds (advanced by [`tick_all`](Self::tick_all)).
    pub fn time(&self) -> f64 {
        self.time
    }

    /// Number of suspended calls on an instance.
    pub fn waiting_calls(&self, object_id: &str) -> usize {
        self.instances.get(object_id).map_or(0, |i| i.waiting.len())
    }

    /// The natives scripts can link against (for frontends' palettes).
    pub fn natives(&self) -> &NativeRegistry {
        &self.natives
    }

    // ---- natives -------------------------------------------------------

    /// Register an engine native, then relink every class.
    pub fn register_native(&mut self, native: NativeFn) -> Result<RelinkReport, RuntimeError> {
        self.natives
            .register(native)
            .map_err(|e| RuntimeError::Library(LibraryError::Duplicate(e)))?;
        Ok(self.relink_all())
    }

    pub fn load_library(&mut self, path: impl AsRef<Path>) -> Result<(LibraryId, RelinkReport), RuntimeError> {
        let id = self.libraries.load(path, &mut self.natives)?;
        Ok((id, self.relink_all()))
    }

    pub fn reload_library(&mut self, id: LibraryId) -> Result<RelinkReport, RuntimeError> {
        self.libraries.reload(id, &mut self.natives)?;
        Ok(self.relink_all())
    }

    pub fn unload_library(&mut self, id: LibraryId) -> Result<RelinkReport, RuntimeError> {
        self.libraries.unload(id, &mut self.natives)?;
        Ok(self.relink_all())
    }

    fn relink_all(&mut self) -> RelinkReport {
        let mut report = RelinkReport::default();
        let catalog = self.events.as_deref().map(|e| e as &dyn EventCatalog);
        for (name, class) in &mut self.classes {
            match Class::link(Arc::clone(class.program.module()), &self.natives, catalog) {
                Ok(relinked) => {
                    *class = relinked;
                    report.relinked.push(name.clone());
                }
                Err(RuntimeError::Link { source, .. }) => {
                    tracing::error!("script class `{name}` failed to relink: {source}");
                    report.failed.push((name.clone(), source));
                }
                Err(other) => tracing::error!("script class `{name}` failed to relink: {other}"),
            }
        }
        report
    }

    // ---- classes -------------------------------------------------------

    /// Link and add a class, named by its module name.
    pub fn load_class(&mut self, module: Module) -> Result<(), RuntimeError> {
        if self.classes.contains_key(&module.name) {
            return Err(RuntimeError::ClassLoaded(module.name));
        }
        self.declare_events(&module)?;
        let class = Class::link(Arc::new(module), &self.natives, self.catalog())?;
        self.classes.insert(class.program.module().name.clone(), class);
        Ok(())
    }

    /// Read a JSON module and load it (or reload it, if already loaded).
    pub fn load_class_file(&mut self, path: impl AsRef<Path>) -> Result<String, RuntimeError> {
        self.load_class_file_reporting(path).map(|(name, _)| name)
    }

    /// [`load_class_file`](Self::load_class_file), also returning what a
    /// reload did with waiting calls (empty for a first load).
    pub fn load_class_file_reporting(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<(String, ReloadReport), RuntimeError> {
        let path = path.as_ref();
        let json = std::fs::read_to_string(path)
            .map_err(|source| RuntimeError::Io { path: path.to_owned(), source })?;
        let module = Module::from_json(&json)
            .map_err(|source| RuntimeError::Parse { path: path.to_owned(), source })?;
        let name = module.name.clone();
        let report = if self.classes.contains_key(&name) {
            self.reload_class(module)?
        } else {
            self.load_class(module)?;
            ReloadReport::default()
        };
        Ok((name, report))
    }

    pub fn has_class(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// The variables a loaded class declares, as `(name, type)`, in slot
    /// order. Hosts use it to find variables they fill at bind time (e.g.
    /// the hidden `__slot:<uuid>` component handles of class instances).
    pub fn class_variables(&self, class: &str) -> Option<Vec<(String, Type)>> {
        let module = self.classes.get(class)?.program.module();
        Some(module.variables.iter().map(|v| (v.name.clone(), v.ty.clone())).collect())
    }

    /// The class an instance runs.
    pub fn class_of(&self, object_id: &str) -> Option<&str> {
        self.instances.get(object_id).map(|i| i.class.as_str())
    }

    /// Swap a loaded class's code. Instances keep their identity, binding
    /// and every variable whose name and type are unchanged; new or
    /// retyped variables start at their defaults. Waiting calls continue
    /// in the new code when their functions' layout is unchanged (see
    /// `Continuation::rebase`), and are dropped otherwise, listed in the
    /// report. On error nothing changes.
    pub fn reload_class(&mut self, module: Module) -> Result<ReloadReport, RuntimeError> {
        let name = module.name.clone();
        if !self.classes.contains_key(&name) {
            return Err(RuntimeError::UnknownClass(name));
        }
        self.declare_events(&module)?;
        let new = Class::link(Arc::new(module), &self.natives, self.catalog())?;
        let old = &self.classes[&name];

        let old_module = Arc::clone(old.program.module());
        let mut migrated = 0;
        let mut kept = 0;
        let mut report = ReloadReport::default();
        for (object_id, instance) in self.instances.iter_mut().filter(|(_, i)| i.class == name) {
            let mut state = new.program.instantiate();
            for (index, var) in new.program.module().variables.iter().enumerate() {
                let Some(old_index) = old_module.variables.iter().position(|v| v.name == var.name && v.ty == var.ty)
                else {
                    continue;
                };
                if let Some(value) = old.program.var(&instance.state, old_index) {
                    // Same name and type: always fits.
                    let _ = new.program.set_var(&mut state, index, value.clone());
                }
            }
            instance.state = state;
            // Suspended calls ran the old code: they continue in the new
            // code where its layout is compatible (#862), see
            // `Continuation::rebase`; the others are dropped.
            for (wake, continuation) in std::mem::take(&mut instance.waiting) {
                match continuation.rebase(new.program.module()) {
                    Ok(rebased) => {
                        instance.waiting.push((wake, rebased));
                        kept += 1;
                    }
                    Err(reason) => {
                        let function = continuation.functions().last().map(|f| f.to_string()).unwrap_or_default();
                        tracing::warn!(
                            class = %name,
                            function = %function,
                            "reload dropped a waiting script call of `{name}::{function}`: {reason}"
                        );
                        report.dropped.push(DroppedCall { object_id: object_id.clone(), function, reason });
                    }
                }
            }
            migrated += 1;
        }
        tracing::info!(class = %name, instances = migrated, kept_waiting = kept, "reloaded script class");
        report.kept = kept;
        self.classes.insert(name, new);
        Ok(report)
    }

    // ---- instances -----------------------------------------------------

    /// Create an instance of `class` with id `object_id`, optionally bound
    /// to `entity`, with variable overrides applied. Its `begin_play` runs
    /// at the next [`dispatch_pending_begin_play`](Self::dispatch_pending_begin_play).
    pub fn spawn(
        &mut self,
        object_id: impl Into<String>,
        class: &str,
        entity: Option<Entity>,
        overrides: &[(String, Value)],
    ) -> Result<(), RuntimeError> {
        let object_id = object_id.into();
        if self.instances.contains_key(&object_id) {
            return Err(RuntimeError::DuplicateInstance(object_id));
        }
        let program = &self.classes.get(class).ok_or_else(|| RuntimeError::UnknownClass(class.to_owned()))?.program;
        let mut state = program.instantiate();
        for (name, value) in overrides {
            let index = program.variable(name).ok_or_else(|| RuntimeError::BadVariable {
                name: name.clone(),
                reason: format!("`{class}` has no such variable"),
            })?;
            program
                .set_var(&mut state, index, value.clone())
                .map_err(|reason| RuntimeError::BadVariable { name: name.clone(), reason })?;
        }
        self.instances.insert(
            object_id.clone(),
            ScriptInstance { class: class.to_owned(), state, entity, waiting: Vec::new() },
        );
        self.order.push(object_id.clone());
        self.pending_begin_play.push(object_id);
        Ok(())
    }

    /// Like [`spawn`](Self::spawn), with overrides as JSON (level files).
    /// Overrides naming variables the class no longer has are ignored with
    /// a warning; a value of the wrong type is an error.
    pub fn spawn_with_json(
        &mut self,
        object_id: impl Into<String>,
        class: &str,
        entity: Option<Entity>,
        overrides: &HashMap<String, serde_json::Value>,
    ) -> Result<(), RuntimeError> {
        let program = &self.classes.get(class).ok_or_else(|| RuntimeError::UnknownClass(class.to_owned()))?.program;
        let mut converted = Vec::with_capacity(overrides.len());
        for (name, json) in overrides {
            // Level files outlive graph edits: a variable that no longer
            // exists is skipped, not fatal.
            let Some(var) = program.variable(name).map(|i| &program.module().variables[i]) else {
                tracing::warn!("`{class}` has no variable `{name}`; ignoring its override");
                continue;
            };
            let value = value_from_json(json, &var.ty)
                .map_err(|reason| RuntimeError::BadVariable { name: name.clone(), reason })?;
            converted.push((name.clone(), value));
        }
        self.spawn(object_id, class, entity, &converted)
    }

    /// Run `end_play` (if the instance has begun) and remove the instance.
    pub fn despawn(&mut self, object_id: &str, world: &mut World) -> Result<(), RuntimeError> {
        if !self.instances.contains_key(object_id) {
            return Err(RuntimeError::UnknownInstance(object_id.to_owned()));
        }
        let begun = !self.pending_begin_play.iter().any(|id| id == object_id);
        let result = if begun { self.run_lifecycle(object_id, END_PLAY, world) } else { Ok(()) };
        self.instances.remove(object_id);
        self.order.retain(|id| id != object_id);
        self.pending_begin_play.retain(|id| id != object_id);
        result
    }

    pub fn bind(&mut self, object_id: &str, entity: Entity) -> Result<Option<Entity>, RuntimeError> {
        let instance = self.instance_mut(object_id)?;
        Ok(instance.entity.replace(entity))
    }

    pub fn unbind(&mut self, object_id: &str) -> Result<Option<Entity>, RuntimeError> {
        Ok(self.instance_mut(object_id)?.entity.take())
    }

    pub fn entity_of(&self, object_id: &str) -> Option<Entity> {
        self.instances.get(object_id)?.entity
    }

    /// Instance ids in dispatch order.
    pub fn instance_ids(&self) -> &[String] {
        &self.order
    }

    /// Read an instance variable.
    pub fn variable(&self, object_id: &str, name: &str) -> Option<&Value> {
        let instance = self.instances.get(object_id)?;
        let program = &self.classes.get(&instance.class)?.program;
        program.var(&instance.state, program.variable(name)?)
    }

    pub fn set_variable(&mut self, object_id: &str, name: &str, value: Value) -> Result<(), RuntimeError> {
        let instance = self.instances.get_mut(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let program = &self.classes.get(&instance.class).ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?.program;
        let bad = |reason: String| RuntimeError::BadVariable { name: name.to_owned(), reason };
        let index = program.variable(name).ok_or_else(|| bad("no such variable".into()))?;
        program.set_var(&mut instance.state, index, value).map_err(bad)
    }

    fn instance_mut(&mut self, object_id: &str) -> Result<&mut ScriptInstance, RuntimeError> {
        self.instances.get_mut(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))
    }

    // ---- events --------------------------------------------------------

    /// Run `begin_play` for every instance spawned since the last call.
    /// Failures are logged per instance and returned.
    pub fn dispatch_pending_begin_play(&mut self, world: &mut World) -> Vec<RuntimeError> {
        let pending = std::mem::take(&mut self.pending_begin_play);
        pending
            .iter()
            .filter_map(|id| self.run_lifecycle(id, BEGIN_PLAY, world).err())
            .inspect(|err| tracing::warn!("{err}"))
            .collect()
    }

    /// Advance game time by `delta_time`, resume every waiting call that is
    /// due, then run `tick(delta_time)` on every instance that has begun,
    /// in spawn order. A failing instance does not stop the others.
    pub fn tick_all(&mut self, world: &mut World, delta_time: f64) -> Vec<RuntimeError> {
        self.time += delta_time;
        let ids: Vec<String> = self
            .order
            .iter()
            .filter(|id| !self.pending_begin_play.contains(id))
            .cloned()
            .collect();
        let mut errors: Vec<RuntimeError> =
            ids.iter().flat_map(|id| self.resume_due(id, world)).collect();
        errors.extend(ids.iter().filter_map(|id| {
                let func = self.instances.get(id).and_then(|i| self.classes.get(&i.class)).and_then(|c| c.entries.tick)?;
                self.call(id, func, &[Value::Float(delta_time)], world).err()
            }));
        for err in &errors {
            tracing::warn!("{err}");
        }
        errors
    }

    /// Resume an instance's calls that are due at the current time. Calls
    /// that wait again (even for zero seconds) resume on a later tick.
    fn resume_due(&mut self, object_id: &str, world: &mut World) -> Vec<RuntimeError> {
        let Some(instance) = self.instances.get_mut(object_id) else { return Vec::new() };
        let now = self.time;
        let (mut due, later): (Vec<_>, Vec<_>) =
            std::mem::take(&mut instance.waiting).into_iter().partition(|(wake, _)| *wake <= now);
        instance.waiting = later;
        due.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut errors = Vec::new();
        for (_, continuation) in due {
            let Some(instance) = self.instances.get_mut(object_id) else { break };
            let Some(class) = self.classes.get(&instance.class) else { break };
            let mut host = Host::at_time(world, instance.entity.unwrap_or(Entity::DANGLING), now)
                .with_events(self.events.as_deref().map(|e| e as &dyn EventSink));
            let mut budget = Budget::new(self.budget);
            match self.vm.resume(&class.program, &mut instance.state, continuation, &mut host, &mut budget) {
                Ok(Completion::Returned(_)) => {}
                Ok(Completion::Waiting { seconds, continuation }) => {
                    instance.waiting.push((now + seconds, continuation));
                }
                Err(source) => errors.push(RuntimeError::Script {
                    object_id: object_id.to_owned(),
                    class: instance.class.clone(),
                    source,
                }),
            }
        }
        errors
    }

    /// Run `end_play` on every instance that has begun (shutdown).
    pub fn end_play_all(&mut self, world: &mut World) -> Vec<RuntimeError> {
        let ids: Vec<String> = self
            .order
            .iter()
            .filter(|id| !self.pending_begin_play.contains(id))
            .cloned()
            .collect();
        ids.iter()
            .filter_map(|id| self.run_lifecycle(id, END_PLAY, world).err())
            .inspect(|err| tracing::warn!("{err}"))
            .collect()
    }

    /// Call exported function `name` on an instance (a custom event).
    pub fn send_event(
        &mut self,
        object_id: &str,
        name: &str,
        args: &[Value],
        world: &mut World,
    ) -> Result<Value, RuntimeError> {
        let instance = self.instances.get(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let class = self.classes.get(&instance.class).ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?;
        let func = class.program.entry(name).ok_or_else(|| RuntimeError::UnknownEvent {
            class: instance.class.clone(),
            name: name.to_owned(),
        })?;
        self.call(object_id, func, args, world)
    }

    /// Run function `func` (any function of the instance's class, exported
    /// or not: event handlers) on an instance, like an event. A function
    /// that waits finishes on a later tick; the caller gets unit now.
    pub fn call_function(
        &mut self,
        object_id: &str,
        func: FuncId,
        args: &[Value],
        world: &mut World,
    ) -> Result<Value, RuntimeError> {
        self.call(object_id, func, args, world)
    }

    /// Whether `object_id` has run `begin_play` (or has none pending).
    pub fn has_begun(&self, object_id: &str) -> bool {
        self.instances.contains_key(object_id) && !self.pending_begin_play.iter().any(|id| id == object_id)
    }

    fn run_lifecycle(&mut self, object_id: &str, event: &str, world: &mut World) -> Result<(), RuntimeError> {
        let instance = self.instances.get(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let Some(class) = self.classes.get(&instance.class) else { return Ok(()) };
        let func = match event {
            BEGIN_PLAY => class.entries.begin_play,
            END_PLAY => class.entries.end_play,
            _ => class.entries.tick,
        };
        match func {
            Some(func) => self.call(object_id, func, &[], world).map(drop),
            None => Ok(()),
        }
    }

    fn call(&mut self, object_id: &str, func: FuncId, args: &[Value], world: &mut World) -> Result<Value, RuntimeError> {
        let instance = self.instances.get_mut(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let class = self.classes.get(&instance.class).ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?;
        // Unbound instances run with a dangling entity: every component
        // access fails its liveness check instead of reaching anything.
        let mut host = Host::at_time(world, instance.entity.unwrap_or(Entity::DANGLING), self.time)
            .with_events(self.events.as_deref().map(|e| e as &dyn EventSink));
        let mut budget = Budget::new(self.budget);
        match self.vm.start(&class.program, &mut instance.state, func, args, &mut host, &mut budget) {
            Ok(Completion::Returned(value)) => Ok(value),
            // A latent event: it finishes on a later tick, so the caller
            // gets unit now.
            Ok(Completion::Waiting { seconds, continuation }) => {
                instance.waiting.push((self.time + seconds, continuation));
                Ok(Value::Unit)
            }
            Err(source) => Err(RuntimeError::Script {
                object_id: object_id.to_owned(),
                class: instance.class.clone(),
                source,
            }),
        }
    }
}

fn site_suffix(site: Option<&ErrorSite>) -> String {
    site.map(|site| format!(" (at {site})")).unwrap_or_default()
}

/// A script error broken down for an editor's problems list (#854, #868):
/// which class, instance and function, and the source location debug info
/// gives (the Blueprint node).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorDetails {
    pub class: Option<String>,
    pub object_id: Option<String>,
    pub function: Option<String>,
    pub pc: Option<usize>,
    pub location: Option<SourceLoc>,
    /// The error itself, without the location.
    pub message: String,
}

impl RuntimeError {
    /// This error's class, instance, function and source location, where
    /// it has them.
    pub fn details(&self) -> ErrorDetails {
        match self {
            Self::Script { object_id, class, source } => ErrorDetails {
                class: Some(class.clone()),
                object_id: Some(object_id.clone()),
                function: source.function().map(|(f, _)| f.to_owned()),
                pc: source.function().map(|(_, pc)| pc),
                location: source.location().cloned(),
                message: source.kind.to_string(),
            },
            Self::Link { class, source, site } => ErrorDetails {
                class: Some(class.clone()),
                object_id: None,
                function: site.as_ref().map(|s| s.function.clone()),
                pc: site.as_ref().and_then(|s| s.pc),
                location: site.as_ref().and_then(|s| s.location.clone()),
                message: source.to_string(),
            },
            Self::UnknownClass(class) | Self::ClassLoaded(class) => {
                ErrorDetails { class: Some(class.clone()), message: self.to_string(), ..Default::default() }
            }
            Self::BadEntryPoint { class, name, .. } => ErrorDetails {
                class: Some(class.clone()),
                function: Some((*name).to_owned()),
                message: self.to_string(),
                ..Default::default()
            },
            Self::UnknownEvent { class, .. } | Self::EventDeclaration { class, .. } => {
                ErrorDetails { class: Some(class.clone()), message: self.to_string(), ..Default::default() }
            }
            Self::UnknownInstance(id) | Self::DuplicateInstance(id) => {
                ErrorDetails { object_id: Some(id.clone()), message: self.to_string(), ..Default::default() }
            }
            _ => ErrorDetails { message: self.to_string(), ..Default::default() },
        }
    }
}

fn declare_events(events: &dyn EventHost, module: &Module) -> Result<(), RuntimeError> {
    for decl in &module.events {
        events.declare(&module.name, decl).map_err(|reason| RuntimeError::EventDeclaration {
            class: module.name.clone(),
            event: decl.name.clone(),
            reason,
        })?;
    }
    Ok(())
}

/// Convert a JSON value (from a level file) to a script value of type `ty`.
pub fn value_from_json(json: &serde_json::Value, ty: &Type) -> Result<Value, String> {
    use serde_json::Value as J;
    match (ty, json) {
        (Type::Bool, J::Bool(b)) => Ok(Value::Bool(*b)),
        (Type::Int, J::Number(n)) => n.as_i64().map(Value::Int).ok_or_else(|| format!("{n} is not an integer")),
        (Type::Float, J::Number(n)) => n.as_f64().map(Value::Float).ok_or_else(|| format!("{n} is not a number")),
        (Type::Str, J::String(s)) => Ok(Value::Str(s.as_str().into())),
        (ty, json) => Err(format!("cannot use {json} as {ty}")),
    }
}
