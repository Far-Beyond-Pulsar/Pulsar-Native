//! What a class reload and a native relink did, and carrying an instance's variables across versions.

use std::collections::HashMap;

use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::migrate::{self, MigrationSource, VariableFate};
use pulsar_script_vm::{Budget, FuncId, Host, Instance, LinkError, Type, Value, Variable, Vm};

use crate::class::Class;
use crate::RuntimeError;

/// What a class reload did with the instances' waiting calls.
#[derive(Debug, Default)]
pub struct ReloadReport {
    /// Waiting calls that continue in the new code.
    pub kept: usize,
    /// Waiting calls dropped because their code changed shape.
    pub dropped: Vec<DroppedCall>,
    /// Variables, across all instances, whose value carried over unchanged.
    pub variables_kept: usize,
    /// Everything else that happened to variables: renames, resets,
    /// retypes, removals and `migrate` runs, one entry per instance.
    pub variables: Vec<VariableChange>,
}

/// One notable change to one instance's variable during a reload.
#[derive(Clone, Debug, PartialEq)]
pub struct VariableChange {
    pub object_id: String,
    /// The variable (for `Removed`, the old name).
    pub variable: String,
    pub kind: ChangeKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChangeKind {
    /// The value carried over under a new name (matched by id).
    Renamed { from: String },
    /// New in this version: starts at its default.
    Defaulted,
    /// The type changed: the value was not carried over and the variable
    /// starts at its default (unless `migrate` set it).
    Incompatible { from: Type, to: Type },
    /// Gone from this version: its value was discarded.
    Removed,
    /// The class's `migrate` function ran for this instance.
    MigrateRan { from_version: u32 },
}

/// The old instance's values, by variable name, for `migrate`.
struct OldState(HashMap<String, Value>);

impl MigrationSource for OldState {
    fn old_value(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }
}

/// The `migrate` function to run for a reload from `old` to `new`, if any:
/// the new module raised its class version and exports one.
fn migration_hook(old_version: u32, new: &Class) -> Result<Option<FuncId>, RuntimeError> {
    let module = new.program.module();
    if module.class_version <= old_version {
        return Ok(None);
    }
    match module.function(migrate::MIGRATE_FUNCTION) {
        Some((index, f)) if f.exported => {
            if f.params == [Type::Int] && f.ret == Type::Unit {
                Ok(Some(FuncId(index)))
            } else {
                Err(RuntimeError::BadEntryPoint {
                    class: module.name.clone(),
                    name: migrate::MIGRATE_FUNCTION,
                    expected: "fn migrate(from_version: int)",
                })
            }
        }
        _ => Ok(None),
    }
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

/// A new instance's state, carried over from an old version's values.
pub(crate) struct Carried {
    pub(crate) state: Instance,
    pub(crate) changes: Vec<VariableChange>,
    /// Variables whose value carried over unchanged.
    pub(crate) kept: usize,
}

/// Build the state of a `new` instance from `old_values` (parallel to
/// `old_vars`): match variables with [`migrate::plan`], carry matches, then
/// run the class's `migrate` function if `old_version` is older. Shared by
/// hot reload and restoring saved state, so both migrate the same way.
pub(crate) fn carry_state(
    new: &Class,
    object_id: &str,
    old_vars: &[Variable],
    old_version: u32,
    old_values: &[Option<Value>],
    scratch: &mut World,
) -> Result<Carried, RuntimeError> {
    let module = new.program.module();
    let plan = migrate::plan(old_vars, &module.variables);
    let mut state = new.program.instantiate();
    let mut changes = Vec::new();
    let mut kept = 0;
    for (index, fate) in plan.fates.iter().enumerate() {
        let variable = &module.variables[index];
        let change = |kind| VariableChange {
            object_id: object_id.to_owned(),
            variable: variable.name.clone(),
            kind,
        };
        match *fate {
            VariableFate::Kept { old, renamed } => {
                let value = old_values.get(old).cloned().flatten();
                match value.map(|v| new.program.set_var(&mut state, index, v)) {
                    Some(Ok(())) if renamed => changes.push(change(ChangeKind::Renamed {
                        from: old_vars[old].name.clone(),
                    })),
                    Some(Ok(())) => kept += 1,
                    _ => changes.push(change(ChangeKind::Defaulted)),
                }
            }
            VariableFate::Retyped { old } => changes.push(change(ChangeKind::Incompatible {
                from: old_vars[old].ty.clone(),
                to: variable.ty.clone(),
            })),
            VariableFate::Added => changes.push(change(ChangeKind::Defaulted)),
        }
    }
    for &removed in &plan.removed {
        changes.push(VariableChange {
            object_id: object_id.to_owned(),
            variable: old_vars[removed].name.clone(),
            kind: ChangeKind::Removed,
        });
    }
    if let Some(hook) = migration_hook(old_version, new)? {
        let source = OldState(
            old_vars
                .iter()
                .zip(old_values)
                .filter_map(|(v, value)| Some((v.name.clone(), value.clone()?)))
                .collect(),
        );
        let mut host = Host::new(scratch, Entity::DANGLING).with_migration(Some(&source));
        Vm::new()
            .call(
                &new.program,
                &mut state,
                hook,
                &[Value::Int(i64::from(old_version))],
                &mut host,
                &mut Budget::new(migrate::MIGRATE_BUDGET),
            )
            .map_err(|source| RuntimeError::Migration {
                class: module.name.clone(),
                object_id: object_id.to_owned(),
                source,
            })?;
        changes.push(VariableChange {
            object_id: object_id.to_owned(),
            variable: migrate::MIGRATE_FUNCTION.to_owned(),
            kind: ChangeKind::MigrateRan {
                from_version: old_version,
            },
        });
    }
    Ok(Carried {
        state,
        changes,
        kept,
    })
}
