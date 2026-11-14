//! The build session: what a running (or the last) build publishes for the
//! build dashboard to show. One per build; [`current`] is the latest.
//!
//! The runner owns the writes: the step title, progress and status line
//! (the same values its notification shows), the editor task the build runs
//! in (whose log holds the cargo output), the process id of the cargo
//! command running now (the root of the process tree the dashboard samples),
//! and how the build ended.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use editor_task_queue::TaskId;
use parking_lot::{Mutex, RwLock};

/// How a build ended, or that it has not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

pub struct BuildSession {
    /// The build configuration's name.
    pub name: String,
    pub started: Instant,
    task: Mutex<Option<TaskId>>,
    /// Overall progress, 0–100 (shared with the runner's `Progress`).
    pub pct: Arc<AtomicU32>,
    /// The current step's title, as "Building (2/3)".
    pub step: Arc<Mutex<String>>,
    /// The status line under the step (shared with the runner's `Progress`).
    pub detail: Arc<Mutex<String>>,
    /// The running cargo command's process id; 0 between commands.
    pub cargo_pid: Arc<AtomicU32>,
    outcome: Mutex<(Outcome, Option<Instant>)>,
}

impl BuildSession {
    pub(crate) fn new(
        name: String,
        pct: Arc<AtomicU32>,
        step: Arc<Mutex<String>>,
        detail: Arc<Mutex<String>>,
        cargo_pid: Arc<AtomicU32>,
    ) -> Self {
        Self {
            name,
            started: Instant::now(),
            task: Mutex::new(None),
            pct,
            step,
            detail,
            cargo_pid,
            outcome: Mutex::new((Outcome::Running, None)),
        }
    }

    /// The editor task the build runs in: its log is the build's output.
    pub fn task(&self) -> Option<TaskId> {
        *self.task.lock()
    }

    pub(crate) fn set_task(&self, task: TaskId) {
        *self.task.lock() = Some(task);
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome.lock().0
    }

    pub(crate) fn finish(&self, outcome: Outcome) {
        *self.outcome.lock() = (outcome, Some(Instant::now()));
        self.cargo_pid.store(0, Ordering::Release);
    }

    /// Time from start to the end of the build, or to now while it runs.
    pub fn elapsed(&self) -> Duration {
        let ended = self.outcome.lock().1;
        ended.unwrap_or_else(Instant::now) - self.started
    }

    /// The running cargo command's process id.
    pub fn cargo_pid(&self) -> Option<u32> {
        match self.cargo_pid.load(Ordering::Acquire) {
            0 => None,
            pid => Some(pid),
        }
    }
}

fn slot() -> &'static RwLock<Option<Arc<BuildSession>>> {
    static CURRENT: RwLock<Option<Arc<BuildSession>>> = RwLock::new(None);
    &CURRENT
}

/// The latest build's session (running or finished), if any build ran.
pub fn current() -> Option<Arc<BuildSession>> {
    slot().read().clone()
}

pub(crate) fn begin(session: Arc<BuildSession>) {
    *slot().write() = Some(session);
}
