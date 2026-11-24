//! The task system's value types: ids, durations, models, statuses, stalls and snapshots.

use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(pub(crate) u64);

impl TaskId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskDuration {
    Short,
    Long,
    /// Runs inline on the calling thread (see [`scope`](crate::scope)); not a pool.
    Foreground,
    /// Lives as long as its thread: a render loop, a watcher, a language
    /// server (see [`TaskQueue::spawn_service`](crate::TaskQueue::spawn_service)). It has no result; it has a
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
    pub(crate) fn from_u8(value: u8) -> Self {
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
    pub(crate) fn code(self) -> u8 {
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

    /// A description for a persistent task ([`TaskQueue::spawn_service`](crate::TaskQueue::spawn_service)).
    pub fn persistent(title: impl Into<String>, category: impl Into<String>) -> Self {
        Self::new(title, category, TaskDuration::Persistent)
    }

    /// A description for inline work ([`scope`](crate::scope)).
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
    /// The stage the task last named with [`TaskContext::set_phase`](crate::TaskContext::set_phase).
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
    /// [`TaskContext::set_details`](crate::TaskContext::set_details).
    pub details: Vec<(String, String)>,
}
