//! Mutex Nodes
//!
//! Nodes for synchronization and safe shared state in Pulsar blueprints.
//!
//! # Node Category: Mutex
//!
//! Provides primitives for mutual exclusion and locking.

use std::sync::{Arc, Mutex, MutexGuard};
use crate::blueprint;

/// A safe wrapper that owns both the Arc<Mutex<T>> and its guard.
///
/// This is necessary because MutexGuard borrows from the Mutex, so we need
/// to keep the Arc alive for as long as the guard exists.
pub struct OwnedMutexGuard<T: 'static> {
    // The Arc must be stored to keep the Mutex alive
    _arc: Arc<Mutex<T>>,
    // Safety: This field must come after _arc to ensure proper drop order
    guard: Option<MutexGuard<'static, T>>,
}

impl<T: 'static> OwnedMutexGuard<T> {
    /// Create a new OwnedMutexGuard by locking the provided Arc<Mutex<T>>
    fn new(arc: Arc<Mutex<T>>) -> Self {
        // Lock the mutex while we own the Arc
        let guard = arc.lock().unwrap();

        // SAFETY: We transmute the guard's lifetime to 'static, but this is safe because:
        // 1. We store the Arc in _arc, which keeps the Mutex alive
        // 2. The guard is stored in a private field and cannot outlive self
        // 3. When OwnedMutexGuard is dropped, the guard is dropped before the Arc
        // 4. The Arc is never moved or dropped while the guard exists
        let guard = unsafe { std::mem::transmute::<MutexGuard<'_, T>, MutexGuard<'static, T>>(guard) };

        Self {
            _arc: arc,
            guard: Some(guard),
        }
    }
}

impl<T: 'static> std::ops::Deref for OwnedMutexGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().expect("Guard should always exist until drop")
    }
}

impl<T: 'static> std::ops::DerefMut for OwnedMutexGuard<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().expect("Guard should always exist until drop")
    }
}

impl<T: 'static> Drop for OwnedMutexGuard<T> {
    fn drop(&mut self) {
        // Drop the guard first, then the Arc
        self.guard.take();
        // _arc will be dropped automatically after this
    }
}

/// Create a new mutex wrapping a value.
///
/// # Inputs
/// - `value`: The value to protect with a mutex
///
/// # Returns
/// An `Arc<Mutex<T>>` for shared ownership and locking.
///
/// # Mutex Create
/// Creates a new mutex-protected value.
#[blueprint(type: crate::NodeTypes::pure, category: "Mutex")]
pub fn create_mutex<T>(value: T) -> Arc<Mutex<T>>
where
    T: Send + 'static,
{
    Arc::new(Mutex::new(value))
}

/// Lock a mutex for exclusive access.
///
/// # Inputs
/// - `mutex`: The mutex to lock
///
/// # Returns
/// A guard that allows access to the value.
///
/// # Mutex Lock
/// Locks the mutex and returns a safe owned guard for access.
#[blueprint(type: crate::NodeTypes::fn_, category: "Mutex")]
pub fn lock_mutex<T>(mutex: Arc<Mutex<T>>) -> OwnedMutexGuard<T>
where
    T: Send + 'static,
{
    OwnedMutexGuard::new(mutex)
}

/// Unlock a mutex (drops the guard).
///
/// # Inputs
/// - `guard`: The mutex guard to drop
///
/// # Mutex Unlock
/// Unlocks the mutex by dropping the guard.
#[blueprint(type: crate::NodeTypes::fn_, category: "Mutex")]
pub fn unlock_mutex<T>(_guard: OwnedMutexGuard<T>) {
    // Dropping the guard unlocks the mutex
}
