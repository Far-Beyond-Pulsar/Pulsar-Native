//! Persistent tasks: threads that live as long as the thing they serve.
//!
//! A render loop, a file watcher, a language-server client. They never
//! "succeed"; they run until stopped, so they are judged by a heartbeat: the
//! loop calls [`TaskContext::heartbeat`] (or [`TaskContext::tick`]) every pass,
//! and the watchdog reports a stall when the beats stop. Each has a task
//! record, a span, and a log like any other task, and its thread is registered
//! with its role (see [`crate::threads`]).

use crate::threads::{self, ThreadRole, ThreadSpec};
use crate::{global, TaskContext, TaskDescription, TaskId, TaskModel, TaskOutcome, TaskQueue};
use std::panic::Location;
use std::sync::Arc;
use std::time::Duration;

/// How often a service's loop is expected to beat unless it says otherwise.
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(5);

/// A persistent task and the thread it runs on.
#[derive(Clone, Debug)]
pub struct ServiceDescription {
    pub title: String,
    pub category: String,
    pub thread: ThreadSpec,
    /// Longest silence before the task is reported stalled. `None` turns the
    /// check off (for a loop with no natural cadence); prefer `set_idle`.
    pub heartbeat: Option<Duration>,
}

impl ServiceDescription {
    pub fn new(
        title: impl Into<String>,
        category: impl Into<String>,
        thread_name: impl Into<String>,
        role: ThreadRole,
    ) -> Self {
        Self {
            title: title.into(),
            category: category.into(),
            thread: ThreadSpec::new(thread_name, role),
            heartbeat: Some(DEFAULT_HEARTBEAT),
        }
    }

    pub fn owner(mut self, owner: impl Into<String>) -> Self {
        self.thread = self.thread.owner(owner);
        self
    }

    pub fn heartbeat(mut self, every: Duration) -> Self {
        self.heartbeat = Some(every);
        self
    }

    pub fn no_heartbeat(mut self) -> Self {
        self.heartbeat = None;
        self
    }

    pub fn stack_size(mut self, bytes: usize) -> Self {
        self.thread = self.thread.stack_size(bytes);
        self
    }
}

/// The running service: join it, ask it to stop, or read its context.
pub struct ServiceHandle<T> {
    join: std::thread::JoinHandle<T>,
    ctx: TaskContext,
}

impl<T> ServiceHandle<T> {
    pub fn id(&self) -> TaskId {
        self.ctx.id()
    }

    pub fn context(&self) -> TaskContext {
        self.ctx.clone()
    }

    /// Ask the loop to stop. It must watch [`TaskContext::is_cancelled`] (or
    /// its own stop flag) to obey.
    pub fn request_stop(&self) {
        TaskQueue {
            inner: Arc::clone(&self.ctx.inner),
        }
        .cancel(self.ctx.id());
    }

    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    pub fn join(self) -> std::thread::Result<T> {
        self.join.join()
    }
}

impl TaskQueue {
    /// Start a persistent task on its own named thread.
    ///
    /// `work` is the loop. It runs inside the task's span (its `tracing` output
    /// goes to the task's log), should call `ctx.heartbeat()` each pass, and
    /// should call `ctx.set_phase(..)` as it changes what it does. Returning
    /// ends the task (`Err` fails it); a panic is recorded with its message.
    #[track_caller]
    pub fn spawn_service<T: TaskOutcome + Send + 'static>(
        &self,
        description: ServiceDescription,
        work: impl FnOnce(&TaskContext) -> T + Send + 'static,
    ) -> std::io::Result<ServiceHandle<T>> {
        let origin = Location::caller();
        let task = TaskDescription::persistent(&description.title, &description.category);
        let shared = self
            .inner
            .create(&task, TaskModel::Persistent, false, origin);
        shared.beat_timeout_ms.store(
            description
                .heartbeat
                .map_or(0, |every| every.as_millis() as u64),
            std::sync::atomic::Ordering::Release,
        );
        let ctx = TaskContext {
            inner: Arc::clone(&self.inner),
            shared,
        };
        let thread_ctx = ctx.clone();
        let spec = description.thread;
        let mut builder = std::thread::Builder::new().name(spec.name.clone());
        if let Some(bytes) = spec.stack_size {
            builder = builder.stack_size(bytes);
        }
        let join = builder
            .spawn(move || {
                let _registered = threads::register_current_thread(spec);
                crate::track::run_scope(&thread_ctx, work)
            })
            .inspect_err(|error| {
                ctx.inner.finish(
                    &ctx.shared,
                    crate::Outcome::Failed(format!("could not start its thread: {error}")),
                );
            })?;
        Ok(ServiceHandle { join, ctx })
    }
}

/// A persistent task with no thread of its own: something that lives as long
/// as what it serves while another loop drives it (a render runtime, which
/// the runtimes' scheduler draws). Its owner beats, names phases, logs (enter
/// [`TaskContext::span`]) and publishes details through
/// [`context`](Self::context); dropping it ends the task.
pub struct PersistentTask {
    ctx: TaskContext,
}

impl PersistentTask {
    pub fn context(&self) -> &TaskContext {
        &self.ctx
    }
}

impl Drop for PersistentTask {
    fn drop(&mut self) {
        self.ctx
            .inner
            .finish(&self.ctx.shared, crate::Outcome::Succeeded);
    }
}

impl TaskQueue {
    /// A persistent task driven by its owner's loop rather than a thread of
    /// its own (see [`PersistentTask`]). `heartbeat` is the longest silence
    /// before it is reported stalled while not idle; `None` turns the check
    /// off.
    #[track_caller]
    pub fn persistent_task(
        &self,
        description: TaskDescription,
        heartbeat: Option<Duration>,
    ) -> PersistentTask {
        let origin = Location::caller();
        let shared = self
            .inner
            .create(&description, TaskModel::Persistent, false, origin);
        shared.beat_timeout_ms.store(
            heartbeat.map_or(0, |every| every.as_millis() as u64),
            std::sync::atomic::Ordering::Release,
        );
        let ctx = TaskContext {
            inner: Arc::clone(&self.inner),
            shared,
        };
        ctx.inner.mark_started(&ctx.shared);
        PersistentTask { ctx }
    }
}

/// [`TaskQueue::spawn_service`] on the global queue.
#[track_caller]
pub fn spawn_service<T: TaskOutcome + Send + 'static>(
    description: ServiceDescription,
    work: impl FnOnce(&TaskContext) -> T + Send + 'static,
) -> std::io::Result<ServiceHandle<T>> {
    global().spawn_service(description, work)
}
