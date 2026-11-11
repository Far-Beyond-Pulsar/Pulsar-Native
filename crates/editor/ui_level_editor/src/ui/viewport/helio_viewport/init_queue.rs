//! One background lane for cold Helio renderer initialization.
//!
//! A split editor can create several renderers at once. Serializing their cold
//! graph builds avoids a burst of synchronous driver pipeline creation while
//! keeping every viewport's render loop independent.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::OnceLock;

type Job = Box<dyn FnOnce() + Send + 'static>;

static INIT_QUEUE: OnceLock<Option<Sender<Job>>> = OnceLock::new();

fn sender() -> Option<&'static Sender<Job>> {
    INIT_QUEUE
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            // A persistent task with no heartbeat of its own: it is idle between
            // jobs, and a job is a renderer's cold start, which can take
            // seconds. The jobs' own work is tracked by the renderer's scopes.
            editor_task_queue::spawn_service(
                editor_task_queue::ServiceDescription::new(
                    "Helio initialization lane",
                    "render",
                    "Helio Initialization",
                    editor_task_queue::ThreadRole::Render,
                )
                .owner("level editor viewport")
                .no_heartbeat(),
                move |ctx| {
                    ctx.set_phase("Waiting for a renderer to initialize");
                    ctx.set_idle(true);
                    while let Ok(job) = rx.recv() {
                        ctx.set_idle(false);
                        ctx.set_phase("Initializing a renderer");
                        job();
                        ctx.set_phase("Waiting for a renderer to initialize");
                        ctx.set_idle(true);
                    }
                },
            )
            .map(|_| tx)
            .map_err(|error| {
                tracing::error!(%error, "Failed to start Helio initialization worker");
            })
            .ok()
        })
        .as_ref()
}

/// Schedule one cold renderer initialization on the shared GPU setup lane.
/// The returned receiver is waited on only by the viewport render thread.
pub(super) fn enqueue<T>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<Receiver<Result<T, ()>>>
where
    T: Send + 'static,
{
    let sender = sender()?;
    let (tx, rx) = mpsc::sync_channel(1);
    sender
        .send(Box::new(move || {
            let result = catch_unwind(AssertUnwindSafe(work)).map_err(|_| ());
            let _ = tx.send(result);
        }))
        .ok()?;
    Some(rx)
}
