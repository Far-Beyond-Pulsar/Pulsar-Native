//! Generic, reactive resource handle.
//!
//! A [`ResourceHandle<T>`] is a cheaply-cloneable, lockable handle to a
//! single value of type `T`, plus a version counter and a multi-listener
//! change notification primitive. It is the building block for both
//! [`crate::store::StateStore`] (one resource per type) and
//! [`crate::keyed_store::KeyedStore`] (one resource per type *per key*).

use event_listener::{Event, EventListener};
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Marker trait for anything that can live in a [`crate::store::StateStore`]
/// or [`crate::keyed_store::KeyedStore`].
///
/// Blanket-implemented for every `Send + Sync + 'static` type — no derive or
/// manual impl required. Implement `Default` on your type as well if you want
/// it to be lazily created on first access via `get_or_init`.
pub trait Resource: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Resource for T {}

/// A shared, lockable, reactive handle to a single resource value.
///
/// Cloning a `ResourceHandle` is cheap (an `Arc` clone); all clones observe
/// the same underlying value, version counter, and change notifications.
pub struct ResourceHandle<T> {
    value: Arc<RwLock<T>>,
    version: Arc<AtomicU64>,
    event: Arc<Event>,
}

impl<T> Clone for ResourceHandle<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            version: self.version.clone(),
            event: self.event.clone(),
        }
    }
}

impl<T> ResourceHandle<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            value: Arc::new(RwLock::new(value)),
            version: Arc::new(AtomicU64::new(0)),
            event: Arc::new(Event::new()),
        }
    }

    /// Read-only access to the current value. Does not bump the version.
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.value.read()
    }

    /// Mutable access via closure. Bumps the version counter and wakes all
    /// listeners registered via [`Self::changed`] after `f` returns.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let result = {
            let mut guard = self.value.write();
            f(&mut guard)
        };
        self.version.fetch_add(1, Ordering::AcqRel);
        self.event.notify(usize::MAX);
        result
    }

    /// Replace the whole value. Shorthand for `update(|v| *v = new)`.
    pub fn set(&self, new: T) {
        self.update(|v| *v = new);
    }

    /// Snapshot the current value (requires `Clone`).
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.value.read().clone()
    }

    /// Current version counter. Increments on every [`Self::update`] /
    /// [`Self::set`]. Useful for cheaply checking "did this change since
    /// last frame?" without taking a lock.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Direct mutable (RAII) access to the value.
    ///
    /// Bumps the version counter and wakes all [`Self::changed`] listeners
    /// when the returned guard is dropped — unlike `std::sync::RwLock`'s
    /// `write()`, no `.unwrap()` is needed (parking_lot locks don't poison).
    ///
    /// Prefer [`Self::update`] for new code (it makes the mutation's extent
    /// explicit), but `write()` is the drop-in replacement when migrating a
    /// call site that previously held a `RwLockWriteGuard` across several
    /// statements.
    pub fn write(&self) -> WriteGuard<'_, T> {
        WriteGuard {
            guard: self.value.write(),
            version: &self.version,
            event: &self.event,
        }
    }

    /// Returns a future that resolves on the next change.
    ///
    /// Registration happens *synchronously* when this method is called (not
    /// when the returned future is first polled), so a notification fired
    /// immediately after calling `changed()` is never missed:
    ///
    /// ```ignore
    /// let waiter = handle.changed(); // registered now
    /// handle.set(new_value);          // wakes `waiter`
    /// waiter.await;                   // resolves immediately
    /// ```
    ///
    /// Any number of independent callers may register and await
    /// concurrently — each gets its own notification, unlike a
    /// single-consumer channel. As with any "wait for next change" API,
    /// a notification that fires *between* an `await` returning and the next
    /// call to `changed()` can be missed — for state that must observe every
    /// transition, read the value (and act on it) before re-registering.
    pub fn changed(&self) -> EventListener<()> {
        self.event.listen()
    }
}

/// RAII write guard returned by [`ResourceHandle::write`].
///
/// Bumps the resource's version counter and notifies all [`ResourceHandle::changed`]
/// listeners when dropped.
pub struct WriteGuard<'a, T> {
    guard: RwLockWriteGuard<'a, T>,
    version: &'a AtomicU64,
    event: &'a Event,
}

impl<'a, T> Deref for WriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<'a, T> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<'a, T> Drop for WriteGuard<'a, T> {
    fn drop(&mut self) {
        self.version.fetch_add(1, Ordering::AcqRel);
        self.event.notify(usize::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_write_roundtrip() {
        let handle = ResourceHandle::new(42i32);
        assert_eq!(*handle.read(), 42);
        handle.set(7);
        assert_eq!(*handle.read(), 7);
    }

    #[test]
    fn write_guard_mutates_and_bumps_version() {
        let handle = ResourceHandle::new(vec![1, 2]);
        {
            let mut guard = handle.write();
            guard.push(3);
        }
        assert_eq!(*handle.read(), vec![1, 2, 3]);
        assert_eq!(handle.version(), 1);
    }

    #[test]
    fn version_bumps_on_update_only() {
        let handle = ResourceHandle::new(0u32);
        assert_eq!(handle.version(), 0);
        let _ = handle.read();
        assert_eq!(handle.version(), 0);
        handle.update(|v| *v += 1);
        assert_eq!(handle.version(), 1);
        handle.set(10);
        assert_eq!(handle.version(), 2);
    }

    #[test]
    fn clones_share_state() {
        let handle = ResourceHandle::new(String::from("a"));
        let other = handle.clone();
        handle.set("b".to_string());
        assert_eq!(*other.read(), "b");
        assert_eq!(other.version(), 1);
    }

    #[test]
    fn multi_listener_notification() {
        smol::block_on(async {
            let handle = ResourceHandle::new(0i32);

            let h1 = handle.clone();
            let h2 = handle.clone();
            let l1 = h1.changed();
            let l2 = h2.changed();

            handle.set(1);

            l1.await;
            l2.await;
        });
    }
}
