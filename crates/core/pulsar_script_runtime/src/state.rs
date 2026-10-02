//! Saving and restoring an instance's variables.
//!
//! A [`SavedState`] is a versioned, language-neutral snapshot of one
//! instance: the class, its schema version, and each persistable variable
//! with its stable id, name, type and value. Restoring it into the current
//! version of the class goes through the same migration as hot reload
//! (match by id, then name; report what changed; run `migrate` if the class
//! version moved on), so a save written by an older version of a class
//! loads into a newer one.
//!
//! Entity and component references are process-local handles and are never
//! saved: those variables keep whatever the host bound them to. This is the
//! codec a save/load system builds on; it is not a save system.

use pulsar_scenedb::World;
use pulsar_script_vm::{Type, TypeRegistry, Value, Variable};
use serde::{Deserialize, Serialize};

use crate::{carry_state, value_from_json, ChangeKind, RuntimeError, ScriptRuntime, VariableChange};

/// One instance's saved variables. See the module docs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedState {
    pub class: String,
    #[serde(default)]
    pub class_version: u32,
    pub variables: Vec<SavedVariable>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedVariable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub ty: Type,
    pub value: serde_json::Value,
}

/// What restoring a [`SavedState`] did.
#[derive(Debug, Default)]
pub struct RestoreReport {
    /// Variables restored unchanged.
    pub variables_kept: usize,
    /// Renames, resets, retypes, removals and `migrate` runs.
    pub changes: Vec<VariableChange>,
    /// Saved values that could not be read back (variable name, reason);
    /// those variables start at their defaults.
    pub unreadable: Vec<(String, String)>,
}

/// A script value as JSON, for saving. `Err` for handles that are only
/// meaningful inside one process.
pub fn value_to_json(value: &Value) -> Result<serde_json::Value, String> {
    use serde_json::Value as J;
    match value {
        Value::Bool(b) => Ok(J::Bool(*b)),
        Value::Int(i) => Ok(J::from(*i)),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .ok_or_else(|| format!("{f} is not a finite number")),
        Value::Str(s) => Ok(J::String(s.to_string())),
        Value::Object(object) => {
            let text = TypeRegistry::global().encode_value(object)?;
            serde_json::from_str(&text).map_err(|e| e.to_string())
        }
        other => Err(format!("a {} cannot be saved", other.kind())),
    }
}

fn is_persistable(ty: &Type) -> bool {
    !matches!(ty, Type::Unit | Type::Entity | Type::Component(_))
}

impl ScriptRuntime {
    /// Snapshot an instance's persistable variables.
    pub fn save_state(&self, object_id: &str) -> Result<SavedState, RuntimeError> {
        let instance =
            self.instances.get(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        let program = &self
            .classes
            .get(&instance.class)
            .ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?
            .program;
        let module = program.module();
        let mut variables = Vec::new();
        for (index, variable) in module.variables.iter().enumerate() {
            if !is_persistable(&variable.ty) {
                continue;
            }
            let value = program.var(&instance.state, index).expect("instances hold every variable");
            let value = value_to_json(value)
                .map_err(|reason| RuntimeError::BadVariable { name: variable.name.clone(), reason })?;
            variables.push(SavedVariable { id: variable.id.clone(), name: variable.name.clone(), ty: variable.ty.clone(), value });
        }
        Ok(SavedState { class: instance.class.clone(), class_version: module.class_version, variables })
    }

    /// Load `saved` into an instance of the same class, migrating it to the
    /// class's current version. Conflicting identities in `saved` (a
    /// repeated id or name) are refused rather than guessed at.
    pub fn restore_state(&mut self, object_id: &str, saved: &SavedState) -> Result<RestoreReport, RuntimeError> {
        let bad = |reason: String| RuntimeError::State { object_id: object_id.to_owned(), reason };
        let instance =
            self.instances.get(object_id).ok_or_else(|| RuntimeError::UnknownInstance(object_id.to_owned()))?;
        if instance.class != saved.class {
            return Err(bad(format!("the save is of class `{}`, the instance is `{}`", saved.class, instance.class)));
        }
        let class = self.classes.get(&instance.class).ok_or_else(|| RuntimeError::UnknownClass(instance.class.clone()))?;

        let (mut names, mut ids) = (std::collections::HashSet::new(), std::collections::HashSet::new());
        for variable in &saved.variables {
            if !names.insert(variable.name.as_str()) {
                return Err(bad(format!("the save has `{}` twice", variable.name)));
            }
            if let Some(id) = &variable.id {
                if id.is_empty() || !ids.insert(id.as_str()) {
                    return Err(bad(format!("the save has an empty or repeated variable id (`{}`)", variable.name)));
                }
            }
        }

        let mut report = RestoreReport::default();
        let old_vars: Vec<Variable> = saved
            .variables
            .iter()
            .map(|v| Variable { name: v.name.clone(), ty: v.ty.clone(), default: None, id: v.id.clone() })
            .collect();
        let old_values: Vec<Option<Value>> = saved
            .variables
            .iter()
            .map(|v| match value_from_json(&v.value, &v.ty) {
                Ok(value) => Some(value),
                Err(reason) => {
                    report.unreadable.push((v.name.clone(), reason));
                    None
                }
            })
            .collect();

        let mut carried = carry_state(class, object_id, &old_vars, saved.class_version, &old_values, &mut World::new())?;
        // Handles are not saved: they keep what the host bound.
        let program = &class.program;
        for (index, variable) in program.module().variables.iter().enumerate() {
            if !is_persistable(&variable.ty) {
                if let Some(current) = program.var(&instance.state, index) {
                    let _ = program.set_var(&mut carried.state, index, current.clone());
                }
            }
        }
        // A variable the save simply lacks is "new", but one that is a
        // handle is not interesting to report.
        carried.changes.retain(|c| {
            !(matches!(c.kind, ChangeKind::Defaulted)
                && program
                    .variable(&c.variable)
                    .is_some_and(|i| !is_persistable(&program.module().variables[i].ty)))
        });
        report.variables_kept = carried.kept;
        report.changes = carried.changes;
        self.instances.get_mut(object_id).expect("checked above").state = carried.state;
        Ok(report)
    }
}
