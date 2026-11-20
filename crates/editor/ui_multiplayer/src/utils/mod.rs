//! Types and formatting helpers of the multiplayer window.

pub mod format;
pub mod types;

/// Runs `future` to completion on a fresh runtime, for the plain worker
/// threads that serve manifests and files. If the runtime cannot be created
/// (for example the OS is out of threads) the failure is logged and the work
/// is skipped, instead of panicking the worker.
pub fn block_on_new_runtime<F: std::future::Future>(future: F) -> Option<F::Output> {
    match tokio::runtime::Runtime::new() {
        Ok(runtime) => Some(runtime.block_on(future)),
        Err(e) => {
            tracing::error!("Could not create a runtime for a sync worker: {}", e);
            None
        }
    }
}
