//! The script phase in two stages, so most of it needs no exclusive lock.
//!
//! A class whose imports are all read-access natives
//! ([`Program::access`](pulsar_script_vm::Program::access)) cannot change
//! the world, and each instance only changes its own variables, so such
//! instances can run at the same time, against a shared `&World`. That is
//! the *read stage*: the host takes a read lock (the renderer's snapshot
//! readers share it) and the instances run on as many threads as pay for
//! themselves. Every other instance runs in the *write stage*, sequentially,
//! under the exclusive lock, exactly as before.
//!
//! [`tick_all`](ScriptRuntime::tick_all) runs both. A host that owns the
//! lock calls [`begin_tick`](ScriptRuntime::begin_tick), then
//! [`run_read_stage`](ScriptRuntime::run_read_stage) holding only a read
//! lock, then [`run_write_stage`](ScriptRuntime::run_write_stage) holding
//! the write lock.
//!
//! What the split changes, on purpose:
//!
//! - **Order.** Read-only instances tick before the others in a frame
//!   instead of interleaved in spawn order. They cannot observe each other
//!   (they cannot write), so only their order relative to writers moves:
//!   they see the world as the frame's script phase found it.
//! - **Debugging.** An instance with breakpoints, or stopped at one, runs in
//!   the write stage, sequentially, where the debugger is wired.
//! - **Events.** Read-only classes cannot publish events (those natives
//!   write); their event handlers still run, in the write stage, as part of
//!   event delivery.
//!
//! Within an instance nothing changes: timers, then calls whose wait is
//! over, then `tick` (see [`crate::latent`]).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::{Access, Budget, Completion, Continuation, Host, Type, Value, Vm, Wake};

use crate::latent::park;
use crate::{Class, RuntimeError, ScriptInstance, ScriptRuntime};

/// Fewer read-only instances than this run on the calling thread: spawning
/// workers costs more than it saves.
pub const DEFAULT_PARALLEL_THRESHOLD: usize = 32;

/// What the last script phase did, for finding out whether the phase is a
/// problem before optimizing it further.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseStats {
    /// Instances run in the read stage (under a shared lock).
    pub read_instances: usize,
    /// Instances run in the write stage (under the exclusive lock).
    pub write_instances: usize,
    /// Threads the read stage used (1: the calling thread).
    pub read_threads: usize,
    /// Wall time of the read stage.
    pub read_time: Duration,
    /// Wall time of the write stage: how long the exclusive lock is needed
    /// for instances (the host adds whatever else it does under it).
    pub write_time: Duration,
}

/// Everything the read stage shares between its threads: only reads.
struct ReadContext<'a> {
    classes: &'a HashMap<String, Class>,
    class_budgets: &'a HashMap<String, u64>,
    default_budget: u64,
    time: f64,
    world: &'a World,
    max_depth: usize,
    checked_arithmetic: bool,
}

