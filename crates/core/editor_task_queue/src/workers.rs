//! Worker and watchdog threads of the queue's pools.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::context::TaskContext;
use crate::state::QueueInner;
use crate::threads::{self, ThreadRole, ThreadSpec};
use crate::track::Tracked;
use crate::types::{TaskDuration, TaskStatus};

const WATCHDOG_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) fn ensure_worker_threads(
    inner: &Arc<QueueInner>,
    duration: TaskDuration,
    target: usize,
) {
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

pub(crate) fn spawn_watchdog(inner: Weak<QueueInner>) {
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
