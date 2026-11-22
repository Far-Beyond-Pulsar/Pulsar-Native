//! The editor's task system: every piece of editor work that is not a single
//! GPUI callback runs here, so it can be listed, logged, audited and stopped.
//! Game/runtime work must never be submitted here; it has its own lifecycle
//! and scheduling requirements. See `.agents/EDITOR_TASKS.md`.
//!
//! # Ways to run work
//!
//! | Entry point | For | Where it runs |
//! |---|---|---|
//! | [`TaskQueue::submit_async`] | I/O and waiting work | a pool worker, as a future |
//! | [`TaskQueue::submit`] | blocking work (builds, imports) | a pool worker |
//! | [`tracked`] | a future an executor already owns (`cx.spawn`) | that executor |
//! | [`scope`] | work that must run inline (building a tab, first-frame GPU init) | the calling thread |
//!
//! Every one of them creates a task record, runs the work inside the task's
//! tracing span (so `tracing` output is captured per task, see [`log`]),
//! measures how it is being polled, and ends in a terminal status. The
//! watchdog turns "no longer making progress" into a [`Stall`] with a reason
//! and a line in the task's log.
//!
//! Short jobs share a small reserved worker pool. Long jobs use a separate
//! pool so sustained work (builds, thumbnail generation, imports) cannot
//! starve short editor interactions. Starring a queued job raises its
//! scheduling priority within its pool.

mod blocking;
pub mod helio;
mod log;
mod micro;
pub mod pools;
mod services;
pub mod threads;
mod track;
mod ui_watch;

pub use blocking::{allow_blocking, blocking_call, blocking_snapshots, BlockingSnapshot};
pub use log::{LogRead, TaskLog, TaskLogLayer, MAX_LOG_LINES};
pub use micro::{micro, micro_snapshots, MicroSnapshot};
pub use services::{spawn_service, PersistentTask, ServiceDescription, ServiceHandle};
pub use threads::{
    current_thread, on_ui_thread, register_current_thread, service_spawner, spawn_named,
    spawn_thread, thread_snapshots, ThreadGuard, ThreadHandle, ThreadRole, ThreadSnapshot,
    ThreadSpec, ThreadState,
};
pub use track::{scope, tracked, TaskOutcome, Tracked};
pub use ui_watch::{
    record_slow_ui_work, set_ui_activity_source, ui_beat, BEAT_EVERY as UI_BEAT_EVERY,
    UNRESPONSIVE_AFTER as UI_UNRESPONSIVE_AFTER,
};

/// Task categories that views list on their own (the Tasks window's
/// Renderers view). The owner of each kind of task uses the const.
pub mod categories {
    /// The engine's render runtimes (`engine_backend::services::render_runtimes`).
    pub const RENDER_RUNTIME: &str = "render.runtime";
}

mod context;
mod queue;
mod shared;
mod state;
mod types;
mod workers;

pub use context::{current, TaskContext};
pub use queue::{global, TaskQueue, WorkerCounts};
pub(crate) use shared::{now_ms, Outcome};
pub use types::{
    Stall, StallKind, StallThresholds, TaskActivity, TaskDescription, TaskDuration, TaskId,
    TaskModel, TaskSnapshot, TaskStatus,
};

#[cfg(test)]
mod tests;
