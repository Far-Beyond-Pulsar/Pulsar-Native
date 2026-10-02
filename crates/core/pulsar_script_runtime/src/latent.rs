//! Latent actions at runtime: calls parked on frames, conditions or events,
//! and timers.
//!
//! The VM only knows how to suspend a call (see [`pulsar_script_vm::latent`]).
//! What it waits for lives here, per instance:
//!
//! - **frames**: counted down once per [`tick_all`](ScriptRuntime::tick_all);
//! - **conditions** (`wait::until`): an exported, parameterless `bool`
//!   function evaluated once per tick, before `tick`;
//! - **events** (`wait::event`): resumed right after the instance handles
//!   an event of that name (a subscribed hub event or a custom event sent
//!   with [`send_event`](ScriptRuntime::send_event));
//! - **scheduled calls** (`schedule::*`): fired once per tick, each calling an exported
//!   function of the class.
//!
//! Order within a tick: timers, then calls whose wait is over (time,
//! frames, conditions), then `tick`. A call that waits again resumes on a
//! later tick, never the one it is in.

use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::{Budget, Completion, Continuation, EventSink, Host, Type, Value, Wake};

use crate::{RuntimeError, ScriptInstance, ScriptRuntime};

/// Where a call that just suspended goes: the timed queue for a `Wait`, or
/// the instance's blocked list for what a latent native asked for.
pub(crate) fn park(instance: &mut ScriptInstance, now: f64, seconds: f64, continuation: Continuation) {
    match instance.latent.take_request() {
        Some(wake) => instance.blocked.push((wake, continuation)),
        None => instance.waiting.push((now + seconds, continuation)),
    }
}

impl ScriptRuntime {
    /// Advance an instance's timers by `delta` and run the ones that came
    /// due. A timer whose function no longer exists is dropped with an
    /// error.
    pub(crate) fn fire_timers(&mut self, object_id: &str, delta: f64, world: &mut World) -> Vec<RuntimeError> {
        let Some(instance) = self.instances.get_mut(object_id) else { return Vec::new() };
        let fired = instance.latent.advance(delta);
        let mut errors = Vec::new();
        for timer in fired {
            let Some(instance) = self.instances.get(object_id) else { break };
            let class = instance.class.clone();
            let entry = self.classes.get(&class).and_then(|c| c.program.entry(&timer.function));
            match entry {
                Some(func) => {
                    if let Err(error) = self.call(object_id, func, &[], world) {
                        errors.push(error);
                    }
                }
                None => errors.push(RuntimeError::UnknownEvent { class, name: timer.function }),
            }
        }
        errors
    }

    /// Move the calls whose wait is over from `blocked` to the front of the
    /// instance's due list, and return them. Frames count down; conditions
    /// are evaluated now; events are not touched (they wake on delivery).
    pub(crate) fn ready_blocked(&mut self, object_id: &str, world: &mut World) -> (Vec<Continuation>, Vec<RuntimeError>) {
        let Some(instance) = self.instances.get_mut(object_id) else { return (Vec::new(), Vec::new()) };
        let blocked = std::mem::take(&mut instance.blocked);
        let (mut ready, mut errors, mut still) = (Vec::new(), Vec::new(), Vec::new());
        for (wake, continuation) in blocked {
            match wake {
                Wake::Frames(remaining) if remaining > 1 => still.push((Wake::Frames(remaining - 1), continuation)),
                Wake::Frames(_) => ready.push(continuation),
                Wake::Event { name } => still.push((Wake::Event { name }, continuation)),
                Wake::Until { predicate } => match self.evaluate_predicate(object_id, &predicate, world) {
                    Ok(true) => ready.push(continuation),
                    Ok(false) => still.push((Wake::Until { predicate }, continuation)),
                    // A broken predicate cannot be retried every tick: drop
                    // the call and say why.
                    Err(error) => errors.push(error),
                },
            }
        }
        if let Some(instance) = self.instances.get_mut(object_id) {
            instance.blocked = still;
        }
        (ready, errors)
    }

