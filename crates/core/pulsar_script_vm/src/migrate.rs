//! Matching a class's state across versions.
//!
//! When a class changes (hot reload, a level's saved overrides, saved
//! variable state), each variable of the new version is matched to a
//! variable of the old one so its value can carry over.
//!
//! # Matching
//!
//! Identity is the variable's stable [`id`](Variable::id) when both sides
//! have one; the frontend keeps it across renames. Names only decide when
//! an id is missing on either side (data from before ids existed):
//!
//! - both ids present: they must be equal. A new variable that reuses an
//!   old variable's *name* but has a different id is a different variable
//!   and does **not** inherit its value;
//! - an id missing on either side: the names must be equal.
//!
//! A matched pair of the same [`Type`] is [`Kept`](VariableFate::Kept). A
//! matched pair whose types differ is [`Retyped`](VariableFate::Retyped):
//! the new variable starts at its default (a value is never silently
//! reinterpreted), and the old value stays readable by the class's
//! `migrate` function. Everything else is [`Added`](VariableFate::Added);
//! old variables nothing matched are removed.
//!
//! # The `migrate` function
//!
//! A class may export `migrate(from_version: int) -> unit`. After a reload
//! that raises [`Module::class_version`], it runs once per instance on the
//! new state, so it can fill retyped or added variables from the old
//! values through the `migration::old_*` natives. It is bounded: it runs on
//! an empty world with a small step budget, and may not suspend, so it
//! cannot touch the scene or leave work behind. If it fails, the whole
//! reload is refused and the old class keeps running.

use crate::error::ScriptError;
use crate::module::Variable;
use crate::native::{Host, NativeFn, NativeRegistry};
use crate::value::Value;

/// The exported function a class may provide to migrate its state.
pub const MIGRATE_FUNCTION: &str = "migrate";

/// Step budget of one `migrate` call.
pub const MIGRATE_BUDGET: u64 = 100_000;

/// What happened to one variable of the new version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariableFate {
    /// The value carries over from `old` (an index into the old variables).
    /// `renamed`: the name changed but the id matched.
    Kept { old: usize, renamed: bool },
    /// Matched `old`, but the type changed: starts at its default.
    Retyped { old: usize },
    /// No old counterpart: starts at its default.
    Added,
}

/// How to build a new instance's state from an old one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationPlan {
    /// One entry per new variable, in order.
    pub fates: Vec<VariableFate>,
    /// Old variables nothing matched (their values are discarded).
    pub removed: Vec<usize>,
}

/// Match `new` variables to `old` ones. See the module docs for the rules.
pub fn plan(old: &[Variable], new: &[Variable]) -> MigrationPlan {
    let mut taken = vec![false; old.len()];
    let fates = new
        .iter()
        .map(|variable| {
            let found = old.iter().enumerate().find(|(i, candidate)| !taken[*i] && same_variable(candidate, variable));
            match found {
                Some((i, candidate)) => {
                    taken[i] = true;
                    if candidate.ty == variable.ty {
                        VariableFate::Kept { old: i, renamed: candidate.name != variable.name }
                    } else {
                        VariableFate::Retyped { old: i }
                    }
                }
                None => VariableFate::Added,
            }
        })
        .collect();
    let removed = (0..old.len()).filter(|i| !taken[*i]).collect();
    MigrationPlan { fates, removed }
}

fn same_variable(old: &Variable, new: &Variable) -> bool {
    match (&old.id, &new.id) {
        (Some(a), Some(b)) => a == b,
        _ => old.name == new.name,
    }
}

/// The variable a saved or authored `key` (an id or a name) refers to.
/// `Err` when it is ambiguous (the id of one variable is the name of
/// another) or unknown.
pub fn resolve_key(variables: &[Variable], key: &str) -> Result<usize, String> {
    let by_id = variables.iter().position(|v| v.id.as_deref() == Some(key));
    let by_name = variables.iter().position(|v| v.name == key);
    match (by_id, by_name) {
        (Some(a), Some(b)) if a != b => Err(format!("`{key}` is both the id of `{}` and the name of `{}`", variables[a].name, variables[b].name)),
        (Some(i), _) | (None, Some(i)) => Ok(i),
        (None, None) => Err(format!("no variable `{key}`")),
    }
}

/// Old values a `migrate` call may read, by variable name.
pub trait MigrationSource: Send + Sync {
    fn old_value(&self, name: &str) -> Option<&Value>;
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    let mut add = |native: NativeFn| {
        if let Err(err) = registry.register(native) {
            tracing::error!("script migration natives: {err}");
        }
    };
    macro_rules! reader {
        ($name:literal, $ty:ty, $pick:ident) => {
            add(NativeFn::builder($name)
                .pure()
                .attr("category", "Migration")
                .params(["name"])
                .build(|host: &mut Host<'_>, name: std::sync::Arc<str>| -> Result<$ty, ScriptError> {
                    let source = host
                        .migration
                        .ok_or_else(|| ScriptError::native("old values are only readable inside `migrate`"))?;
                    let value = source
                        .old_value(&name)
                        .ok_or_else(|| ScriptError::native(format!("the old class had no variable `{name}`")))?;
                    value.$pick().map(Into::into).ok_or_else(|| {
                        ScriptError::native(format!("old `{name}` is a {}, not the requested type", value.kind()))
                    })
                }));
        };
    }
    reader!("migration::old_bool", bool, as_bool);
    reader!("migration::old_int", i64, as_int);
    reader!("migration::old_float", f64, as_float);
    reader!("migration::old_string", String, as_str);
    add(NativeFn::builder("migration::has_old")
        .pure()
        .attr("category", "Migration")
        .params(["name"])
        .build(|host: &mut Host<'_>, name: std::sync::Arc<str>| {
            host.migration.is_some_and(|s| s.old_value(&name).is_some())
        }));
}
