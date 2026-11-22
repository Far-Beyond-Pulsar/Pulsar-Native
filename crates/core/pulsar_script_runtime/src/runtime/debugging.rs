//! Breakpoints, paused calls and resuming them with debugger commands.

use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::{
    Budget, Completion, DebugCommand, DebugSnapshot, Debugger, EventSink, Host,
};

use super::ScriptRuntime;
use crate::latent;
use crate::RuntimeError;

impl ScriptRuntime {
    /// Stop snapshots for `object_id`, in the order the calls paused.
    pub fn paused_calls(&self, object_id: &str) -> Vec<DebugSnapshot> {
        self.instances
            .get(object_id)
            .map(|instance| {
                instance
                    .paused
                    .iter()
                    .map(|(snapshot, _)| snapshot.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Set a bytecode breakpoint for one instance.
    pub fn set_breakpoint(
        &mut self,
        object_id: &str,
        function: impl Into<String>,
        pc: usize,
    ) -> Result<(), RuntimeError> {
        self.instance_mut(object_id)?
            .debugger
            .set_breakpoint(function, pc);
        Ok(())
    }

    /// Set breakpoints on all bytecode instructions emitted from a graph node.
    pub fn set_node_breakpoint(
        &mut self,
        object_id: &str,
        function: &str,
        file: &str,
        node: &str,
    ) -> Result<usize, RuntimeError> {
        let instance = self
            .instances
            .get(object_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let class = self
            .classes
            .get(&instance.class)
            .ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?;
        let module = class.program.module();
        Ok(self
            .instances
            .get_mut(object_id)
            .expect("instance checked")
            .debugger
            .set_node_breakpoint(module, function, file, node))
    }

    /// Remove a bytecode breakpoint for one instance.
    pub fn remove_breakpoint(
        &mut self,
        object_id: &str,
        function: &str,
        pc: usize,
    ) -> Result<bool, RuntimeError> {
        Ok(self
            .instance_mut(object_id)?
            .debugger
            .remove_breakpoint(function, pc))
    }

    /// Drain debugger stop notifications in the order the runtime observed them.
    pub fn take_debug_events(&mut self) -> Vec<(String, DebugSnapshot)> {
        std::mem::take(&mut self.debug_events)
    }

    /// The current stop snapshot for one paused call.
    pub fn debug_snapshot(&self, object_id: &str, index: usize) -> Option<&DebugSnapshot> {
        self.instances
            .get(object_id)?
            .paused
            .get(index)
            .map(|(snapshot, _)| snapshot)
    }

    /// Continue one paused call with a debugger command. A paused call is
    /// removed from the queue only when resumed; a new stop is available
    /// through [`debug_snapshot`](Self::debug_snapshot) and
    /// [`take_debug_events`](Self::take_debug_events).
    pub fn resume_paused(
        &mut self,
        object_id: &str,
        index: usize,
        command: DebugCommand,
        world: &mut World,
    ) -> Result<(), RuntimeError> {
        let instance = self
            .instances
            .get_mut(object_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        if index >= instance.paused.len() {
            return Err(RuntimeError::State {
                object_id: object_id.to_owned(),
                reason: format!("no paused call at index {index}"),
            });
        }
        let (snapshot, continuation) = instance.paused.remove(index);
        let class_name = instance.class.clone();
        let class = self
            .classes
            .get(&class_name)
            .ok_or_else(|| RuntimeError::UnknownClass(class_name.clone()))?;
        instance.debugger.command(command, &snapshot);
        let mut host = Host::at_time(
            world,
            instance.entity.unwrap_or(Entity::DANGLING),
            self.time,
        )
        .with_events(self.events.as_deref().map(|e| e as &dyn EventSink))
        .with_latent(Some(&mut instance.latent));
        let mut budget = Budget::new(
            self.class_budgets
                .get(&class_name)
                .copied()
                .unwrap_or(self.budget),
        );
        let starting_budget = budget.remaining;
        let function = continuation.functions().last().copied().unwrap_or("resume");
        let _profile_scope = profiling::is_profiling_enabled()
            .then(|| profiling::ProfileScope::new(format!("script:{class_name}::{function}")));
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
        instance.instructions_executed = instance
            .instructions_executed
            .saturating_add(starting_budget - budget.remaining);
        match outcome {
            Ok(Completion::Returned(_)) => Ok(()),
            Ok(Completion::Waiting {
                seconds,
                continuation,
            }) => {
                latent::park(instance, self.time, seconds, continuation);
                Ok(())
            }
            Ok(Completion::Paused {
                snapshot,
                continuation,
            }) => {
                self.debug_events
                    .push((object_id.to_owned(), snapshot.clone()));
                instance.paused.push((snapshot.clone(), continuation));
                Ok(())
            }
            Err(source) => Err(RuntimeError::Script {
                object_id: object_id.to_owned(),
                class: class_name,
                source,
            }),
        }
    }
}
