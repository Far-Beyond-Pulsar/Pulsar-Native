//! Spawning, binding and despawning instances, and reading and writing their variables.

use std::collections::HashMap;

use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::migrate;
use pulsar_script_vm::{Debugger, Latent, Value};

use super::ScriptRuntime;
use crate::class::{ScriptInstance, END_PLAY};
use crate::json::value_from_json_with;
use crate::RuntimeError;

impl ScriptRuntime {
    /// The class an instance runs.
    pub fn class_of(&self, object_id: &str) -> Option<&str> {
        self.instances.get(object_id).map(|i| i.class.as_str())
    }

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
        let program = &self
            .classes
            .get(class)
            .ok_or_else(|| RuntimeError::UnknownClass(class.to_owned()))?
            .program;
        let mut state = program.instantiate();
        for (name, value) in overrides {
            // A key is a stable variable id or a name (data from before ids).
            let index =
                migrate::resolve_key(&program.module().variables, name).map_err(|reason| {
                    RuntimeError::BadVariable {
                        name: name.clone(),
                        reason: format!("`{class}`: {reason}"),
                    }
                })?;
            program
                .set_var(&mut state, index, value.clone())
                .map_err(|reason| RuntimeError::BadVariable {
                    name: name.clone(),
                    reason,
                })?;
        }
        self.instances.insert(
            object_id.clone(),
            ScriptInstance {
                class: class.to_owned(),
                state,
                entity,
                waiting: Vec::new(),
                blocked: Vec::new(),
                latent: Latent::new(),
                paused: Vec::new(),
                debugger: Debugger::new(),
                instructions_executed: 0,
            },
        );
        self.order.push(object_id.clone());
        self.pending_begin_play.push(object_id);
        Ok(())
    }

    /// Like [`spawn`](Self::spawn), with overrides as JSON (level files).
    /// Entities in them are StableId strings, which `entity_for` resolves
    /// (see [`value_from_json_with`]).
    ///
    /// Each override stands alone: one naming a variable the class no longer
    /// has is ignored with a warning, and one that does not convert keeps the
    /// class default and comes back in the returned list. Only a problem
    /// with the instance itself (unknown class, duplicate id) is an `Err`.
    pub fn spawn_with_json(
        &mut self,
        object_id: impl Into<String>,
        class: &str,
        entity: Option<Entity>,
        overrides: &HashMap<String, serde_json::Value>,
        entity_for: &dyn Fn(&str) -> Option<Entity>,
    ) -> Result<Vec<RuntimeError>, RuntimeError> {
        let program = &self
            .classes
            .get(class)
            .ok_or_else(|| RuntimeError::UnknownClass(class.to_owned()))?
            .program;
        let mut converted = Vec::with_capacity(overrides.len());
        let mut rejected = Vec::new();
        for (name, json) in overrides {
            // Level files outlive graph edits: a variable that no longer
            // exists is skipped, not fatal.
            let variables = &program.module().variables;
            let var = match migrate::resolve_key(variables, name) {
                Ok(index) => &variables[index],
                Err(reason)
                    if variables
                        .iter()
                        .any(|v| v.id.as_deref() == Some(name) || &v.name == name) =>
                {
                    // Known but ambiguous: refuse instead of guessing.
                    rejected.push(RuntimeError::BadVariable {
                        name: name.clone(),
                        reason: reason.to_string(),
                    });
                    continue;
                }
                Err(_) => {
                    tracing::warn!("`{class}` has no variable `{name}`; ignoring its override");
                    continue;
                }
            };
            match value_from_json_with(json, &var.ty, entity_for) {
                Ok(value) => converted.push((name.clone(), value)),
                Err(reason) => rejected.push(RuntimeError::BadVariable {
                    name: name.clone(),
                    reason,
                }),
            }
        }
        self.spawn(object_id, class, entity, &converted)?;
        Ok(rejected)
    }

    /// Run `end_play` (if the instance has begun) and remove the instance.
    pub fn despawn(&mut self, object_id: &str, world: &mut World) -> Result<(), RuntimeError> {
        if !self.instances.contains_key(object_id) {
            return Err(RuntimeError::UnknownInstance(object_id.to_owned()));
        }
        let begun = !self.pending_begin_play.iter().any(|id| id == object_id);
        let result = if begun {
            self.run_lifecycle(object_id, END_PLAY, world)
        } else {
            Ok(())
        };
        self.instances.remove(object_id);
        self.order.retain(|id| id != object_id);
        self.pending_begin_play.retain(|id| id != object_id);
        result
    }

    pub fn bind(
        &mut self,
        object_id: &str,
        entity: Entity,
    ) -> Result<Option<Entity>, RuntimeError> {
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

    pub fn set_variable(
        &mut self,
        object_id: &str,
        name: &str,
        value: Value,
    ) -> Result<(), RuntimeError> {
        let instance = self
            .instances
            .get_mut(object_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let program = &self
            .classes
            .get(&instance.class)
            .ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?
            .program;
        let bad = |reason: String| RuntimeError::BadVariable {
            name: name.to_owned(),
            reason,
        };
        let index = program
            .variable(name)
            .ok_or_else(|| bad("no such variable".into()))?;
        program
            .set_var(&mut instance.state, index, value)
            .map_err(bad)
    }

    pub(crate) fn instance_mut(
        &mut self,
        object_id: &str,
    ) -> Result<&mut ScriptInstance, RuntimeError> {
        self.instances
            .get_mut(object_id)
            .ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))
    }
}
