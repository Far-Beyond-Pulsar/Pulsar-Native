//! Tokio runtime initialization for Pulsar Engine
//
// This module provides a helper for creating the Tokio runtime.

use tokio::runtime::Runtime;

/// Create the engine's multi-threaded Tokio runtime.
///
/// It is built by `editor_task_queue::pools`, which registers every worker
/// with the thread registry (Tasks window, Threads view) and records its
/// parked/running state. Code that needs async work and has no runtime should
/// use `editor_task_queue::pools::tokio_handle()` rather than build its own.
pub fn create_runtime() -> Runtime {
    editor_task_queue::pools::build_tokio_runtime(
        8,
        Some(|| {
            // Name Tokio worker threads for profiling
            let thread_id = std::thread::current().id();
            profiling::set_thread_name(&format!("Tokio Worker {:?}", thread_id));
        }),
    )
    .expect("failed to build the engine's tokio runtime")
}

/// Make rayon's global pool and Helio's task host the engine's: their
/// threads are created through the thread registry. Must run before anything
/// uses rayon or starts a renderer.
pub fn install_thread_hosts() {
    if let Err(error) = editor_task_queue::pools::install_rayon_global(None) {
        tracing::warn!("{error}; its threads will not be listed in the Tasks window");
    }
    // Helio's and the plugins' feature and world crates start their threads
    // and jobs (voxel planners, pipeline compiles) through `helio_tasks`.
    editor_task_queue::helio::install();
}
