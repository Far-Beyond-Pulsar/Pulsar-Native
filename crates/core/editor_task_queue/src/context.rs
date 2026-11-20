//! `TaskContext`: what running task work uses to report progress, phases and heartbeats.

use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::Level;

use crate::shared::{now_ms, TaskShared};
use crate::state::QueueInner;
use crate::threads;
use crate::types::TaskId;

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
    /// [`TaskSnapshot::details`](crate::TaskSnapshot::details). For a view that lists one kind of task as a
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