    /// Run the exported `bool` function `name` of an instance's class, with
    /// no latent state (it cannot wait or set timers) and no debugger.
    fn evaluate_predicate(&mut self, object_id: &str, name: &str, world: &mut World) -> Result<bool, RuntimeError> {
        let state_error = |reason: String| RuntimeError::State { object_id: object_id.to_owned(), reason };
        let instance =
            self.instances.get_mut(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let class = self.classes.get(&instance.class).ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?;
        let func = class.program.entry(name).ok_or_else(|| RuntimeError::UnknownEvent {
            class: instance.class.clone(),
            name: name.to_owned(),
        })?;
        let function = &class.program.module().functions[func.0 as usize];
        if !function.params.is_empty() || function.ret != Type::Bool {
            return Err(state_error(format!(
                "`wait::until` needs a function with no parameters that returns bool, `{name}` is ({}) -> {}",
                function.params.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
                function.ret
            )));
        }
        let mut host = Host::at_time(world, instance.entity.unwrap_or(Entity::DANGLING), self.time)
            .with_events(self.events.as_deref().map(|e| e as &dyn EventSink));
        let mut budget = Budget::new(self.class_budgets.get(&instance.class).copied().unwrap_or(self.budget));
        let starting_budget = budget.remaining;
        let prior_debugger = self.vm.debugger.take();
        let outcome = self.vm.call(&class.program, &mut instance.state, func, &[], &mut host, &mut budget);
        self.vm.debugger = prior_debugger;
        instance.instructions_executed = instance.instructions_executed.saturating_add(starting_budget - budget.remaining);
        match outcome {
            Ok(Value::Bool(holds)) => Ok(holds),
            Ok(other) => Err(state_error(format!("`{name}` returned {}, expected bool", other.kind()))),
            Err(source) => Err(RuntimeError::Script { object_id: object_id.to_owned(), class: instance.class.clone(), source }),
        }
    }

    /// An event named `name` reached the instance: resume the calls waiting
    /// for it. Errors from the resumed calls are queued for the next
    /// [`tick_all`](Self::tick_all) to report.
    pub(crate) fn wake_event(&mut self, object_id: &str, name: &str, world: &mut World) {
        let Some(instance) = self.instances.get_mut(object_id) else { return };
        let (woken, still): (Vec<_>, Vec<_>) = std::mem::take(&mut instance.blocked)
            .into_iter()
            .partition(|(wake, _)| matches!(wake, Wake::Event { name: waiting_for } if waiting_for == name));
        instance.blocked = still;
        let now = self.time;
        for (_, continuation) in woken {
            if let Err(error) = self.resume_continuation(object_id, continuation, now, world) {
                self.deferred_errors.push(error);
            }
        }
    }

    /// The names of the events an exported-or-not function handles for
    /// this instance's class (its subscriptions to `func`).
    pub(crate) fn event_names_handled_by(&self, object_id: &str, func: pulsar_script_vm::FuncId) -> Vec<String> {
        let Some(instance) = self.instances.get(object_id) else { return Vec::new() };
        let Some(class) = self.classes.get(&instance.class) else { return Vec::new() };
        class
            .program
            .subscriptions()
            .iter()
            .filter(|s| s.handler == func)
            .filter_map(|s| s.event_name.clone())
            .collect()
    }

    /// Resume one continuation now, parking it again if it waits.
    pub(crate) fn resume_continuation(
        &mut self,
        object_id: &str,
        continuation: Continuation,
        now: f64,
        world: &mut World,
    ) -> Result<(), RuntimeError> {
        let instance =
            self.instances.get_mut(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let class = self.classes.get(&instance.class).ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?;
        let mut host = Host::at_time(world, instance.entity.unwrap_or(Entity::DANGLING), now)
            .with_events(self.events.as_deref().map(|e| e as &dyn EventSink))
            .with_latent(Some(&mut instance.latent));
        let mut budget = Budget::new(self.class_budgets.get(&instance.class).copied().unwrap_or(self.budget));
        let starting_budget = budget.remaining;
        let prior_debugger = self.vm.debugger.take();
        let outcome = self.vm.resume(&class.program, &mut instance.state, continuation, &mut host, &mut budget);
        self.vm.debugger = prior_debugger;
        instance.instructions_executed = instance.instructions_executed.saturating_add(starting_budget - budget.remaining);
        match outcome {
            Ok(Completion::Returned(_)) => Ok(()),
            Ok(Completion::Waiting { seconds, continuation }) => {
                park(instance, now, seconds, continuation);
                Ok(())
            }
            // A debugger stop inside an event-woken call is not wired here:
            // the call is left where it stopped, resumable from the debugger.
            Ok(Completion::Paused { snapshot, continuation }) => {
                self.debug_events.push((object_id.to_owned(), snapshot.clone()));
                instance.paused.push((snapshot, continuation));
                Ok(())
            }
            Err(source) => Err(RuntimeError::Script { object_id: object_id.to_owned(), class: instance.class.clone(), source }),
        }
    }

    /// After a class reload: waiting-on-condition calls are rebased onto the
    /// new code like timed waits (dropped, and reported, if its layout no
    /// longer fits), and timers whose function is gone are dropped.
    pub(crate) fn carry_latent_across_reload(&mut self, class_name: &str, report: &mut crate::ReloadReport) {
        let Some(class) = self.classes.get(class_name) else { return };
        let module = std::sync::Arc::clone(class.program.module());
        let exported: std::collections::HashSet<String> =
            module.functions.iter().filter(|f| f.exported).map(|f| f.name.clone()).collect();
        for (object_id, instance) in self.instances.iter_mut().filter(|(_, i)| i.class == class_name) {
            for (wake, continuation) in std::mem::take(&mut instance.blocked) {
                match continuation.rebase(&module) {
                    Ok(rebased) => {
                        instance.blocked.push((wake, rebased));
                        report.kept += 1;
                    }
                    Err(reason) => {
                        let function = continuation.functions().last().map(|f| f.to_string()).unwrap_or_default();
                        tracing::warn!(class = %class_name, function = %function, "reload dropped a waiting script call of `{class_name}::{function}`: {reason}");
                        report.dropped.push(crate::DroppedCall { object_id: object_id.clone(), function, reason });
                    }
                }
            }
            for function in instance.latent.retain_functions(|f| exported.contains(f)) {
                tracing::warn!(class = %class_name, %function, "reload dropped a timer: the function is no longer exported");
            }
        }
    }
}