impl ScriptRuntime {
    /// Instances that run in the read stage, in spawn order: begun, of a
    /// read-access class, and not being debugged.
    fn read_stage_ids(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|id| !self.pending_begin_play.contains(id))
            .filter(|id| {
                let Some(instance) = self.instances.get(*id) else {
                    return false;
                };
                instance.paused.is_empty()
                    && instance.debugger.breakpoints().next().is_none()
                    && self
                        .classes
                        .get(&instance.class)
                        .is_some_and(|c| c.program.access() == Access::Read)
            })
            .cloned()
            .collect()
    }

    /// Start a tick: advance game time by `delta_time`. Call once per tick,
    /// before the two stages ([`tick_all`](Self::tick_all) does).
    pub fn begin_tick(&mut self, delta_time: f64) {
        self.time += delta_time;
        self.phase_stats = PhaseStats::default();
    }

    /// The read stage: run every read-only instance against `world`, in
    /// parallel when there are enough of them. Holds no lock of its own;
    /// the caller needs a read lock (or nothing, if it owns the world).
    pub fn run_read_stage(&mut self, world: &World, delta_time: f64) -> Vec<RuntimeError> {
        let started = Instant::now();
        let ids = self.read_stage_ids();
        let mut batch: Vec<(String, ScriptInstance)> = ids
            .iter()
            .filter_map(|id| self.instances.remove(id).map(|i| (id.clone(), i)))
            .collect();
        let threads = self.worker_count(batch.len());
        let context = ReadContext {
            classes: &self.classes,
            class_budgets: &self.class_budgets,
            default_budget: self.budget,
            time: self.time,
            world,
            max_depth: self.vm.max_depth,
            checked_arithmetic: self.vm.checked_arithmetic,
        };
        let mut errors: Vec<Vec<RuntimeError>> = (0..batch.len()).map(|_| Vec::new()).collect();
        if threads <= 1 {
            let mut vm = new_vm(&context);
            for ((id, instance), errors) in batch.iter_mut().zip(&mut errors) {
                *errors = run_instance(&context, &mut vm, id, instance, delta_time);
            }
        } else {
            let chunk = batch.len().div_ceil(threads);
            std::thread::scope(|scope| {
                for (instances, errors) in batch.chunks_mut(chunk).zip(errors.chunks_mut(chunk)) {
                    let context = &context;
                    scope.spawn(move || {
                        let mut vm = new_vm(context);
                        for ((id, instance), errors) in instances.iter_mut().zip(errors) {
                            *errors = run_instance(context, &mut vm, id, instance, delta_time);
                        }
                    });
                }
            });
        }
        let ran = batch.len();
        for (id, instance) in batch {
            self.instances.insert(id, instance);
        }
        self.phase_stats.read_instances = ran;
        self.phase_stats.read_threads = threads.max(1);
        self.phase_stats.read_time = started.elapsed();
        let errors: Vec<RuntimeError> = errors.into_iter().flatten().collect();
        for error in &errors {
            tracing::warn!("{error}");
        }
        errors
    }

    /// The write stage: everything the read stage did not run, sequentially,
    /// in spawn order, with exclusive access. Calls resumed outside a tick
    /// that failed are reported here too.
    pub fn run_write_stage(&mut self, world: &mut World, delta_time: f64) -> Vec<RuntimeError> {
        let started = Instant::now();
        let read: std::collections::HashSet<String> = self.read_stage_ids().into_iter().collect();
        let ids: Vec<String> = self
            .order
            .iter()
            .filter(|id| !self.pending_begin_play.contains(id) && !read.contains(*id))
            .cloned()
            .collect();
        let mut errors: Vec<RuntimeError> = std::mem::take(&mut self.deferred_errors);
        errors.extend(
            ids.iter()
                .flat_map(|id| self.fire_timers(id, delta_time, world)),
        );
        errors.extend(ids.iter().flat_map(|id| self.resume_due(id, world)));
        errors.extend(ids.iter().filter_map(|id| {
            let func = self
                .instances
                .get(id)
                .and_then(|i| self.classes.get(&i.class))
                .and_then(|c| c.entries.tick)?;
            self.call(id, func, &[Value::Float(delta_time)], world)
                .err()
        }));
        for error in &errors {
            tracing::warn!("{error}");
        }
        self.phase_stats.write_instances = ids.len();
        self.phase_stats.write_time = started.elapsed();
        errors
    }

    /// What the last tick's stages did.
    pub fn phase_stats(&self) -> PhaseStats {
        self.phase_stats
    }

    /// Run the read stage inline below this many instances (default
    /// [`DEFAULT_PARALLEL_THRESHOLD`]); `usize::MAX` disables threads.
    pub fn set_parallel_threshold(&mut self, instances: usize) {
        self.parallel_threshold = instances;
    }

    /// Threads for `instances` read-only instances: one per
    /// `parallel_threshold` of them, at most the machine's parallelism.
    fn worker_count(&self, instances: usize) -> usize {
        if instances < self.parallel_threshold.max(2) {
            return 1;
        }
        let machine = std::thread::available_parallelism().map_or(1, usize::from);
        (instances / self.parallel_threshold.max(1)).clamp(1, machine)
    }
}

fn new_vm(context: &ReadContext<'_>) -> Vm {
    let mut vm = Vm::new();
    vm.max_depth = context.max_depth;
    vm.checked_arithmetic = context.checked_arithmetic;
    vm
}

