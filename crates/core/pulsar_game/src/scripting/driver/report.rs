//! What a frame or reconcile did: [`DriverReport`] and the scene-lock
//! timings ([`LockTimes`]) a shared-world frame adds to it.

use pulsar_scenedb::Entity;
use pulsar_script_runtime::RuntimeError;

/// What a frame cost in scene-lock time. The write lock is what blocks the
/// renderer's snapshot and readback; the read lock shares with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockTimes {
    /// Time spent waiting to take the write lock (both acquisitions).
    pub write_wait: std::time::Duration,
    /// Time the write lock was held (both acquisitions).
    pub write_hold: std::time::Duration,
    pub read_wait: std::time::Duration,
    pub read_hold: std::time::Duration,
    /// What the script runtime's stages did.
    pub phase: pulsar_script_runtime::PhaseStats,
}

/// What one reconcile or frame did.
#[derive(Debug, Default)]
pub struct DriverReport {
    /// Instance ids started, in `begin_play` order.
    pub started: Vec<String>,
    /// Instance ids stopped (their `end_play` ran if they had begun).
    pub stopped: Vec<String>,
    /// Entities built by `world::spawn` / `world::spawn_child`.
    pub spawned: Vec<Entity>,
    /// Entities removed by `world::destroy`.
    pub destroyed: Vec<Entity>,
    /// Errors raised by scripts (per instance, never fatal).
    pub script_errors: Vec<RuntimeError>,
    /// Class modules that did not load or reload (link errors, unreadable
    /// modules), per class.
    pub load_errors: Vec<RuntimeError>,
    /// Waiting calls class reloads had to drop, as `(class, call)`.
    pub dropped_calls: Vec<(String, pulsar_script_runtime::DroppedCall)>,
    /// Instances or commands that could not be applied, as messages.
    pub failures: Vec<String>,
    /// How long the scene lock was waited for and held, when the frame ran
    /// through [`run_frame_shared`](ScriptDriver::run_frame_shared).
    pub locks: Option<LockTimes>,
}

impl DriverReport {
    /// Whether the frame raised any error or warning worth reporting.
    pub fn has_problems(&self) -> bool {
        !self.script_errors.is_empty()
            || !self.load_errors.is_empty()
            || !self.dropped_calls.is_empty()
    }
}
