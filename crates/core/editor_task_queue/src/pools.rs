//! The engine's one rayon pool and one tokio runtime.
//!
//! Library code that parallelises or spawns async work without naming a pool
//! (`par_iter`, `rayon::join`, `tokio::spawn`, `Handle::current()`) ends up on
//! these, so it is our threads doing the work, registered in the thread
//! registry with a role and a pool name, whatever the library meant to use.
//!
//! What this cannot capture: code that builds its **own** pool or runtime
//! (`ThreadPoolBuilder::new().build()`, `Runtime::new()`, `reqwest::blocking`).
//! Those threads are invisible here by construction. The raw-thread ratchet
//! (`tests/raw_threads.rs`) lists the sites in our own code that still do.
//!
//! Visibility differs by library:
//!
//! * tokio gives start, stop, park and unpark hooks, so each worker is
//!   registered and its running/parked state is exact. Per-task hooks need
//!   `--cfg tokio_unstable` and are not enabled (see `.agents/EDITOR_TASKS.md`).
//! * rayon gives start and exit only, so its workers are registered with
//!   [`ThreadState::Unknown`](crate::ThreadState).

use crate::threads::{self, ThreadGuard, ThreadRole, ThreadSpec};
use std::cell::RefCell;
use std::sync::OnceLock;

/// Pool name shown for rayon workers.
pub const RAYON_POOL: &str = "rayon";
/// Pool name shown for tokio workers.
pub const TOKIO_POOL: &str = "tokio";

/// Make rayon's global pool ours. Call once at startup, before anything uses
/// rayon (the first `par_iter` would otherwise create the default pool, whose
/// threads we cannot see). `threads: None` keeps rayon's default width.
///
/// Fails if the global pool already exists.
pub fn install_rayon_global(threads: Option<usize>) -> Result<(), String> {
    let mut builder = rayon::ThreadPoolBuilder::new()
        .thread_name(|index| format!("rayon-{index}"))
        .spawn_handler(|thread| {
            let mut spawn = std::thread::Builder::new();
            if let Some(name) = thread.name() {
                spawn = spawn.name(name.to_owned());
            }
            if let Some(bytes) = thread.stack_size() {
                spawn = spawn.stack_size(bytes);
            }
            let name = thread.name().unwrap_or("rayon").to_owned();
            spawn.spawn(move || {
                let _registered = threads::register_current_thread(
                    ThreadSpec::new(name, ThreadRole::Compute)
                        .pool(RAYON_POOL)
                        .owner("rayon global pool"),
                );
                // The registered state stays `Running`: rayon has no park hook,
                // so mark it unknown instead of claiming it is busy.
                if let Some(handle) = threads::current_thread() {
                    handle.mark_unknown();
                }
                thread.run();
            })?;
            Ok(())
        });
    if let Some(threads) = threads {
        builder = builder.num_threads(threads);
    }
    builder
        .build_global()
        .map_err(|error| format!("rayon's global pool was already created: {error}"))
}

thread_local! {
    static TOKIO_GUARD: RefCell<Option<ThreadGuard>> = const { RefCell::new(None) };
}

static TOKIO_HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// Build the engine's tokio runtime with every worker registered. `on_start`
/// runs on each worker as it starts (profiler thread naming).
///
/// The first runtime built here becomes [`tokio_handle`]. Code that needs async
/// work and has no runtime of its own should use that handle instead of
/// building a runtime.
pub fn build_tokio_runtime(
    workers: usize,
    on_start: Option<fn()>,
) -> std::io::Result<tokio::runtime::Runtime> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .thread_name("PulsarEngineRuntime")
        .on_thread_start(move || {
            let name = std::thread::current()
                .name()
                .unwrap_or("PulsarEngineRuntime")
                .to_owned();
            let guard = threads::register_current_thread(
                ThreadSpec::new(
                    format!("{name}-{:?}", std::thread::current().id()),
                    ThreadRole::Async,
                )
                .pool(TOKIO_POOL)
                .owner("engine tokio runtime"),
            );
            // Parked until the runtime hands this worker something.
            if let Some(handle) = threads::current_thread() {
                handle.mark_parked();
            }
            TOKIO_GUARD.with(|slot| *slot.borrow_mut() = Some(guard));
            if let Some(on_start) = on_start {
                on_start();
            }
        })
        .on_thread_stop(|| {
            TOKIO_GUARD.with(|slot| drop(slot.borrow_mut().take()));
        })
        .on_thread_park(|| {
            if let Some(handle) = threads::current_thread() {
                handle.mark_parked();
            }
        })
        .on_thread_unpark(|| {
            if let Some(handle) = threads::current_thread() {
                handle.mark_running();
            }
        })
        .enable_all()
        .build()?;
    let _ = TOKIO_HANDLE.set(runtime.handle().clone());
    Ok(runtime)
}

/// The engine's runtime, once [`build_tokio_runtime`] has run.
pub fn tokio_handle() -> Option<tokio::runtime::Handle> {
    TOKIO_HANDLE.get().cloned()
}

/// What the runtime says about itself.
#[derive(Clone, Copy, Debug)]
pub struct TokioMetrics {
    pub workers: usize,
    pub alive_tasks: usize,
    /// Tasks waiting in the runtime's injection queue.
    pub queue_depth: usize,
}

pub fn tokio_metrics() -> Option<TokioMetrics> {
    let metrics = TOKIO_HANDLE.get()?.metrics();
    Some(TokioMetrics {
        workers: metrics.num_workers(),
        alive_tasks: metrics.num_alive_tasks(),
        queue_depth: metrics.global_queue_depth(),
    })
}
