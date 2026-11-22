//! Running lifecycle events, custom events and waiting calls on instances.

use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::{Budget, Completion, Debugger, EventSink, FuncId, Host, Value};

use super::ScriptRuntime;
use crate::class::{BEGIN_PLAY, END_PLAY};
use crate::latent;
use crate::RuntimeError;

impl ScriptRuntime {
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
        self.begin_tick(delta_time);
        let mut errors = self.run_read_stage(world, delta_time);
        errors.extend(self.run_write_stage(world, delta_time));
        errors
    }

    /// Resume an instance's calls that are due at the current time. Calls
    /// that wait again (even for zero seconds) resume on a later tick.
    pub(crate) fn resume_due(&mut self, object_id: &str, world: &mut World) -> Vec<RuntimeError> {
        // Calls whose latent wait (frames, a condition) is over resume with
        // the ones whose time has come.
        let (ready, mut errors) = self.ready_blocked(object_id, world);
        let Some(instance) = self.instances.get_mut(object_id) else {
            return errors;
        };
        let now = self.time;
        let (mut due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut instance.waiting)
            .into_iter()
            .partition(|(wake, _)| *wake <= now);
        instance.waiting = later;
        due.sort_by(|a, b| a.0.total_cmp(&b.0));
        due.extend(ready.into_iter().map(|continuation| (now, continuation)));
        for (_, continuation) in due {
            let Some(instance) = self.instances.get_mut(object_id) else {
                break;
            };
            let Some(class) = self.classes.get(&instance.class) else {
                break;
            };
            let mut host = Host::at_time(world, instance.entity.unwrap_or(Entity::DANGLING), now)
                .with_events(self.events.as_deref().map(|e| e as &dyn EventSink))
                .with_latent(Some(&mut instance.latent));
            let mut budget = Budget::new(
                self.class_budgets
                    .get(&instance.class)
                    .copied()
                    .unwrap_or(self.budget),
            );
            let starting_budget = budget.remaining;
            let function = continuation.functions().last().copied().unwrap_or("resume");
            let _profile_scope = profiling::is_profiling_enabled().then(|| {
                profiling::ProfileScope::new(format!("script:{}::{function}", instance.class))
            });
            let prior_debugger = self.vm.debugger.take();
            self.vm.debugger = Some(std::mem::replace(&mut instance.debugger, Debugger::new()));
            let outcome = self.vm.resume(
                &class.program,
                &mut instance.state,
                continuation,
                &mut host,
                &mut budget,
            );
            instance.debugger = self.vm.debugger.take().unwrap_or_default();
            self.vm.debugger = prior_debugger;
            match outcome {
                Ok(Completion::Returned(_)) => {}
                Ok(Completion::Waiting {
                    seconds,
                    continuation,
                }) => {
                    latent::park(instance, now, seconds, continuation);
                }
                Ok(Completion::Paused {
                    snapshot,
                    continuation,
                }) => {
                    self.debug_events
                        .push((object_id.to_owned(), snapshot.clone()));
                    instance.paused.push((snapshot, continuation));
                }
                Err(source) => errors.push(RuntimeError::Script {
                    object_id: object_id.to_owned(),
                    class: instance.class.clone(),
                    source,
                }),
            }
            instance.instructions_executed = instance
                .instructions_executed
                .saturating_add(starting_budget - budget.remaining);
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
        let instance = self
            .instances
            .get(object_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let class = self
            .classes
            .get(&instance.class)
            .ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?;
        let func = class
            .program
            .entry(name)
            .ok_or_else(|| RuntimeError::UnknownEvent {
                class: instance.class.clone(),
                name: name.to_owned(),
            })?;
        let result = self.call(object_id, func, args, world)?;
        self.wake_event(object_id, name, world);
        Ok(result)
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
        let result = self.call(object_id, func, args, world)?;
        // Calls waiting for the event this function handles resume now.
        for name in self.event_names_handled_by(object_id, func) {
            self.wake_event(object_id, &name, world);
        }
        Ok(result)
    }

    /// Whether `object_id` has run `begin_play` (or has none pending).
    pub fn has_begun(&self, object_id: &str) -> bool {
        self.instances.contains_key(object_id)
            && !self.pending_begin_play.iter().any(|id| id == object_id)
    }

    pub(crate) fn run_lifecycle(
        &mut self,
        object_id: &str,
        event: &str,
        world: &mut World,
    ) -> Result<(), RuntimeError> {
        let instance = self
            .instances
            .get(object_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let Some(class) = self.classes.get(&instance.class) else {
            return Ok(());
        };
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

    pub(crate) fn call(
        &mut self,
        object_id: &str,
        func: FuncId,
        args: &[Value],
        world: &mut World,
    ) -> Result<Value, RuntimeError> {
        let instance = self
            .instances
            .get_mut(object_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let class = self
            .classes
            .get(&instance.class)
            .ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?;
        // Unbound instances run with a dangling entity: every component
        // access fails its liveness check instead of reaching anything.
        let mut host = Host::at_time(
            world,
            instance.entity.unwrap_or(Entity::DANGLING),
            self.time,
        )
        .with_events(self.events.as_deref().map(|e| e as &dyn EventSink))
        .with_latent(Some(&mut instance.latent));
        let mut budget = Budget::new(
            self.class_budgets
                .get(&instance.class)
                .copied()
                .unwrap_or(self.budget),
        );
        let starting_budget = budget.remaining;
        let function_name = class
            .program
            .module()
            .functions
            .get(func.0 as usize)
            .map_or("call", |f| f.name.as_str());
        let _profile_scope = profiling::is_profiling_enabled().then(|| {
            profiling::ProfileScope::new(format!("script:{}::{function_name}", instance.class))
        });
        let prior_debugger = self.vm.debugger.take();
        self.vm.debugger = Some(std::mem::replace(&mut instance.debugger, Debugger::new()));
        let outcome = self.vm.start(
            &class.program,
            &mut instance.state,
            func,
            args,
            &mut host,
            &mut budget,
        );
        instance.debugger = self.vm.debugger.take().unwrap_or_default();
        self.vm.debugger = prior_debugger;
        match outcome {
            Ok(Completion::Returned(value)) => {
                instance.instructions_executed = instance
                    .instructions_executed
                    .saturating_add(starting_budget - budget.remaining);
                Ok(value)
            }
            // A latent event: it finishes on a later tick, so the caller
            // gets unit now.
            Ok(Completion::Waiting {
                seconds,
                continuation,
            }) => {
                latent::park(instance, self.time, seconds, continuation);
                instance.instructions_executed = instance
                    .instructions_executed
                    .saturating_add(starting_budget - budget.remaining);
                Ok(Value::Unit)
            }
            Ok(Completion::Paused {
                snapshot,
                continuation,
            }) => {
                self.debug_events
                    .push((object_id.to_owned(), snapshot.clone()));
                instance.paused.push((snapshot, continuation));
                instance.instructions_executed = instance
                    .instructions_executed
                    .saturating_add(starting_budget - budget.remaining);
                Ok(Value::Unit)
            }
            Err(source) => {
                instance.instructions_executed = instance
                    .instructions_executed
                    .saturating_add(starting_budget - budget.remaining);
                Err(RuntimeError::Script {
                    object_id: object_id.to_owned(),
                    class: instance.class.clone(),
                    source,
                })
            }
        }
    }
}
