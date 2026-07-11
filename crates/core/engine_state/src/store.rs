//! Generic type-indexed resource store.
//!
//! [`StateStore`] is the core of the arbitrary, type-safe state system: a
//! table holding at most one [`ResourceHandle<T>`] per type `T`. It replaces
//! the pattern of hand-rolled `static GLOBAL_FOO: OnceLock<RwLock<Foo>>`
//! globals scattered across crates.
//!
//! A `StateStore` is a plain, cheaply-`Clone`-able value (internally an
//! `Arc<DashMap<..>>`). Nothing requires it to live in a global — tests and
//! headless tools can construct their own — but [`crate::EngineContext`]
//! owns one (`EngineContext::store`) and exposes it everywhere via
//! `EngineContext::global()`.
//!
//! # Example
//!
//! ```
//! use engine_state::StateStore;
//!
//! #[derive(Clone, Default)]
//! struct GizmoSettings {
//!     snap_translation: f32,
//! }
//!
//! let store = StateStore::new();
//!
//! // First access anywhere creates the resource via `Default`.
//! let gizmo = store.get_or_init::<GizmoSettings>();
//! gizmo.update(|g| g.snap_translation = 0.5);
//!
//! // Any other holder of the store sees the same value.
//! let gizmo2 = store.get_or_init::<GizmoSettings>();
//! assert_eq!(gizmo2.read().snap_translation, 0.5);
//! ```

use crate::resource::{Resource, ResourceHandle};
use dashmap::DashMap;
use std::any::{Any, TypeId};
use std::sync::Arc;

/// A type-indexed table of [`ResourceHandle<T>`]s — at most one per type.
#[derive(Clone, Default)]
pub struct StateStore {
    slots: Arc<DashMap<TypeId, Box<dyn Any + Send + Sync>>>,
}

impl StateStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the resource of type `T`, returning its handle.
    ///
    /// Note: existing clones of a *previous* handle for `T` (obtained before
    /// this call) will keep referring to the old value. Prefer
    /// [`Self::get_or_init`] + [`ResourceHandle::set`] if other holders
    /// should observe the change.
    pub fn insert<T: Resource>(&self, value: T) -> ResourceHandle<T> {
        let handle = ResourceHandle::new(value);
        self.slots
            .insert(TypeId::of::<T>(), Box::new(handle.clone()));
        handle
    }

    /// Get the handle for `T`, creating it via [`Default`] if absent.
    ///
    /// This is the primary entry point: any code, anywhere, can call this to
    /// get a shared handle to its resource type without any prior
    /// registration step.
    pub fn get_or_init<T: Resource + Default>(&self) -> ResourceHandle<T> {
        self.slots
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(ResourceHandle::<T>::new(T::default())))
            .downcast_ref::<ResourceHandle<T>>()
            .expect("StateStore: TypeId collision")
            .clone()
    }

    /// Get the handle for `T` only if it has already been inserted or
    /// initialized.
    pub fn get<T: Resource>(&self) -> Option<ResourceHandle<T>> {
        self.slots.get(&TypeId::of::<T>()).map(|b| {
            b.downcast_ref::<ResourceHandle<T>>()
                .expect("StateStore: TypeId collision")
                .clone()
        })
    }

    /// Whether a resource of type `T` has been inserted/initialized.
    pub fn contains<T: Resource>(&self) -> bool {
        self.slots.contains_key(&TypeId::of::<T>())
    }

    /// Remove and return the handle for `T`, if present.
    ///
    /// Existing clones of the handle remain valid (they keep their own
    /// `Arc`), but future calls to [`Self::get_or_init`] will create a fresh
    /// resource.
    pub fn remove<T: Resource>(&self) -> Option<ResourceHandle<T>> {
        self.slots.remove(&TypeId::of::<T>()).map(|(_, b)| {
            *b.downcast::<ResourceHandle<T>>()
                .expect("StateStore: TypeId collision")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, Clone, PartialEq, Debug)]
    struct Foo {
        value: i32,
    }

    #[derive(Default, Clone, PartialEq, Debug)]
    struct Bar {
        name: String,
    }

    #[test]
    fn get_or_init_creates_default_once() {
        let store = StateStore::new();
        assert!(!store.contains::<Foo>());

        let a = store.get_or_init::<Foo>();
        assert_eq!(*a.read(), Foo::default());

        a.update(|f| f.value = 5);

        let b = store.get_or_init::<Foo>();
        assert_eq!(b.read().value, 5);
        assert!(store.contains::<Foo>());
    }

    #[test]
    fn distinct_types_are_independent() {
        let store = StateStore::new();
        store.get_or_init::<Foo>().update(|f| f.value = 1);
        store
            .get_or_init::<Bar>()
            .update(|b| b.name = "hello".into());

        assert_eq!(store.get::<Foo>().unwrap().read().value, 1);
        assert_eq!(store.get::<Bar>().unwrap().read().name, "hello");
    }

    #[test]
    fn insert_and_remove() {
        let store = StateStore::new();
        store.insert(Foo { value: 42 });
        assert_eq!(store.get::<Foo>().unwrap().read().value, 42);

        let removed = store.remove::<Foo>().unwrap();
        assert_eq!(removed.read().value, 42);
        assert!(!store.contains::<Foo>());
    }

    #[test]
    fn get_returns_none_when_absent() {
        let store = StateStore::new();
        assert!(store.get::<Foo>().is_none());
    }

    #[test]
    fn store_clone_shares_state() {
        let store = StateStore::new();
        let other = store.clone();

        store.get_or_init::<Foo>().update(|f| f.value = 9);
        assert_eq!(other.get::<Foo>().unwrap().read().value, 9);
    }
}
