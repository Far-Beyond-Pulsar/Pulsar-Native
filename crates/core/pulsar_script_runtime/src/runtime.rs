//! `ScriptRuntime`: the native registry, loaded classes and live instances.
//!
//! The `impl` is split by concern in the child modules.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use pulsar_script_vm::{CapabilityPolicy, DebugSnapshot, NativeLibraries, NativeRegistry, Vm};

use crate::class::{Class, ScriptInstance};
use crate::error::RuntimeError;
use crate::event_host::EventHost;
use crate::limits::{ScriptLimits, DEFAULT_BUDGET};
use crate::phase::{PhaseStats, DEFAULT_PARALLEL_THRESHOLD};
use crate::reload::RelinkReport;

mod classes;
mod debugging;
mod dispatch;
mod events;
mod instances;
mod natives;

pub struct ScriptRuntime {
    pub(crate) natives: NativeRegistry,
    pub(crate) libraries: NativeLibraries,
    pub(crate) vm: Vm,
    pub(crate) classes: HashMap<String, Class>,
    pub(crate) instances: HashMap<String, ScriptInstance>,
    /// Instance ids in spawn order: events dispatch in this order.
    pub(crate) order: Vec<String>,
    pub(crate) pending_begin_play: Vec<String>,
    /// Game time in seconds: the sum of every `tick_all` delta.
    pub(crate) time: f64,
    /// Step budget for one event on one instance (see [`ScriptLimits`]).
    pub budget: u64,
    /// Per-class step budgets, by class name.
    pub(crate) class_budgets: HashMap<String, u64>,
    /// Which native capabilities classes may import.
    pub(crate) capabilities: CapabilityPolicy,
    pub(crate) events: Option<Arc<dyn EventHost>>,
    pub(crate) debug_events: Vec<(String, DebugSnapshot)>,
    /// Errors from calls resumed outside a tick (by an event), reported by
    /// the next `tick_all`.
    pub(crate) deferred_errors: Vec<RuntimeError>,
    /// What the last tick's read and write stages did.
    pub(crate) phase_stats: PhaseStats,
    /// Read-only instances needed before the read stage uses threads.
    pub(crate) parallel_threshold: usize,
}

/// A snapshot of execution counters for one live script instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceRuntimeStats {
    /// Runtime object id (the driver uses a stable entity/class identity).
    pub object_id: String,
    /// Loaded script class name.
    pub class: String,
    /// Lifetime instructions executed since this instance spawned.
    pub instructions_executed: u64,
    /// Calls currently suspended on `Wait`.
    pub waiting_calls: usize,
    /// Calls stopped at a debugger breakpoint or step boundary.
    pub paused_calls: usize,
}

impl ScriptRuntime {
    /// A runtime with the engine's natives. Native library shadow copies go
    /// in `shadow_dir`.
    pub fn new(shadow_dir: impl Into<PathBuf>) -> Self {
        Self::with_natives(NativeRegistry::with_engine_natives(), shadow_dir)
    }

    pub fn with_natives(natives: NativeRegistry, shadow_dir: impl Into<PathBuf>) -> Self {
        Self {
            natives,
            libraries: NativeLibraries::new(shadow_dir),
            vm: Vm::new(),
            classes: HashMap::new(),
            instances: HashMap::new(),
            order: Vec::new(),
            pending_begin_play: Vec::new(),
            time: 0.0,
            budget: DEFAULT_BUDGET,
            class_budgets: HashMap::new(),
            capabilities: CapabilityPolicy::allow_all(),
            events: None,
            debug_events: Vec::new(),
            deferred_errors: Vec::new(),
            phase_stats: PhaseStats::default(),
            parallel_threshold: DEFAULT_PARALLEL_THRESHOLD,
        }
    }

    /// Apply execution limits (budget, call depth, checked arithmetic,
    /// per-class budgets). Takes effect from the next call.
    pub fn set_limits(&mut self, limits: ScriptLimits) {
        self.budget = limits.instruction_budget;
        self.vm.max_depth = limits.max_call_depth;
        self.vm.checked_arithmetic = limits.checked_arithmetic;
        self.class_budgets = limits.class_budgets;
    }

    /// The current execution limits.
    pub fn limits(&self) -> ScriptLimits {
        ScriptLimits {
            instruction_budget: self.budget,
            max_call_depth: self.vm.max_depth,
            checked_arithmetic: self.vm.checked_arithmetic,
            class_budgets: self.class_budgets.clone(),
        }
    }

    /// Set which native capabilities classes may import, then
    /// relink every class: one importing a native outside the policy fails
    /// to link ([`LinkError::CapabilityDenied`](pulsar_script_vm::LinkError::CapabilityDenied)).
    pub fn set_capabilities(&mut self, policy: CapabilityPolicy) -> RelinkReport {
        self.capabilities = policy;
        self.relink_all()
    }

    pub fn capabilities(&self) -> &CapabilityPolicy {
        &self.capabilities
    }

    /// Game time in seconds (advanced by [`tick_all`](Self::tick_all)).
    pub fn time(&self) -> f64 {
        self.time
    }

    /// Number of suspended calls on an instance.
    pub fn waiting_calls(&self, object_id: &str) -> usize {
        self.instances
            .get(object_id)
            .map_or(0, |i| i.waiting.len() + i.blocked.len())
    }

    /// Runtime counters for live script instances, in spawn order.
    pub fn instance_stats(&self) -> Vec<InstanceRuntimeStats> {
        self.order
            .iter()
            .filter_map(|id| {
                let instance = self.instances.get(id)?;
                Some(InstanceRuntimeStats {
                    object_id: id.clone(),
                    class: instance.class.clone(),
                    instructions_executed: instance.instructions_executed,
                    waiting_calls: instance.waiting.len() + instance.blocked.len(),
                    paused_calls: instance.paused.len(),
                })
            })
            .collect()
    }

    /// The natives scripts can link against (for frontends' palettes).
    pub fn natives(&self) -> &NativeRegistry {
        &self.natives
    }
}
