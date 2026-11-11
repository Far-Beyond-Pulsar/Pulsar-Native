//! Tracked futures and inline scopes: the two ways to put work that does not
//! run on a pool worker under the task system.

use crate::{global, Outcome, TaskContext, TaskDescription, TaskId};
use std::future::Future;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::task::{Context, Poll};
use tracing::Level;

/// How a unit of work's return value maps to a task's final status.
///
/// `()` always succeeds; `Result<T, E>` fails with the error's message.
pub trait TaskOutcome {
    fn failure(&self) -> Option<String>;
}

impl TaskOutcome for () {
    fn failure(&self) -> Option<String> {
        None
    }
}

impl<T, E: std::fmt::Display> TaskOutcome for Result<T, E> {
    fn failure(&self) -> Option<String> {
        self.as_ref().err().map(ToString::to_string)
    }
}

fn outcome_of<T: TaskOutcome>(value: &T) -> Outcome {
    match value.failure() {
        None => Outcome::Succeeded,
        Some(error) => Outcome::Failed(error),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

fn record_panic(ctx: &TaskContext, payload: &(dyn std::any::Any + Send)) {
    ctx.shared.emit(
        Level::ERROR,
        &format!("panicked: {}", panic_message(payload)),
    );
    ctx.inner
        .finish(&ctx.shared, Outcome::Failed("Task panicked".into()));
}

/// A future under the task system. It records every poll (how long it holds
/// its thread, how long it waits between polls), runs inside the task's span
/// so `tracing` output is attributed to the task, and ends the task when it
/// completes, panics, or is dropped.
///
/// Any executor can run it. Use [`tracked`] to wrap work given to `cx.spawn`.
pub struct Tracked<'a, T: TaskOutcome> {
    ctx: TaskContext,
    future: Pin<Box<dyn Future<Output = T> + 'a>>,
    done: bool,
}

impl<'a, T: TaskOutcome> Tracked<'a, T> {
    pub(crate) fn new(ctx: TaskContext, future: Pin<Box<dyn Future<Output = T> + 'a>>) -> Self {
        Self {
            ctx,
            future,
            done: false,
        }
    }

    pub fn id(&self) -> TaskId {
        self.ctx.id()
    }

    /// The task's context, for progress, phases and cancellation checks.
    pub fn context(&self) -> TaskContext {
        self.ctx.clone()
    }
}

impl<T: TaskOutcome> Future for Tracked<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        this.ctx.inner.mark_started(&this.ctx.shared);
        this.ctx.shared.begin_poll();
        let guard = this.ctx.enter();
        let polled = catch_unwind(AssertUnwindSafe(|| this.future.as_mut().poll(cx)));
        drop(guard);
        match polled {
            Ok(Poll::Ready(value)) => {
                this.done = true;
                this.ctx.inner.finish(&this.ctx.shared, outcome_of(&value));
                Poll::Ready(value)
            }
            Ok(Poll::Pending) => {
                this.ctx.shared.set_state(crate::TaskActivity::Pending);
                Poll::Pending
            }
            Err(payload) => {
                this.done = true;
                record_panic(&this.ctx, payload.as_ref());
                resume_unwind(payload)
            }
        }
    }
}

impl<T: TaskOutcome> Drop for Tracked<'_, T> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let outcome = if std::thread::panicking() {
            Outcome::Failed("Task panicked".into())
        } else {
            Outcome::Dropped
        };
        self.ctx.inner.finish(&self.ctx.shared, outcome);
    }
}

/// Put a future under the task system, on the global queue.
///
/// ```ignore
/// cx.spawn(async move |this, cx| {
///     editor_task_queue::tracked(
///         TaskDescription::new("Load level", "level", TaskDuration::Short),
///         async move { load(&path).await },
///     )
///     .await
/// })
/// .detach();
/// ```
///
/// The task is created now, so a future that is built and never polled shows
/// up as stalled. Spawn anything it starts with the task's span
/// (`future.instrument(ctx.span())`) to keep their logs with it.
#[track_caller]
pub fn tracked<'a, T: TaskOutcome>(
    description: TaskDescription,
    future: impl Future<Output = T> + 'a,
) -> Tracked<'a, T> {
    global().track(description, future)
}

/// Run `work` inline on the calling thread as a task.
///
/// For work that cannot leave its thread: building a GPUI entity, first-frame
/// GPU setup. The task is listed while it runs, its `tracing` output is
/// captured to its log, and the watchdog reports it if it holds the thread
/// for more than a second. Prefer an async task wherever the work can move.
///
/// A `Result` return makes an `Err` the task's failure; a panic is recorded
/// (with its message) and then continues to unwind.
#[track_caller]
pub fn scope<R: TaskOutcome>(
    description: TaskDescription,
    work: impl FnOnce(&TaskContext) -> R,
) -> R {
    global().scope(description, work)
}

/// Body of [`TaskQueue::scope`](crate::TaskQueue::scope).
pub(crate) fn run_scope<R: TaskOutcome>(
    ctx: &TaskContext,
    work: impl FnOnce(&TaskContext) -> R,
) -> R {
    ctx.inner.mark_started(&ctx.shared);
    ctx.shared.begin_poll();
    let guard = ctx.enter();
    let result = catch_unwind(AssertUnwindSafe(|| work(ctx)));
    drop(guard);
    match result {
        Ok(value) => {
            ctx.inner.finish(&ctx.shared, outcome_of(&value));
            value
        }
        Err(payload) => {
            record_panic(ctx, payload.as_ref());
            resume_unwind(payload)
        }
    }
}
