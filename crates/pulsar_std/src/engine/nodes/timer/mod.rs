//! Timer Nodes
//!
//! Nodes for time-based operations and scheduling in Pulsar blueprints.
//!
//! # Node Category: Timer
//!
//! Provides utilities for delays, intervals, and time queries.

use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::blueprint;

/// Execute a node after a delay (in milliseconds).
///
/// # Inputs
/// - `ms`: Delay in milliseconds before execution
///
/// # Timer Set Timeout
/// Executes the connected node after the specified delay.
#[blueprint(type: crate::NodeTypes::fn_, category: "Timer")]
pub fn set_timeout(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
    // In a real async system, this would schedule a callback.
    // Here, it simply blocks for the delay.
}

/// Repeatedly execute a node at intervals (in milliseconds).
///
/// # Inputs
/// - `ms`: Interval in milliseconds
/// - `count`: Number of times to repeat (0 = infinite)
///
/// # Timer Set Interval
/// Executes the connected node at the specified interval.
#[blueprint(type: crate::NodeTypes::fn_, category: "Timer")]
pub fn set_interval(ms: u64, count: u64) {
    let mut i = 0;
    while count == 0 || i < count {
        thread::sleep(Duration::from_millis(ms));
        // In a real async system, this would trigger a callback.
        i += 1;
    }
}

/// Get the current system time in milliseconds since the Unix epoch.
///
/// # Timer Now
/// Returns the current system time in milliseconds.
#[blueprint(type: crate::NodeTypes::pure, category: "Timer")]
pub fn now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}
