//! Thread Nodes
//!
//! Nodes for spawning, joining, and managing threads in Pulsar blueprints.
//!
//! # Node Category: Thread
//!
//! Provides concurrency primitives for parallel execution and synchronization.

use std::thread;
use std::time::Duration;

use crate::blueprint;

/// Spawn a new thread to execute a function.
///
/// # Inputs
/// - `input`: Value to pass to the thread (must be `Send + 'static`)
/// - `f`: Function to execute in the thread (must be `FnOnce(T) -> R + Send + 'static`)
///
/// # Returns
/// A handle to the spawned thread.
///
/// # Thread Spawn
/// Spawns a new thread to run the provided function with the given input.
#[blueprint(type: crate::NodeTypes::fn_, category: "Thread")]
pub fn spawn_thread<T, R, F>(input: T, f: F) -> thread::JoinHandle<R>
where
    T: Send + 'static,
    R: Send + 'static,
    F: FnOnce(T) -> R + Send + 'static,
{
    thread::spawn(move || f(input))
}

/// Wait for a thread to finish and retrieve its result.
///
/// # Inputs
/// - `handle`: The thread handle to join
///
/// # Returns
/// The result produced by the thread.
///
/// # Thread Join
/// Waits for the thread to finish and returns its result.
#[blueprint(type: crate::NodeTypes::fn_, category: "Thread")]
pub fn join_thread<R>(handle: thread::JoinHandle<R>) -> R
where
    R: Send + 'static,
{
    handle.join().expect("Thread panicked")
}

/// Pause the current thread for a given number of milliseconds.
///
/// # Inputs
/// - `ms`: Number of milliseconds to sleep
///
/// # Thread Sleep
/// Pauses the current thread for the specified duration.
#[blueprint(type: crate::NodeTypes::fn_, category: "Thread")]
pub fn sleep_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

// More advanced thread nodes (yield, park, etc.) can be added as needed.
