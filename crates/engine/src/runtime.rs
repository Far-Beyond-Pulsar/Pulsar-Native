//! Tokio runtime initialization for Pulsar Engine
//
// This module provides a helper for creating the Tokio runtime.

use tokio::runtime::Runtime;

/// Create a multi-threaded Tokio runtime for the engine.
pub fn create_runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .thread_name("PulsarEngineRuntime")
        .on_thread_start(|| {
            // Name Tokio worker threads for profiling
            let thread_id = std::thread::current().id();
            profiling::set_thread_name(&format!("Tokio Worker {:?}", thread_id));
        })
        .enable_all()
        .build()
        .unwrap()
}
