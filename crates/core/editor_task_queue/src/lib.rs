//! Queue for asynchronous editor work. Game/runtime work must never be
//! submitted here; it has its own lifecycle and scheduling requirements.
//!
//! Short jobs share a small reserved worker pool. Long jobs use a separate
//! pool so sustained work (builds, thumbnail generation, imports) cannot
//! starve short editor interactions. Starring a queued job raises its
//! scheduling priority within its pool.

use parking_lot::{Condvar, Mutex};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

const SHORT_WORKERS: usize = 3;
const LONG_WORKERS: usize = 2;

static GLOBAL: OnceLock<TaskQueue> = OnceLock::new();

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
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
}

#[derive(Clone, Debug)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub title: String,
    pub category: String,
    pub duration: TaskDuration,
    pub status: TaskStatus,
    pub starred: bool,
    pub progress: Option<f32>,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub submitted_at: SystemTime,
}

type Work = Box<dyn FnOnce(TaskContext) -> Result<(), String> + Send + 'static>;

struct QueuedWork {
    id: TaskId,
    duration: TaskDuration,
    work: Work,
}

struct TaskRecord {
    snapshot: TaskSnapshot,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
struct QueueState {
    records: BTreeMap<TaskId, TaskRecord>,
    pending: Vec<QueuedWork>,
}

struct QueueInner {
    state: Mutex<QueueState>,
    ready: Condvar,
    next_id: AtomicU64,
    revision: AtomicU64,
}

/// Cloneable task submitter and task-list reader.
#[derive(Clone)]
pub struct TaskQueue {
    inner: Arc<QueueInner>,
}

impl TaskQueue {
    pub fn new() -> Self {
        let inner = Arc::new(QueueInner {
            state: Mutex::new(QueueState::default()),
            ready: Condvar::new(),
            next_id: AtomicU64::new(1),
            revision: AtomicU64::new(0),
        });
        for ix in 0..SHORT_WORKERS {
            spawn_worker(Arc::clone(&inner), TaskDuration::Short, ix);
        }
        for ix in 0..LONG_WORKERS {
            spawn_worker(Arc::clone(&inner), TaskDuration::Long, ix);
        }
        Self { inner }
    }

    /// Submit editor work. The returned task remains in the list after it
    /// completes so users can inspect its outcome.
    pub fn submit(
        &self,
        description: TaskDescription,
        work: impl FnOnce(TaskContext) -> Result<(), String> + Send + 'static,
    ) -> TaskId {
        let id = TaskId(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let cancelled = Arc::new(AtomicBool::new(false));
        let snapshot = TaskSnapshot {
            id,
            title: description.title,
            category: description.category,
            duration: description.duration,
            status: TaskStatus::Queued,
            starred: false,
            progress: None,
            detail: None,
            error: None,
            submitted_at: SystemTime::now(),
        };
        {
            let mut state = self.inner.state.lock();
            state.records.insert(
                id,
                TaskRecord {
                    snapshot,
                    cancelled,
                },
            );
            state.pending.push(QueuedWork {
                id,
                duration: description.duration,
                work: Box::new(work),
            });
        }
        self.changed();
        self.inner.ready.notify_all();
        id
    }

    pub fn snapshots(&self) -> Vec<TaskSnapshot> {
        let state = self.inner.state.lock();
        let mut snapshots = state
            .records
            .values()
            .map(|record| record.snapshot.clone())
            .collect::<Vec<_>>();
        snapshots.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));
        snapshots
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
            self.changed();
            self.inner.ready.notify_all();
        }
        true
    }

    /// Cancel a queued job immediately or request cooperative cancellation
    /// from a running job. Work can check this through `TaskContext`.
    pub fn cancel(&self, id: TaskId) -> bool {
        let mut state = self.inner.state.lock();
        let Some(record) = state.records.get_mut(&id) else {
            return false;
        };
        match record.snapshot.status {
            TaskStatus::Queued => {
                record.cancelled.store(true, Ordering::Release);
                record.snapshot.status = TaskStatus::Cancelled;
                state.pending.retain(|work| work.id != id);
            }
            TaskStatus::Running => record.cancelled.store(true, Ordering::Release),
            TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled => return false,
        }
        drop(state);
        self.changed();
        true
    }

    fn changed(&self) {
        self.inner.revision.fetch_add(1, Ordering::AcqRel);
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct TaskContext {
    id: TaskId,
    inner: Arc<QueueInner>,
    cancelled: Arc<AtomicBool>,
}

impl TaskContext {
    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Mark this task cancelled after its work observes a cancellation request
    /// from its own domain-specific control (for example the build Cancel
    /// button).
    pub fn mark_cancelled(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Set progress from 0.0 to 1.0 and an optional human-readable stage.
    pub fn report_progress(&self, progress: f32, detail: impl Into<String>) {
        let detail = detail.into();
        let mut state = self.inner.state.lock();
        if let Some(record) = state.records.get_mut(&self.id) {
            record.snapshot.progress = Some(progress.clamp(0.0, 1.0));
            record.snapshot.detail = (!detail.is_empty()).then_some(detail);
        }
        drop(state);
        self.inner.revision.fetch_add(1, Ordering::AcqRel);
    }
}

fn spawn_worker(inner: Arc<QueueInner>, duration: TaskDuration, index: usize) {
    let pool = match duration {
        TaskDuration::Short => "short",
        TaskDuration::Long => "long",
    };
    std::thread::Builder::new()
        .name(format!("editor-task-{pool}-{index}"))
        .spawn(move || worker_loop(inner, duration))
        .expect("failed to spawn editor task worker");
}

fn worker_loop(inner: Arc<QueueInner>, duration: TaskDuration) {
    loop {
        let work = {
            let mut state = inner.state.lock();
            loop {
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
                    if let Some(record) = state.records.get_mut(&work.id) {
                        record.snapshot.status = TaskStatus::Running;
                    }
                    inner.revision.fetch_add(1, Ordering::AcqRel);
                    break work;
                }
                inner.ready.wait(&mut state);
            }
        };

        let cancellation = {
            let state = inner.state.lock();
            state
                .records
                .get(&work.id)
                .map(|record| Arc::clone(&record.cancelled))
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
        };
        let context = TaskContext {
            id: work.id,
            inner: Arc::clone(&inner),
            cancelled: Arc::clone(&cancellation),
        };
        let result = catch_unwind(AssertUnwindSafe(|| (work.work)(context)));
        let mut state = inner.state.lock();
        if let Some(record) = state.records.get_mut(&work.id) {
            if cancellation.load(Ordering::Acquire) {
                record.snapshot.status = TaskStatus::Cancelled;
            } else {
                match result {
                    Ok(Ok(())) => {
                        record.snapshot.status = TaskStatus::Succeeded;
                        record.snapshot.progress = Some(1.0);
                    }
                    Ok(Err(error)) => {
                        record.snapshot.status = TaskStatus::Failed;
                        record.snapshot.error = Some(error);
                    }
                    Err(_) => {
                        record.snapshot.status = TaskStatus::Failed;
                        record.snapshot.error = Some("Task panicked".into());
                    }
                }
            }
        }
        drop(state);
        inner.revision.fetch_add(1, Ordering::AcqRel);
    }
}
