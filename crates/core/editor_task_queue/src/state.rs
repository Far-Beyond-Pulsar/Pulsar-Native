//! The queue's task records, pending work and the shared state behind `TaskQueue`.

use parking_lot::{Condvar, Mutex};
use std::collections::BTreeMap;
use std::future::Future;
use std::panic::Location;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::Level;

use crate::context::TaskContext;
use crate::log;
use crate::shared::{now_ms, Outcome, TaskShared};
use crate::types::{
    Stall, StallKind, StallThresholds, TaskActivity, TaskDescription, TaskDuration, TaskId,
    TaskModel, TaskSnapshot, TaskStatus,
};

/// Finished tasks kept for the audit history before the oldest unstarred ones
/// are forgotten.
pub(crate) const MAX_FINISHED_TASKS: usize = 500;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) type LocalFuture = Pin<Box<dyn Future<Output = Result<(), String>>>>;
pub(crate) type Work = Box<dyn FnOnce(TaskContext) -> LocalFuture + Send + 'static>;

pub(crate) struct QueuedWork {
    pub(crate) id: TaskId,
    pub(crate) duration: TaskDuration,
    pub(crate) shared: Arc<TaskShared>,
    pub(crate) work: Work,
}

pub(crate) struct TaskRecord {
    pub(crate) snapshot: TaskSnapshot,
    pub(crate) shared: Arc<TaskShared>,
}

impl TaskRecord {
    /// The snapshot with its live, computed fields filled in.
    pub(crate) fn view(
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
pub(crate) struct QueueState {
    pub(crate) records: BTreeMap<TaskId, TaskRecord>,
    pub(crate) pending: Vec<QueuedWork>,
    pub(crate) on_demand_workers: usize,
    pub(crate) dedicated_workers: usize,
}

impl QueueState {
    pub(crate) fn running_per_pool(&self) -> (usize, usize) {
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

pub(crate) struct QueueInner {
    pub(crate) state: Mutex<QueueState>,
    pub(crate) ready: Condvar,
    pub(crate) revision: AtomicU64,
    pub(crate) spawned_workers: Mutex<(usize, usize)>,
    pub(crate) thresholds: Mutex<StallThresholds>,
}

impl QueueInner {
    pub(crate) fn changed(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// Create a task record, its log and its span.
    pub(crate) fn create(
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
    pub(crate) fn watchdog_pass(&self) {
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
