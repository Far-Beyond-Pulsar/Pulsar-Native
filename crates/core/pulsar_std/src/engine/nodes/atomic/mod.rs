//! # Atomic Module
//!
//! Atomic operations for the Pulsar visual programming system.
//!
//! This module provides thread-safe atomic operations including:
//! - AtomicBool operations (new, load, store)
//! - AtomicI32 operations (new, load, store, add)
//!
//! All operations use Rust's standard library atomic types from `std::sync::atomic`.

use crate::blueprint;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

// =============================================================================
// AtomicBool Operations
// =============================================================================

/// Creates a new atomic boolean.
///
/// # Inputs
/// - `value`: The initial value for the atomic boolean
///
/// # Returns
/// An AtomicBool initialized with the given value
///
/// # Example
/// If `value` is true, the output will be an atomic boolean set to true.
///
/// # Notes
/// Atomic booleans are useful for thread-safe flags and synchronization.
/// # Atomic Bool New
/// Creates a new atomic boolean value.
#[blueprint(type: NodeTypes::pure, category: "Atomic", color: "#E74C3C")]
pub fn atomic_bool_new(value: bool) -> AtomicBool {
    AtomicBool::new(value)
}

/// Loads the value of an atomic boolean.
///
/// # Inputs
/// - `atomic`: The atomic boolean to load
///
/// # Returns
/// The current value of the atomic boolean (true or false)
///
/// # Example
/// If the atomic boolean contains true, the output will be true.
///
/// # Notes
/// Uses relaxed memory ordering for performance. For stricter ordering, modify as needed.
/// # Atomic Bool Load
/// Retrieves the current value of an atomic boolean.
#[blueprint(type: NodeTypes::pure, category: "Atomic", color: "#E74C3C")]
pub fn atomic_bool_load(atomic: AtomicBool) -> bool {
    atomic.load(Ordering::Relaxed)
}

/// Stores a value in an atomic boolean.
///
/// # Inputs
/// - `atomic`: The atomic boolean to store the value in
/// - `value`: The boolean value to store
///
/// # Returns
/// No return value (the atomic is updated in place)
///
/// # Example
/// If `atomic` is false and `value` is true, the atomic boolean will be set to true.
///
/// # Notes
/// The atomic boolean is updated with the new value using relaxed memory ordering.
/// # Atomic Bool Store
/// Sets a value in an atomic boolean.
#[blueprint(type: NodeTypes::impure, category: "Atomic", color: "#E74C3C")]
pub fn atomic_bool_store(atomic: AtomicBool, value: bool) {
    atomic.store(value, Ordering::Relaxed);
}

// =============================================================================
// AtomicI32 Operations
// =============================================================================

/// Creates a new atomic i32.
///
/// # Inputs
/// - `value`: The initial value for the atomic i32
///
/// # Returns
/// An AtomicI32 initialized with the provided value
///
/// # Example
/// If `value` is 42, the output will be an atomic i32 containing 42.
///
/// # Notes
/// Atomic types are useful for thread-safe shared state and synchronization.
/// # Atomic I32 New
/// Creates a new atomic i32 value.
#[blueprint(type: NodeTypes::pure, category: "Atomic", color: "#E74C3C")]
pub fn atomic_i32_new(value: i64) -> AtomicI32 {
    AtomicI32::new(value as i32)
}

/// Loads the value of an atomic i32.
///
/// # Inputs
/// - `atomic`: The atomic i32 variable to load from
///
/// # Returns
/// The current value of the atomic i32
///
/// # Example
/// If the atomic i32 contains 42, the output will be 42.
///
/// # Notes
/// Use this node for thread-safe reads of integer values shared between threads.
/// Uses relaxed memory ordering for fast, non-synchronized access.
/// # Atomic I32 Load
/// Retrieves the current value of an atomic i32.
#[blueprint(type: NodeTypes::pure, category: "Atomic", color: "#E74C3C")]
pub fn atomic_i32_load(atomic: AtomicI32) -> i32 {
    atomic.load(Ordering::Relaxed)
}

/// Stores a value in an atomic i32.
///
/// # Inputs
/// - `atomic`: The atomic i32 to store the value in
/// - `value`: The value to store (converted to i32)
///
/// # Returns
/// No return value (the atomic is updated in place)
///
/// # Example
/// If `atomic` contains 5 and `value` is 42, after execution the atomic will contain 42.
///
/// # Notes
/// Uses relaxed ordering. For stricter synchronization, use a different ordering if needed.
/// # Atomic I32 Store
/// Sets a value in an atomic i32.
#[blueprint(type: NodeTypes::impure, category: "Atomic", color: "#E74C3C")]
pub fn atomic_i32_store(atomic: AtomicI32, value: i64) {
    atomic.store(value as i32, Ordering::Relaxed);
}

/// Atomically adds a value to an atomic i32 and returns the previous value.
///
/// # Inputs
/// - `atomic`: The atomic i32 to modify
/// - `value`: The value to add
///
/// # Returns
/// The previous value of the atomic i32 before the addition
///
/// # Example
/// If the atomic value is 5 and you add 3, the output will be 5 and the atomic value will become 8.
///
/// # Notes
/// Uses Relaxed ordering for performance. For stricter memory guarantees, use other orderings as needed.
/// The operation is thread-safe and returns the value before the addition.
/// # Atomic I32 Add
/// Atomically adds to an i32 value and returns the previous value.
#[blueprint(type: NodeTypes::impure, category: "Atomic", color: "#E74C3C")]
pub fn atomic_i32_add(atomic: AtomicI32, value: i64) -> i32 {
    atomic.fetch_add(value as i32, Ordering::Relaxed)
}
