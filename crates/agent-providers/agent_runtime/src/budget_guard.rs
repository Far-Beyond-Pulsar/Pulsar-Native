//! Depth and fan-out limits for sub-agent runs.
//!
//! The spawn path asks [`BudgetGuard::try_spawn`] before it creates a child
//! run. The guard is shared (cheap to clone) by every run in one tree, so the
//! limits hold across the whole tree rather than per parent.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
};

/// Limits for one run tree. The root run has depth 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunBudget {
    /// Deepest allowed nesting. `1` lets the root spawn children that cannot
    /// spawn further.
    pub max_depth: u32,
    /// Children one run may have alive at the same time.
    pub max_children: u32,
    /// Runs in the whole tree over its lifetime, root included.
    pub max_total_runs: u32,
}

impl Default for RunBudget {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_children: 4,
            max_total_runs: 16,
        }
    }
}

/// Why a spawn was refused. `Display` is written for the model to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetError {
    DepthExceeded { depth: u32, max_depth: u32 },
    FanOutExceeded { parent: String, max_children: u32 },
    TotalRunsExceeded { max_total_runs: u32 },
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BudgetError::DepthExceeded { depth, max_depth } => write!(
                f,
                "Cannot spawn a sub-agent: nesting depth {depth} would exceed the limit of {max_depth}. Do the work yourself."
            ),
            BudgetError::FanOutExceeded {
                parent,
                max_children,
            } => write!(
                f,
                "Cannot spawn a sub-agent: run '{parent}' already has {max_children} active children. Wait for one to finish."
            ),
            BudgetError::TotalRunsExceeded { max_total_runs } => write!(
                f,
                "Cannot spawn a sub-agent: this task already used its budget of {max_total_runs} runs."
            ),
        }
    }
}

impl std::error::Error for BudgetError {}

#[derive(Debug)]
struct State {
    budget: RunBudget,
    total_runs: u32,
    live_children: HashMap<String, u32>,
}

/// Shared accounting for one run tree.
#[derive(Clone, Debug)]
pub struct BudgetGuard(Arc<Mutex<State>>);

impl BudgetGuard {
    /// The root run is counted against `max_total_runs` immediately.
    pub fn new(budget: RunBudget) -> Self {
        Self(Arc::new(Mutex::new(State {
            budget,
            total_runs: 1,
            live_children: HashMap::new(),
        })))
    }

    pub fn budget(&self) -> RunBudget {
        self.lock().budget
    }

    /// Runs created so far, root included.
    pub fn total_runs(&self) -> u32 {
        self.lock().total_runs
    }

    /// Live children of `parent_run_id`.
    pub fn live_children(&self, parent_run_id: &str) -> u32 {
        self.lock()
            .live_children
            .get(parent_run_id)
            .copied()
            .unwrap_or(0)
    }

    /// Reserve a child slot under `parent_run_id`, which runs at
    /// `parent_depth`. On success the returned permit holds the slot until it
    /// is dropped: move it into the child run so a finished child frees its
    /// fan-out slot (the lifetime total stays spent).
    pub fn try_spawn(
        &self,
        parent_run_id: &str,
        parent_depth: u32,
    ) -> Result<SpawnPermit, BudgetError> {
        let mut state = self.lock();
        let child_depth = parent_depth + 1;
        if child_depth > state.budget.max_depth {
            return Err(BudgetError::DepthExceeded {
                depth: child_depth,
                max_depth: state.budget.max_depth,
            });
        }
        let live = state.live_children.get(parent_run_id).copied().unwrap_or(0);
        if live >= state.budget.max_children {
            return Err(BudgetError::FanOutExceeded {
                parent: parent_run_id.to_string(),
                max_children: state.budget.max_children,
            });
        }
        if state.total_runs >= state.budget.max_total_runs {
            return Err(BudgetError::TotalRunsExceeded {
                max_total_runs: state.budget.max_total_runs,
            });
        }
        state.total_runs += 1;
        *state
            .live_children
            .entry(parent_run_id.to_string())
            .or_insert(0) += 1;
        Ok(SpawnPermit {
            guard: self.clone(),
            parent: parent_run_id.to_string(),
            child_depth,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A reserved child slot. Dropping it frees the parent's fan-out slot.
#[derive(Debug)]
pub struct SpawnPermit {
    guard: BudgetGuard,
    parent: String,
    child_depth: u32,
}

impl SpawnPermit {
    /// Depth the child runs at; pass it to the child's own `try_spawn`.
    pub fn child_depth(&self) -> u32 {
        self.child_depth
    }
}

impl Drop for SpawnPermit {
    fn drop(&mut self) {
        let mut state = self.guard.lock();
        if let Some(live) = state.live_children.get_mut(&self.parent) {
            *live = live.saturating_sub(1);
            if *live == 0 {
                state.live_children.remove(&self.parent);
            }
        }
    }
}