/// One instance's tick in the read stage: timers, calls whose wait is
/// over, `tick`. Mirrors the write stage's sequence without the debugger
/// and the event hub (see the module docs).
fn run_instance(
    context: &ReadContext<'_>,
    vm: &mut Vm,
    object_id: &str,
    instance: &mut ScriptInstance,
    delta_time: f64,
) -> Vec<RuntimeError> {
    let Some(class) = context.classes.get(&instance.class) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    let run = |instance: &mut ScriptInstance,
               outcome: Result<Completion, pulsar_script_vm::ScriptError>,
               spent: u64| {
        instance.instructions_executed = instance.instructions_executed.saturating_add(spent);
        match outcome {
            Ok(Completion::Returned(_)) => None,
            Ok(Completion::Waiting {
                seconds,
                continuation,
            }) => {
                park(instance, context.time, seconds, continuation);
                None
            }
            // Instances being debugged never run here.
            Ok(Completion::Paused { .. }) => None,
            Err(source) => Some(RuntimeError::Script {
                object_id: object_id.to_owned(),
                class: instance.class.clone(),
                source,
            }),
        }
    };
    let limit = context
        .class_budgets
        .get(&instance.class)
        .copied()
        .unwrap_or(context.default_budget);
    let entity = instance.entity.unwrap_or(Entity::DANGLING);

    // Timers.
    for timer in instance.latent.advance(delta_time) {
        let Some(func) = class.program.entry(&timer.function) else {
            errors.push(RuntimeError::UnknownEvent {
                class: instance.class.clone(),
                name: timer.function,
            });
            continue;
        };
        let mut budget = Budget::new(limit);
        let before = budget.remaining;
        let outcome = {
            let mut host = Host::read_only(context.world, entity, context.time)
                .with_latent(Some(&mut instance.latent));
            vm.start(
                &class.program,
                &mut instance.state,
                func,
                &[],
                &mut host,
                &mut budget,
            )
        };
        errors.extend(run(instance, outcome, before - budget.remaining));
    }

    // Calls whose wait is over: latent (frames, conditions), then timed.
    let mut ready: Vec<Continuation> = Vec::new();
    let mut still = Vec::new();
    for (wake, continuation) in std::mem::take(&mut instance.blocked) {
        match wake {
            Wake::Frames(remaining) if remaining > 1 => {
                still.push((Wake::Frames(remaining - 1), continuation))
            }
            Wake::Frames(_) => ready.push(continuation),
            Wake::Event { name } => still.push((Wake::Event { name }, continuation)),
            Wake::Until { predicate } => {
                match predicate_holds(context, vm, instance, class, &predicate, limit) {
                    Ok(true) => ready.push(continuation),
                    Ok(false) => still.push((Wake::Until { predicate }, continuation)),
                    Err(error) => errors.push(error_for(object_id, instance, error)),
                }
            }
        }
    }
    instance.blocked = still;
    let now = context.time;
    let (mut due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut instance.waiting)
        .into_iter()
        .partition(|(at, _)| *at <= now);
    instance.waiting = later;
    due.sort_by(|a, b| a.0.total_cmp(&b.0));
    due.extend(ready.into_iter().map(|continuation| (now, continuation)));
    for (_, continuation) in due {
        let mut budget = Budget::new(limit);
        let before = budget.remaining;
        let outcome = {
            let mut host =
                Host::read_only(context.world, entity, now).with_latent(Some(&mut instance.latent));
            vm.resume(
                &class.program,
                &mut instance.state,
                continuation,
                &mut host,
                &mut budget,
            )
        };
        errors.extend(run(instance, outcome, before - budget.remaining));
    }

    // `tick`.
    if let Some(func) = class.entries.tick {
        let mut budget = Budget::new(limit);
        let before = budget.remaining;
        let outcome = {
            let mut host =
                Host::read_only(context.world, entity, now).with_latent(Some(&mut instance.latent));
            vm.start(
                &class.program,
                &mut instance.state,
                func,
                &[Value::Float(delta_time)],
                &mut host,
                &mut budget,
            )
        };
        errors.extend(run(instance, outcome, before - budget.remaining));
    }
    errors
}

/// A `wait::until` predicate in the read stage: no latent state, no
/// debugger; it must return `bool` without parameters.
fn predicate_holds(
    context: &ReadContext<'_>,
    vm: &mut Vm,
    instance: &mut ScriptInstance,
    class: &Class,
    name: &str,
    limit: u64,
) -> Result<bool, PredicateError> {
    let func = class
        .program
        .entry(name)
        .ok_or_else(|| PredicateError::Missing(name.to_owned()))?;
    let function = &class.program.module().functions[func.0 as usize];
    if !function.params.is_empty() || function.ret != Type::Bool {
        return Err(PredicateError::Shape(format!(
            "`wait::until` needs a function with no parameters that returns bool, `{name}` is ({}) -> {}",
            function.params.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
            function.ret
        )));
    }
    let mut budget = Budget::new(limit);
    let before = budget.remaining;
    let mut host = Host::read_only(
        context.world,
        instance.entity.unwrap_or(Entity::DANGLING),
        context.time,
    );
    let outcome = vm.call(
        &class.program,
        &mut instance.state,
        func,
        &[],
        &mut host,
        &mut budget,
    );
    instance.instructions_executed = instance
        .instructions_executed
        .saturating_add(before - budget.remaining);
    match outcome {
        Ok(Value::Bool(holds)) => Ok(holds),
        Ok(other) => Err(PredicateError::Shape(format!(
            "`{name}` returned {}, expected bool",
            other.kind()
        ))),
        Err(source) => Err(PredicateError::Script(source)),
    }
}

enum PredicateError {
    Missing(String),
    Shape(String),
    Script(pulsar_script_vm::ScriptError),
}

fn error_for(object_id: &str, instance: &ScriptInstance, error: PredicateError) -> RuntimeError {
    match error {
        PredicateError::Missing(name) => RuntimeError::UnknownEvent {
            class: instance.class.clone(),
            name,
        },
        PredicateError::Shape(reason) => RuntimeError::State {
            object_id: object_id.to_owned(),
            reason,
        },
        PredicateError::Script(source) => RuntimeError::Script {
            object_id: object_id.to_owned(),
            class: instance.class.clone(),
            source,
        },
    }
}
