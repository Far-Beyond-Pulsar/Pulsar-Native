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

pub mod helio;
mod log;
mod micro;
pub mod pools;
mod services;
pub mod threads;
mod track;

pub use log::{LogRead, TaskLog, TaskLogLayer, MAX_LOG_LINES};
pub use micro::{micro, micro_snapshots, MicroSnapshot};
pub use services::{spawn_service, PersistentTask, ServiceDescription, ServiceHandle};
pub use threads::{
    current_thread, register_current_thread, spawn_named, spawn_thread, thread_snapshots,
    ThreadGuard, ThreadHandle, ThreadRole, ThreadSnapshot, ThreadSpec, ThreadState,
};
pub use track::{scope, tracked, TaskOutcome, Tracked};

/// Task categories that views list on their own (the Tasks window's
/// Renderers view). The owner of each kind of task uses the const.
pub mod categories {
    /// The engine's render runtimes (`engine_backend::services::render_runtimes`).
    pub const RENDER_RUNTIME: &str = "render.runtime";
}

use parking_lot::{Condvar, Mutex};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe, Location};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};
use tracing::Level;

const DEFAULT_ON_DEMAND_WORKERS: usize = 3;
const DEFAULT_DEDICATED_WORKERS: usize = 2;
const MAX_WORKERS_PER_POOL: usize = 16;
/// Finished tasks kept for the audit history before the oldest unstarred ones
/// are forgotten.
const MAX_FINISHED_TASKS: usize = 500;
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(1);

static GLOBAL: OnceLock<TaskQueue> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the first task system use in this process.
pub(crate) fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Process-wide queue used only by the editor.
pub fn global() -> &'static TaskQueue {
    GLOBAL.get_or_init(TaskQueue::new)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(u64);

impl TaskId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskDuration {
    Short,
    Long,
    /// Runs inline on the calling thread (see [`scope`]); not a pool.
    Foreground,
    /// Lives as long as its thread: a render loop, a watcher, a language
    /// server (see [`TaskQueue::spawn_service`]). It has no result; it has a
    /// heartbeat.
    Persistent,
}

/// How a task's work is expected to behave. Stall detection depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskModel {
    /// A future: it must return from every poll quickly and wait with `.await`.
    Async,
    /// A closure that blocks its worker by design (a build, an import).
    Blocking,
    /// Inline on the calling thread, which it blocks until it returns.
    Foreground,
    /// A long-lived thread's loop, judged by its heartbeat.
    Persistent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// What a running task is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskActivity {
    /// Not started yet.
    NotStarted,
    /// Inside a poll (or inline): running code, holding its thread.
    Polling,
    /// Between polls: waiting for something to wake it.
    Pending,
    Finished,
}

impl TaskActivity {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Polling,
            2 => Self::Pending,
            3 => Self::Finished,
            _ => Self::NotStarted,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallKind {
    /// Queued, and no worker has taken it.
    QueueWait,
    /// A tracked future that nobody has polled.
    NeverPolled,
    /// An async task stuck inside one poll: synchronous work in async code.
    BlockedPoll,
    /// Inline work holding its (UI or render) thread.
    BlockedThread,
    /// An async task waiting with no log, progress or phase change.
    Pending,
    /// A blocking task with no log, progress or phase change.
    NoProgress,
    /// A persistent task whose heartbeat stopped.
    NoHeartbeat,
}

impl StallKind {
    fn code(self) -> u8 {
        self as u8 + 1
    }
}

/// Why a task looks stalled or frozen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stall {
    pub kind: StallKind,
    pub message: String,
}

/// How long a task may go without an observable change before it is reported.
#[derive(Clone, Copy, Debug)]
pub struct StallThresholds {
    pub queue_wait: Duration,
    pub never_polled: Duration,
    pub blocked_poll: Duration,
    pub blocked_thread: Duration,
    pub pending_idle: Duration,
    pub blocking_idle: Duration,
}

