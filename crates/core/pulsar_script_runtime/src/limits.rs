//! Execution limits of a runtime: step budget, call depth, arithmetic checks.

use std::collections::HashMap;

/// Default step budget for one event on one instance.
pub const DEFAULT_BUDGET: u64 = 1_000_000;

/// Execution limits of a runtime: what stops a runaway
/// script, and whether integer overflow is an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptLimits {
    /// Instructions one event on one instance may run
    /// ([`ScriptErrorKind::BudgetExceeded`](pulsar_script_vm::ScriptErrorKind::BudgetExceeded)
    /// past it).
    pub instruction_budget: u64,
    /// Script call depth ([`ScriptErrorKind::StackOverflow`](pulsar_script_vm::ScriptErrorKind::StackOverflow)
    /// past it).
    pub max_call_depth: usize,
    /// Integer overflow is a runtime error instead of wrapping.
    pub checked_arithmetic: bool,
    /// Per-class instruction budgets, by class (module) name, replacing
    /// `instruction_budget` for that class.
    pub class_budgets: HashMap<String, u64>,
}

impl ScriptLimits {
    /// Editor and Play-in-Editor: generous limits, checked arithmetic on.
    pub fn development() -> Self {
        Self {
            instruction_budget: DEFAULT_BUDGET,
            max_call_depth: pulsar_script_vm::DEFAULT_MAX_DEPTH,
            checked_arithmetic: true,
            class_budgets: HashMap::new(),
        }
    }

    /// Shipping builds: strict limits, checked arithmetic off.
    pub fn shipping() -> Self {
        Self {
            instruction_budget: 100_000,
            max_call_depth: 64,
            checked_arithmetic: false,
            class_budgets: HashMap::new(),
        }
    }
}

impl Default for ScriptLimits {
    /// The VM's own defaults: [`DEFAULT_BUDGET`], the VM's depth limit,
    /// wrapping arithmetic.
    fn default() -> Self {
        Self {
            instruction_budget: DEFAULT_BUDGET,
            max_call_depth: pulsar_script_vm::DEFAULT_MAX_DEPTH,
            checked_arithmetic: false,
            class_budgets: HashMap::new(),
        }
    }
}
