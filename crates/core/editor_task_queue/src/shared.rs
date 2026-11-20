//! State shared between a task's record, its context and its future, and the task clock.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tracing::Level;

use crate::log;
use crate::types::{TaskActivity, TaskId, TaskModel, TaskSnapshot};

static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the first task system use in this process.
pub(crate) fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
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
    pub(crate) state: AtomicU8,
    pub(crate) state_since: AtomicU64,
    pub(crate) polls: AtomicU64,
    pub(crate) last_activity: AtomicU64,
    pub(crate) reported_stall: AtomicU8,
    pub(crate) beats: AtomicU64,
    pub(crate) last_beat_ms: AtomicU64,
    pub(crate) beat_interval_us: AtomicU64,
    pub(crate) tick_us: AtomicU64,
    pub(crate) idle: AtomicBool,
    pub(crate) beat_timeout_ms: AtomicU64,
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

    pub(crate) fn activity(&self) -> (TaskActivity, u64) {
        (
            TaskActivity::from_u8(self.state.load(Ordering::Acquire)),
            self.state_since.load(Ordering::Acquire),
        )
    }

    pub(crate) fn last_activity_ms(&self) -> u64 {
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

    pub(crate) fn fill_heartbeat(&self, snapshot: &mut TaskSnapshot, now: u64) {
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