impl Default for StallThresholds {
    fn default() -> Self {
        Self {
            queue_wait: Duration::from_secs(10),
            never_polled: Duration::from_secs(5),
            blocked_poll: Duration::from_secs(2),
            blocked_thread: Duration::from_secs(1),
            pending_idle: Duration::from_secs(15),
            blocking_idle: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TaskDescription {
    pub title: String,
    pub category: String,
    pub duration: TaskDuration,
}

impl TaskDescription {
    pub fn new(
        title: impl Into<String>,
        category: impl Into<String>,
        duration: TaskDuration,
    ) -> Self {
        Self {
            title: title.into(),
            category: category.into(),
            duration,
        }
    }

    /// A description for a persistent task ([`TaskQueue::spawn_service`]).
    pub fn persistent(title: impl Into<String>, category: impl Into<String>) -> Self {
        Self::new(title, category, TaskDuration::Persistent)
    }

    /// A description for inline work ([`scope`]).
    pub fn foreground(title: impl Into<String>, category: impl Into<String>) -> Self {
        Self::new(title, category, TaskDuration::Foreground)
    }
}

#[derive(Clone, Debug)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub title: String,
    pub category: String,
    pub duration: TaskDuration,
    pub model: TaskModel,
    pub status: TaskStatus,
    pub starred: bool,
    pub progress: Option<f32>,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub submitted_at: SystemTime,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    /// The stage the task last named with [`TaskContext::set_phase`].
    pub phase: Option<String>,
    /// `file:line` of the code that created the task.
    pub origin: String,
    /// The thread running it.
    pub worker: Option<String>,
    /// How many times its future was polled.
    pub polls: u64,
    pub activity: TaskActivity,
    /// How long it has been in `activity`.
    pub activity_for: Duration,
    /// Time since its last log line, progress, or phase change.
    pub idle_for: Duration,
    pub stall: Option<Stall>,
    /// Lines written to its log so far.
    pub log_lines: u64,
    /// Persistent tasks: heartbeats so far.
    pub beats: u64,
    /// Persistent tasks: recent heartbeat rate (frames or loop passes per second).
    pub beat_rate_hz: Option<f32>,
    /// Persistent tasks: how long the last pass took, if it reported it.
    pub last_tick: Option<Duration>,
    /// Persistent tasks: time since the last heartbeat.
    pub since_beat: Option<Duration>,
    /// Persistent tasks: intentionally idle (waiting for work), so silence is fine.
    pub idle: bool,
    /// What the task says about itself, by name, for views that list a kind
    /// of task as a table (the Tasks window's Renderers view): set with
    /// [`TaskContext::set_details`].
    pub details: Vec<(String, String)>,
}

/// What a finished unit of work reported.
pub(crate) enum Outcome {
    Succeeded,
    Failed(String),
    /// The future was dropped before it finished.
    Dropped,
}

/// State shared between a task's record, its context and its future.
pub(crate) struct TaskShared {
    pub(crate) id: TaskId,
    pub(crate) model: TaskModel,
    pub(crate) pooled: bool,
    pub(crate) cancelled: AtomicBool,
    pub(crate) started: AtomicBool,
    pub(crate) log: Arc<log::TaskLog>,
    pub(crate) span: tracing::Span,
    state: AtomicU8,
    state_since: AtomicU64,
    polls: AtomicU64,
    last_activity: AtomicU64,
    reported_stall: AtomicU8,
    beats: AtomicU64,
    last_beat_ms: AtomicU64,
    beat_interval_us: AtomicU64,
    tick_us: AtomicU64,
    idle: AtomicBool,
    beat_timeout_ms: AtomicU64,
}

impl TaskShared {
    pub(crate) fn set_state(&self, activity: TaskActivity) {
        self.state.store(activity as u8, Ordering::Release);
        self.state_since.store(now_ms(), Ordering::Release);
    }

    pub(crate) fn begin_poll(&self) {
        self.polls.fetch_add(1, Ordering::AcqRel);
        self.set_state(TaskActivity::Polling);
    }

    pub(crate) fn touch(&self) {
        self.last_activity.store(now_ms(), Ordering::Release);
    }

    fn activity(&self) -> (TaskActivity, u64) {
        (
            TaskActivity::from_u8(self.state.load(Ordering::Acquire)),
            self.state_since.load(Ordering::Acquire),
        )
    }

    fn last_activity_ms(&self) -> u64 {
        self.last_activity
            .load(Ordering::Acquire)
            .max(self.log.last_line_ms())
    }

    pub(crate) fn beat(&self, tick: Option<Duration>) {
        let now = now_ms();
        let last = self.last_beat_ms.swap(now, Ordering::AcqRel);
        let interval_us = now.saturating_sub(last).saturating_mul(1_000);
        let previous = self.beat_interval_us.load(Ordering::Acquire);
        let smoothed = if previous == 0 {
            interval_us
        } else {
            (previous * 7 + interval_us) / 8
        };
        self.beat_interval_us.store(smoothed, Ordering::Release);
        if let Some(tick) = tick {
            self.tick_us
                .store(tick.as_micros() as u64, Ordering::Release);
        }
        self.beats.fetch_add(1, Ordering::AcqRel);
        self.idle.store(false, Ordering::Release);
        self.touch();
    }

    fn fill_heartbeat(&self, snapshot: &mut TaskSnapshot, now: u64) {
        let beats = self.beats.load(Ordering::Acquire);
        let since = now.saturating_sub(self.last_beat_ms.load(Ordering::Acquire));
        let interval = self.beat_interval_us.load(Ordering::Acquire);
        snapshot.beats = beats;
        snapshot.idle = self.idle.load(Ordering::Acquire);
        snapshot.since_beat = (beats > 0).then(|| Duration::from_millis(since));
        // A rate that is not backed by a recent beat would be stale.
        snapshot.beat_rate_hz =
            (beats > 1 && interval > 0 && since < 3_000).then(|| 1_000_000.0 / interval as f32);
        let tick = self.tick_us.load(Ordering::Acquire);
        snapshot.last_tick = (tick > 0).then(|| Duration::from_micros(tick));
    }

    /// Write a line into the task's log and emit it as a tracing event inside
    /// the task's span (so the file log and Mission Control carry it too).
    pub(crate) fn emit(&self, level: Level, message: &str) {
        self.log
            .push(log::format_line(level, "editor_task", message));
        self.span.in_scope(|| match level {
            Level::ERROR => tracing::error!(task_direct = true, "{}", message),
            Level::WARN => tracing::warn!(task_direct = true, "{}", message),
            Level::INFO => tracing::info!(task_direct = true, "{}", message),
            Level::DEBUG => tracing::debug!(task_direct = true, "{}", message),
            Level::TRACE => tracing::trace!(task_direct = true, "{}", message),
        });
    }
}

type LocalFuture = Pin<Box<dyn Future<Output = Result<(), String>>>>;
type Work = Box<dyn FnOnce(TaskContext) -> LocalFuture + Send + 'static>;

struct QueuedWork {
    id: TaskId,
    duration: TaskDuration,
    shared: Arc<TaskShared>,
    work: Work,
}

struct TaskRecord {
    snapshot: TaskSnapshot,
    shared: Arc<TaskShared>,
}

impl TaskRecord {
    /// The snapshot with its live, computed fields filled in.
    fn view(
        &self,
        now: u64,
        thresholds: &StallThresholds,
        running: (usize, usize),
    ) -> TaskSnapshot {
        let mut snapshot = self.snapshot.clone();
        let (activity, since) = self.shared.activity();
        snapshot.activity = activity;
        snapshot.activity_for = Duration::from_millis(now.saturating_sub(since));
        snapshot.polls = self.shared.polls.load(Ordering::Acquire);
        snapshot.idle_for =
            Duration::from_millis(now.saturating_sub(self.shared.last_activity_ms()));
        snapshot.log_lines = self.shared.log.total();
        if self.shared.model == TaskModel::Persistent {
            self.shared.fill_heartbeat(&mut snapshot, now);
        }
        snapshot.stall = self.diagnose(now, thresholds, running);
        snapshot
    }

    fn diagnose(
        &self,
        now: u64,
        thresholds: &StallThresholds,
        running: (usize, usize),
    ) -> Option<Stall> {
        let snapshot = &self.snapshot;
        let shared = &self.shared;
        let (activity, since) = shared.activity();
        let in_state = Duration::from_millis(now.saturating_sub(since));
        let idle = Duration::from_millis(now.saturating_sub(shared.last_activity_ms()));
        let waited = snapshot.submitted_at.elapsed().unwrap_or_default();
        let stall = |kind, message: String| Some(Stall { kind, message });
        match snapshot.status {
            TaskStatus::Queued if shared.pooled => {
                if waited < thresholds.queue_wait {
                    return None;
                }
                let (pool, count) = match snapshot.duration {
                    TaskDuration::Long => ("dedicated", running.1),
                    _ => ("on-demand", running.0),
                };
                stall(
                    StallKind::QueueWait,
                    format!(
                        "Waiting {:.0}s for a free {pool} worker; {count} task(s) are running in that pool.",
                        waited.as_secs_f32()
                    ),
                )
            }
            TaskStatus::Queued => (waited >= thresholds.never_polled).then(|| Stall {
                kind: StallKind::NeverPolled,
                message: format!(
                    "Created {:.0}s ago but never polled: the future was not handed to an executor, or it is not being awaited.",
                    waited.as_secs_f32()
                ),
            }),
            TaskStatus::Running => match (shared.model, activity) {
                (TaskModel::Foreground, TaskActivity::Polling)
                    if in_state >= thresholds.blocked_thread =>
                {
                    stall(
                        StallKind::BlockedThread,
                        format!(
                            "Running inline on thread \"{}\" for {:.1}s. It holds that thread until it returns.",
                            snapshot.worker.as_deref().unwrap_or("?"),
                            in_state.as_secs_f32()
                        ),
                    )
                }
                (TaskModel::Async, TaskActivity::Polling) if in_state >= thresholds.blocked_poll => {
                    stall(
                        StallKind::BlockedPoll,
                        format!(
                            "Inside one poll for {:.1}s on \"{}\". Synchronous work or a blocking call in async code is holding the worker; move it to `submit` or await a non-blocking API.",
                            in_state.as_secs_f32(),
                            snapshot.worker.as_deref().unwrap_or("?")
                        ),
                    )
                }
                (TaskModel::Async, TaskActivity::Pending) if idle >= thresholds.pending_idle => {
                    stall(
                        StallKind::Pending,
                        format!(
                            "Awaiting for {:.0}s with no log line, progress or phase change (phase: {}). Whatever it awaits has not completed or woken it.",
                            idle.as_secs_f32(),
                            snapshot.phase.as_deref().unwrap_or("none set")
                        ),
                    )
                }
                (TaskModel::Persistent, _) => {
                    let timeout = shared.beat_timeout_ms.load(Ordering::Acquire);
                    let silent = now.saturating_sub(shared.last_beat_ms.load(Ordering::Acquire));
                    if timeout == 0 || shared.idle.load(Ordering::Acquire) || silent < timeout {
                        return None;
                    }
                    stall(
                        StallKind::NoHeartbeat,
                        format!(
                            "No heartbeat for {:.1}s on \"{}\" (expected one every {:.1}s; phase: {}). The loop is blocked, waiting on something that never arrives, or dead.",
                            silent as f32 / 1000.0,
                            snapshot.worker.as_deref().unwrap_or("?"),
                            timeout as f32 / 1000.0,
                            snapshot.phase.as_deref().unwrap_or("none set")
                        ),
                    )
                }
                (TaskModel::Blocking, _) if idle >= thresholds.blocking_idle => stall(
                    StallKind::NoProgress,
                    format!(
                        "No log line, progress or phase change for {:.0}s on \"{}\" (phase: {}).",
                        idle.as_secs_f32(),
                        snapshot.worker.as_deref().unwrap_or("?"),
                        snapshot.phase.as_deref().unwrap_or("none set")
                    ),
                ),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Default)]
struct QueueState {
    records: BTreeMap<TaskId, TaskRecord>,
    pending: Vec<QueuedWork>,
    on_demand_workers: usize,
    dedicated_workers: usize,
}

impl QueueState {
    fn running_per_pool(&self) -> (usize, usize) {
        let mut counts = (0, 0);
        for record in self.records.values() {
            if record.snapshot.status == TaskStatus::Running && record.shared.pooled {
                match record.snapshot.duration {
                    TaskDuration::Short => counts.0 += 1,
                    TaskDuration::Long => counts.1 += 1,
                    TaskDuration::Foreground | TaskDuration::Persistent => {}
                }
            }
        }
        counts
    }
}

struct QueueInner {
    state: Mutex<QueueState>,
    ready: Condvar,
    revision: AtomicU64,
    spawned_workers: Mutex<(usize, usize)>,
    thresholds: Mutex<StallThresholds>,
}

impl QueueInner {
    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// Create a task record, its log and its span.
    fn create(
        self: &Arc<Self>,
        description: &TaskDescription,
        model: TaskModel,
        pooled: bool,
        origin: &Location<'_>,
    ) -> Arc<TaskShared> {
        let id = TaskId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let log = Arc::new(log::TaskLog::new());
        log::register(id, Arc::clone(&log));
        // A root span, at ERROR so a restrictive RUST_LOG still keeps tasks
        // attributable: the span carries identity, not a message.
        let span = tracing::span!(
            parent: None,
            Level::ERROR,
            "editor_task",
            task_id = id.get(),
            title = %description.title,
            category = %description.category,
        );
        let shared = Arc::new(TaskShared {
            id,
            model,
            pooled,
            cancelled: AtomicBool::new(false),
            started: AtomicBool::new(false),
            log,
            span,
            state: AtomicU8::new(TaskActivity::NotStarted as u8),
            state_since: AtomicU64::new(now_ms()),
            polls: AtomicU64::new(0),
            last_activity: AtomicU64::new(now_ms()),
            reported_stall: AtomicU8::new(0),
            beats: AtomicU64::new(0),
            last_beat_ms: AtomicU64::new(now_ms()),
            beat_interval_us: AtomicU64::new(0),
            tick_us: AtomicU64::new(0),
            idle: AtomicBool::new(false),
            beat_timeout_ms: AtomicU64::new(0),
        });
        let snapshot = TaskSnapshot {
            id,
            title: description.title.clone(),
            category: description.category.clone(),
            duration: description.duration,
            model,
            status: TaskStatus::Queued,
            starred: false,
            progress: None,
            detail: None,
            error: None,
            submitted_at: SystemTime::now(),
            started_at: None,
            finished_at: None,
            phase: None,
            origin: format!("{}:{}", origin.file(), origin.line()),
            worker: None,
            polls: 0,
            activity: TaskActivity::NotStarted,
            activity_for: Duration::ZERO,
            idle_for: Duration::ZERO,
            stall: None,
            log_lines: 0,
            beats: 0,
            beat_rate_hz: None,
            last_tick: None,
            since_beat: None,
            idle: false,
            details: Vec::new(),
        };
        let forgotten = {
            let mut state = self.state.lock();
            state.records.insert(
                id,
                TaskRecord {
                    snapshot,
                    shared: Arc::clone(&shared),
                },
            );
            Self::prune(&mut state)
        };
        for id in forgotten {
            log::unregister(id);
        }
        shared.emit(
            Level::INFO,
            &format!(
                "queued: {} [{}] from {}",
                description.title, description.category, origin
            ),
        );
        self.changed();
        shared
    }

    /// Forget the oldest finished, unstarred tasks beyond the history limit.
    fn prune(state: &mut QueueState) -> Vec<TaskId> {
        let finished = state
            .records
            .values()
            .filter(|record| record.snapshot.status.is_finished())
            .count();
        if finished <= MAX_FINISHED_TASKS {
            return Vec::new();
        }
        let drop_ids: Vec<TaskId> = state
            .records
            .values()
            .filter(|record| record.snapshot.status.is_finished() && !record.snapshot.starred)
            .map(|record| record.snapshot.id)
            .take(finished - MAX_FINISHED_TASKS)
            .collect();
        for id in &drop_ids {
            state.records.remove(id);
        }
        drop_ids
    }

    /// Mark the task running on the current thread.
    pub(crate) fn mark_started(&self, shared: &TaskShared) {
        if shared.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        let waited = {
            let mut state = self.state.lock();
            let Some(record) = state.records.get_mut(&shared.id) else {
                return;
            };
            if record.snapshot.status == TaskStatus::Queued {
                record.snapshot.status = TaskStatus::Running;
            }
            record.snapshot.started_at = Some(SystemTime::now());
            record.snapshot.worker = Some(thread.clone());
            record.snapshot.submitted_at.elapsed().unwrap_or_default()
        };
        shared.touch();
        shared.last_beat_ms.store(now_ms(), Ordering::Release);
        self.changed();
        shared.emit(
            Level::INFO,
            &format!(
                "started on {thread} after {:.0}ms in the queue",
                waited.as_secs_f64() * 1000.0
            ),
        );
    }

    /// Move a task to its terminal status. Idempotent: the first call wins.
    pub(crate) fn finish(&self, shared: &TaskShared, outcome: Outcome) {
        let (status, summary, level, duration) = {
            let mut state = self.state.lock();
            let Some(record) = state.records.get_mut(&shared.id) else {
                return;
            };
            if record.snapshot.status.is_finished() {
                return;
            }
            let snapshot = &mut record.snapshot;
            let cancelled = shared.cancelled.load(Ordering::Acquire);
            let (status, summary, level) = if cancelled {
                (TaskStatus::Cancelled, "cancelled".to_string(), Level::WARN)
            } else {
                match outcome {
                    Outcome::Succeeded => {
                        snapshot.progress = Some(1.0);
                        (TaskStatus::Succeeded, "succeeded".to_string(), Level::INFO)
                    }
                    Outcome::Failed(error) => {
                        let summary = format!("failed: {error}");
                        snapshot.error = Some(error);
                        (TaskStatus::Failed, summary, Level::ERROR)
                    }
                    Outcome::Dropped => (
                        TaskStatus::Cancelled,
                        if shared.started.load(Ordering::Acquire) {
                            "dropped before it completed: its future was dropped while pending"
                                .to_string()
                        } else {
                            "dropped without ever being polled".to_string()
                        },
                        Level::WARN,
                    ),
                }
            };
            snapshot.status = status;
            snapshot.finished_at = Some(SystemTime::now());
            let duration = snapshot
                .started_at
                .and_then(|started| started.elapsed().ok())
                .unwrap_or_default();
            (status, summary, level, duration)
        };
        shared.set_state(TaskActivity::Finished);
        shared.touch();
        self.changed();
        shared.emit(
            level,
            &format!(
                "finished ({status:?}): {summary} after {:.0}ms, {} poll(s)",
                duration.as_secs_f64() * 1000.0,
                shared.polls.load(Ordering::Acquire)
            ),
        );
    }

    /// Report newly stalled tasks into their logs, and recoveries.
    fn watchdog_pass(&self) {
        let thresholds = *self.thresholds.lock();
        let now = now_ms();
        let findings: Vec<(Arc<TaskShared>, Option<Stall>)> = {
            let state = self.state.lock();
            let running = state.running_per_pool();
            state
                .records
                .values()
                .filter(|record| !record.snapshot.status.is_finished())
                .map(|record| {
                    (
                        Arc::clone(&record.shared),
                        record.diagnose(now, &thresholds, running),
                    )
                })
                .collect()
        };
        for (shared, stall) in findings {
            let code = stall.as_ref().map_or(0, |stall| stall.kind.code());
            if shared.reported_stall.swap(code, Ordering::AcqRel) == code {
                continue;
            }
            match stall {
                Some(stall) => shared.emit(Level::WARN, &format!("stalled: {}", stall.message)),
                None => shared.emit(Level::INFO, "no longer stalled"),
            }
        }
    }
}

/// Current configured concurrency for the two editor-only worker pools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerCounts {
    pub on_demand: usize,
    pub dedicated: usize,
}

/// Cloneable task submitter and task-list reader.
#[derive(Clone)]
pub struct TaskQueue {
    inner: Arc<QueueInner>,
}

impl TaskQueue {
    pub fn new() -> Self {
        let _ = now_ms();
        let inner = Arc::new(QueueInner {
            state: Mutex::new(QueueState {
                on_demand_workers: DEFAULT_ON_DEMAND_WORKERS,
                dedicated_workers: DEFAULT_DEDICATED_WORKERS,
                ..QueueState::default()
            }),
            ready: Condvar::new(),
            revision: AtomicU64::new(0),
            spawned_workers: Mutex::new((0, 0)),
            thresholds: Mutex::new(StallThresholds::default()),
        });
        ensure_worker_threads(&inner, TaskDuration::Short, DEFAULT_ON_DEMAND_WORKERS);
        ensure_worker_threads(&inner, TaskDuration::Long, DEFAULT_DEDICATED_WORKERS);
        spawn_watchdog(Arc::downgrade(&inner));
        Self { inner }
    }

    pub fn worker_counts(&self) -> WorkerCounts {
        let state = self.inner.state.lock();
        WorkerCounts {
            on_demand: state.on_demand_workers,
            dedicated: state.dedicated_workers,
        }
    }

    /// Change pool concurrency. Reducing a pool lets its already-running work
    /// finish, then the excess workers become idle until the count grows again.
    pub fn set_worker_counts(&self, on_demand: usize, dedicated: usize) -> WorkerCounts {
        let counts = WorkerCounts {
            on_demand: on_demand.clamp(1, MAX_WORKERS_PER_POOL),
            dedicated: dedicated.clamp(1, MAX_WORKERS_PER_POOL),
        };
        {
            let mut state = self.inner.state.lock();
            state.on_demand_workers = counts.on_demand;
            state.dedicated_workers = counts.dedicated;
        }
        ensure_worker_threads(&self.inner, TaskDuration::Short, counts.on_demand);
        ensure_worker_threads(&self.inner, TaskDuration::Long, counts.dedicated);
        self.inner.changed();
        self.inner.ready.notify_all();
        counts
    }

    /// Change how long a task may go quiet before it is reported as stalled.
    pub fn set_stall_thresholds(&self, thresholds: StallThresholds) {
        *self.inner.thresholds.lock() = thresholds;
    }

    pub fn stall_thresholds(&self) -> StallThresholds {
        *self.inner.thresholds.lock()
    }

    /// Submit editor work that blocks its worker by design (a build, an
    /// import). Prefer [`Self::submit_async`] for anything that waits.
    ///
    /// The returned task remains in the list after it completes so users can
    /// inspect its outcome and its log.
    #[track_caller]
    pub fn submit(
        &self,
        description: TaskDescription,
        work: impl FnOnce(TaskContext) -> Result<(), String> + Send + 'static,
    ) -> TaskId {
        let origin = Location::caller();
        self.enqueue(
            description,
            TaskModel::Blocking,
            origin,
            Box::new(move |ctx| Box::pin(async move { work(ctx) })),
        )
    }

    /// Submit an async editor task. The future runs on a pool worker, inside
    /// the task's span; it must `.await` to wait and never block in a poll.
    /// It need not be `Send`: it is created and polled on one worker thread.
    #[track_caller]
    pub fn submit_async<F, Fut>(&self, description: TaskDescription, work: F) -> TaskId
    where
        F: FnOnce(TaskContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + 'static,
    {
        let origin = Location::caller();
        self.enqueue(
            description,
            TaskModel::Async,
            origin,
            Box::new(move |ctx| Box::pin(work(ctx))),
        )
    }

    fn enqueue(
        &self,
        description: TaskDescription,
        model: TaskModel,
        origin: &Location<'_>,
        work: Work,
    ) -> TaskId {
        let duration = match description.duration {
            TaskDuration::Foreground | TaskDuration::Persistent => TaskDuration::Short,
            other => other,
        };
        let shared = self.inner.create(&description, model, true, origin);
        let id = shared.id;
        self.inner.state.lock().pending.push(QueuedWork {
            id,
            duration,
            shared,
            work,
        });
        self.inner.ready.notify_all();
        id
    }

    /// Track a future that an executor owns (see [`tracked`]).
    #[track_caller]
    pub fn track<'a, T: TaskOutcome>(
        &self,
        description: TaskDescription,
        future: impl Future<Output = T> + 'a,
    ) -> Tracked<'a, T> {
        let origin = Location::caller();
        let shared = self
            .inner
            .create(&description, TaskModel::Async, false, origin);
        Tracked::new(
            TaskContext {
                inner: Arc::clone(&self.inner),
                shared,
            },
            Box::pin(future),
        )
    }

    /// Run `work` inline on the calling thread as a task (see [`scope`]).
    #[track_caller]
    pub fn scope<R: TaskOutcome>(
        &self,
        description: TaskDescription,
        work: impl FnOnce(&TaskContext) -> R,
    ) -> R {
        let origin = Location::caller();
        let description = TaskDescription {
            duration: TaskDuration::Foreground,
            ..description
        };
        let shared = self
            .inner
            .create(&description, TaskModel::Foreground, false, origin);
        let ctx = TaskContext {
            inner: Arc::clone(&self.inner),
            shared,
        };
        track::run_scope(&ctx, work)
    }

    pub fn snapshots(&self) -> Vec<TaskSnapshot> {
        let thresholds = *self.inner.thresholds.lock();
        let state = self.inner.state.lock();
        let now = now_ms();
        let running = state.running_per_pool();
        let mut snapshots = state
            .records
            .values()
            .map(|record| record.view(now, &thresholds, running))
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|a| std::cmp::Reverse(a.submitted_at));
        snapshots
    }

    /// The task's log buffer. Read it with [`TaskLog::read_from`]; it stays
    /// readable while the task is listed.
    pub fn log(&self, id: TaskId) -> Option<Arc<TaskLog>> {
        let state = self.inner.state.lock();
        state
            .records
            .get(&id)
            .map(|record| Arc::clone(&record.shared.log))
    }

    pub fn revision(&self) -> u64 {
        self.inner.revision.load(Ordering::Acquire)
    }

    pub fn set_starred(&self, id: TaskId, starred: bool) -> bool {
        let changed = {
            let mut state = self.inner.state.lock();
            let Some(record) = state.records.get_mut(&id) else {
                return false;
            };
            if record.snapshot.starred == starred {
                false
            } else {
                record.snapshot.starred = starred;
                true
            }
        };
        if changed {
            self.inner.changed();
            self.inner.ready.notify_all();
        }
        true
    }

    /// Cancel a queued job immediately or request cooperative cancellation
    /// from a running job. Work can check this through `TaskContext`.
    pub fn cancel(&self, id: TaskId) -> bool {
        let shared;
        let was_queued;
        {
            let mut state = self.inner.state.lock();
            let Some(record) = state.records.get_mut(&id) else {
                return false;
            };
            shared = Arc::clone(&record.shared);
            match record.snapshot.status {
                TaskStatus::Queued => {
                    shared.cancelled.store(true, Ordering::Release);
                    record.snapshot.status = TaskStatus::Cancelled;
                    record.snapshot.finished_at = Some(SystemTime::now());
                    was_queued = true;
                    state.pending.retain(|work| work.id != id);
                }
                TaskStatus::Running => {
                    shared.cancelled.store(true, Ordering::Release);
                    was_queued = false;
                }
                TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled => return false,
            }
        }
        if was_queued {
            shared.set_state(TaskActivity::Finished);
            shared.emit(Level::WARN, "cancelled while still queued; it never ran");
        } else {
            shared.emit(
                Level::WARN,
                "cancellation requested; the task must observe it to stop",
            );
        }
        self.inner.changed();
        true
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    static CURRENT: RefCell<Vec<TaskContext>> = const { RefCell::new(Vec::new()) };
}

/// The task whose work is running on this thread right now, if any. Deep
/// helpers use it to report progress or a phase without being passed the
/// context.
pub fn current() -> Option<TaskContext> {
    CURRENT.with(|stack| stack.borrow().last().cloned())
}

/// Keeps a task's span entered and its context current while alive.
pub(crate) struct EnterGuard {
    _span: tracing::span::EnteredSpan,
    /// The task the thread record named before this one entered.
    prev_task: u64,
}

impl Drop for EnterGuard {
    fn drop(&mut self) {
        CURRENT.with(|stack| {
            stack.borrow_mut().pop();
        });
        threads::set_current_task(self.prev_task);
    }
}

#[derive(Clone)]
pub struct TaskContext {
    pub(crate) inner: Arc<QueueInner>,
    pub(crate) shared: Arc<TaskShared>,
}

impl TaskContext {
    pub fn id(&self) -> TaskId {
        self.shared.id
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Acquire)
    }

    /// Mark this task cancelled after its work observes a cancellation request
    /// from its own domain-specific control (for example the build Cancel
    /// button).
    pub fn mark_cancelled(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
    }

    /// The task's tracing span. Instrument spawned sub-futures with it
    /// (`future.instrument(ctx.span())`) so their logs stay with this task.
    pub fn span(&self) -> tracing::Span {
        self.shared.span.clone()
    }

    pub(crate) fn enter(&self) -> EnterGuard {
        CURRENT.with(|stack| stack.borrow_mut().push(self.clone()));
        let prev_task = threads::set_current_task(self.shared.id.get());
        EnterGuard {
            _span: self.shared.span.clone().entered(),
            prev_task,
        }
    }

    /// Set progress from 0.0 to 1.0 and an optional human-readable stage.
    pub fn report_progress(&self, progress: f32, detail: impl Into<String>) {
        let detail = detail.into();
        {
            let mut state = self.inner.state.lock();
            if let Some(record) = state.records.get_mut(&self.shared.id) {
                record.snapshot.progress = Some(progress.clamp(0.0, 1.0));
                record.snapshot.detail = (!detail.is_empty()).then_some(detail);
            }
        }
        self.shared.touch();
        self.inner.changed();
    }

    /// Name the stage the task is in. It shows in the task list, resets the
    /// stall clock, and is written to the log, so a stall report says where
    /// the task stopped.
    pub fn set_phase(&self, phase: impl Into<String>) {
        let phase = phase.into();
        {
            let mut state = self.inner.state.lock();
            if let Some(record) = state.records.get_mut(&self.shared.id) {
                record.snapshot.phase = Some(phase.clone());
            }
        }
        self.shared.touch();
        self.inner.changed();
        if let Some(thread) = threads::current_thread() {
            thread.set_phase(phase.clone());
        }
        self.shared.emit(Level::INFO, &format!("phase: {phase}"));
    }

    /// Write a line to this task's log (and the engine log) at any level.
    pub fn log(&self, level: Level, message: impl AsRef<str>) {
        self.shared.touch();
        self.shared.emit(level, message.as_ref());
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.log(Level::INFO, message);
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.log(Level::WARN, message);
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.log(Level::ERROR, message);
    }

    /// Persistent tasks: one pass of the loop happened (a frame, a poll of the
    /// watcher). Cheap enough to call every iteration. The task is reported as
    /// stalled when beats stop for longer than [`Self::expect_heartbeat`].
    pub fn heartbeat(&self) {
        self.shared.beat(None);
    }

    /// [`Self::heartbeat`], also recording how long the pass of work took.
    pub fn tick(&self, work: Duration) {
        self.shared.beat(Some(work));
    }

    /// Persistent tasks: the loop is waiting for work on purpose (for the first
    /// UI frame, for a message), so silence is not a stall. Any heartbeat or
    /// `set_idle(false)` ends it and restarts the heartbeat clock.
    pub fn set_idle(&self, idle: bool) {
        if !idle {
            self.shared.last_beat_ms.store(now_ms(), Ordering::Release);
        }
        self.shared.idle.store(idle, Ordering::Release);
        self.shared.touch();
    }

    /// Replace what the task says about itself by name (owner, size, mode):
    /// [`TaskSnapshot::details`]. For a view that lists one kind of task as a
    /// table; it is not logged.
    pub fn set_details(&self, details: Vec<(String, String)>) {
        {
            let mut state = self.inner.state.lock();
            if let Some(record) = state.records.get_mut(&self.shared.id) {
                record.snapshot.details = details;
            }
        }
        self.inner.changed();
    }

    /// Persistent tasks: how long between heartbeats counts as stalled. Zero
    /// turns the check off.
    pub fn expect_heartbeat(&self, every: Duration) {
        self.shared
            .beat_timeout_ms
            .store(every.as_millis() as u64, Ordering::Release);
    }
}

fn ensure_worker_threads(inner: &Arc<QueueInner>, duration: TaskDuration, target: usize) {
    let mut spawned = inner.spawned_workers.lock();
    let count = match duration {
        TaskDuration::Long => &mut spawned.1,
        _ => &mut spawned.0,
    };
    while *count < target {
        let index = *count;
        *count += 1;
        spawn_worker(Arc::clone(inner), duration, index);
    }
}

fn spawn_worker(inner: Arc<QueueInner>, duration: TaskDuration, index: usize) {
    let pool = match duration {
        TaskDuration::Long => "long",
        _ => "short",
    };
    std::thread::Builder::new()
        .name(format!("editor-task-{pool}-{index}"))
        .spawn(move || worker_loop(inner, duration, index))
        .expect("failed to spawn editor task worker");
}

fn spawn_watchdog(inner: Weak<QueueInner>) {
    let spawned = std::thread::Builder::new()
        .name("editor-task-watchdog".into())
        .spawn(move || {
            let _registered = threads::register_current_thread(
                ThreadSpec::new("editor-task-watchdog", ThreadRole::Other)
                    .owner("editor_task_queue"),
            );
            loop {
                std::thread::sleep(WATCHDOG_INTERVAL);
                let Some(inner) = inner.upgrade() else { return };
                inner.watchdog_pass();
            }
        });
    if let Err(error) = spawned {
        tracing::error!("failed to start the editor task watchdog: {error}");
    }
}

fn worker_loop(inner: Arc<QueueInner>, duration: TaskDuration, worker_index: usize) {
    let (pool, name) = match duration {
        TaskDuration::Long => ("editor-long", format!("editor-task-long-{worker_index}")),
        _ => ("editor-short", format!("editor-task-short-{worker_index}")),
    };
    let _registered = threads::register_current_thread(
        ThreadSpec::new(name, ThreadRole::EditorWorker)
            .pool(pool)
            .owner("editor_task_queue"),
    );
    let thread = threads::current_thread();
    loop {
        if let Some(thread) = &thread {
            thread.mark_parked();
            thread.clear_phase();
        }
        let work = {
            let mut state = inner.state.lock();
            loop {
                let active_workers = match duration {
                    TaskDuration::Long => state.dedicated_workers,
                    _ => state.on_demand_workers,
                };
                if worker_index >= active_workers {
                    inner.ready.wait(&mut state);
                    continue;
                }
                let next = state
                    .pending
                    .iter()
                    .enumerate()
                    .filter(|(_, work)| work.duration == duration)
                    .max_by_key(|(_, work)| {
                        let record = state.records.get(&work.id).expect("queued task record");
                        (
                            record.snapshot.starred,
                            std::cmp::Reverse(record.snapshot.submitted_at),
                        )
                    })
                    .map(|(ix, _)| ix);
                if let Some(ix) = next {
                    let work = state.pending.swap_remove(ix);
                    // Running before the lock is released, so a cancel cannot
                    // slip between taking the work and starting it.
                    if let Some(record) = state.records.get_mut(&work.id) {
                        record.snapshot.status = TaskStatus::Running;
                    }
                    inner.changed();
                    break work;
                }
                inner.ready.wait(&mut state);
            }
        };

        if let Some(thread) = &thread {
            thread.mark_running();
        }
        let ctx = TaskContext {
            inner: Arc::clone(&inner),
            shared: Arc::clone(&work.shared),
        };
        inner.mark_started(&work.shared);
        // The future is created and polled on this thread only. A panic is
        // recorded by the tracked future (with its message) before it
        // unwinds to here; the worker survives it.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let future = (work.work)(ctx.clone());
            let tracked = Tracked::new(ctx, future);
            futures::executor::block_on(tracked)
        }));
    }
}

#[cfg(test)]
mod tests;
