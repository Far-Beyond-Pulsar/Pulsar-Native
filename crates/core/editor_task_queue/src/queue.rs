//! `TaskQueue`: submitting tasks, listing them and tuning the worker pools.

use parking_lot::{Condvar, Mutex};
use std::future::Future;
use std::panic::Location;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;
use tracing::Level;

use crate::context::TaskContext;
use crate::log::TaskLog;
use crate::shared::now_ms;
use crate::state::{QueueInner, QueueState, QueuedWork, Work};
use crate::track::{self, TaskOutcome, Tracked};
use crate::types::{
    StallThresholds, TaskActivity, TaskDescription, TaskDuration, TaskId, TaskModel, TaskSnapshot,
    TaskStatus,
};
use crate::workers::{ensure_worker_threads, spawn_watchdog};

pub(crate) const DEFAULT_ON_DEMAND_WORKERS: usize = 3;
pub(crate) const DEFAULT_DEDICATED_WORKERS: usize = 2;
pub(crate) const MAX_WORKERS_PER_POOL: usize = 16;
static GLOBAL: OnceLock<TaskQueue> = OnceLock::new();

/// Process-wide queue used only by the editor.
pub fn global() -> &'static TaskQueue {
    GLOBAL.get_or_init(TaskQueue::new)
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
    pub(crate) inner: Arc<QueueInner>,
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

    /// Track a future that an executor owns (see [`tracked`](crate::tracked)).
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

    /// Run `work` inline on the calling thread as a task (see [`scope`](crate::scope)).
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
