//! Generic, per-key resource store.
//!
//! [`KeyedStore<K>`] is [`crate::store::StateStore`]'s sibling for state that
//! is naturally scoped per key — most commonly per-window
//! (`KeyedStore<WindowId>`), but also usable for per-project, per-entity, or
//! any other `Eq + Hash + Clone` key.
//!
//! It replaces ad-hoc `DashMap<K, SomeBespokeWrapper>` registries (e.g. the
//! old `TypedRendererRegistry`) with one generic mechanism: "give me the `T`
//! for this key, creating it if needed."
//!
//! # Example
//!
//! ```
//! use engine_state::KeyedStore;
//!
//! type WindowId = u64;
//!
//! #[derive(Clone, Default)]
//! struct PanelLayout {
//!     sidebar_width: f32,
//! }
//!
//! let windows: KeyedStore<WindowId> = KeyedStore::new();
//!
//! let layout = windows.get_or_init::<PanelLayout>(&1);
//! layout.update(|l| l.sidebar_width = 240.0);
//!
//! assert_eq!(windows.get::<PanelLayout>(&1).unwrap().read().sidebar_width, 240.0);
//! assert!(windows.get::<PanelLayout>(&2).is_none());
//! ```

use crate::resource::{Resource, ResourceHandle};
use dashmap::DashMap;
use std::any::{Any, TypeId};
use std::hash::Hash;
use std::sync::Arc;

/// Like [`crate::store::StateStore`], but every resource type is further
/// keyed by `K` (e.g. a window ID).
#[derive(Clone)]
pub struct KeyedStore<K: Eq + Hash + Clone + Send + Sync + 'static> {
    // TypeId -> Arc<DashMap<K, ResourceHandle<T>>>, type-erased.
    slots: Arc<DashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    _marker: std::marker::PhantomData<fn() -> K>,
}

impl<K: Eq + Hash + Clone + Send + Sync + 'static> Default for KeyedStore<K> {
    fn default() -> Self {
        Self {
            slots: Arc::new(DashMap::new()),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<K: Eq + Hash + Clone + Send + Sync + 'static> KeyedStore<K> {
    /// Create an empty keyed store.
    pub fn new() -> Self {
        Self::default()
    }

    fn table<T: Resource>(&self) -> Arc<DashMap<K, ResourceHandle<T>>> {
        self.slots
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Arc::new(DashMap::<K, ResourceHandle<T>>::new())))
            .downcast_ref::<Arc<DashMap<K, ResourceHandle<T>>>>()
            .expect("KeyedStore: TypeId collision")
            .clone()
    }

    /// Insert or replace the `T` for `key`, returning its handle.
    pub fn insert<T: Resource>(&self, key: K, value: T) -> ResourceHandle<T> {
        let handle = ResourceHandle::new(value);
        self.table::<T>().insert(key, handle.clone());
        handle
    }

    /// Get the handle for `(K, T)`, creating it via [`Default`] if absent.
    pub fn get_or_init<T: Resource + Default>(&self, key: &K) -> ResourceHandle<T> {
        self.table::<T>()
            .entry(key.clone())
            .or_insert_with(|| ResourceHandle::new(T::default()))
            .clone()
    }

    /// Get the handle for `(K, T)` only if it has already been
    /// inserted/initialized.
    pub fn get<T: Resource>(&self, key: &K) -> Option<ResourceHandle<T>> {
        self.table::<T>().get(key).map(|h| h.clone())
    }

    /// Remove and return the handle for `(K, T)`, if present.
    pub fn remove<T: Resource>(&self, key: &K) -> Option<ResourceHandle<T>> {
        self.table::<T>().remove(key).map(|(_, h)| h)
    }

    /// Whether `key` currently holds a `T`.
    pub fn contains<T: Resource>(&self, key: &K) -> bool {
        self.table::<T>().contains_key(key)
    }

    /// All keys that currently hold a `T`.
    pub fn keys<T: Resource>(&self) -> Vec<K> {
        self.table::<T>().iter().map(|e| e.key().clone()).collect()
    }

    /// Remove the `T` table for *all* keys at once (e.g. on project close,
    /// drop all per-window scene state in one call).
    pub fn clear<T: Resource>(&self) {
        self.table::<T>().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type WindowId = u64;

    #[derive(Default, Clone, PartialEq, Debug)]
    struct Layout {
        sidebar_width: f32,
    }

    #[derive(Default, Clone, PartialEq, Debug)]
    struct Title(String);

    #[test]
    fn per_key_isolation() {
        let store: KeyedStore<WindowId> = KeyedStore::new();

        store
            .get_or_init::<Layout>(&1)
            .update(|l| l.sidebar_width = 100.0);
        store
            .get_or_init::<Layout>(&2)
            .update(|l| l.sidebar_width = 200.0);

        assert_eq!(store.get::<Layout>(&1).unwrap().read().sidebar_width, 100.0);
        assert_eq!(store.get::<Layout>(&2).unwrap().read().sidebar_width, 200.0);
    }

    #[test]
    fn distinct_types_per_key() {
        let store: KeyedStore<WindowId> = KeyedStore::new();

        store.insert(
            1,
            Layout {
                sidebar_width: 50.0,
            },
        );
        store.insert(1, Title("hello".into()));

        assert_eq!(store.get::<Layout>(&1).unwrap().read().sidebar_width, 50.0);
        assert_eq!(store.get::<Title>(&1).unwrap().read().0, "hello");
    }

    #[test]
    fn missing_key_returns_none() {
        let store: KeyedStore<WindowId> = KeyedStore::new();
        store.get_or_init::<Layout>(&1);
        assert!(store.get::<Layout>(&2).is_none());
    }

    #[test]
    fn remove_and_clear() {
        let store: KeyedStore<WindowId> = KeyedStore::new();
        store.insert(1, Layout::default());
        store.insert(2, Layout::default());

        assert!(store.remove::<Layout>(&1).is_some());
        assert!(store.get::<Layout>(&1).is_none());
        assert!(store.get::<Layout>(&2).is_some());

        store.clear::<Layout>();
        assert!(store.get::<Layout>(&2).is_none());
    }

    #[test]
    fn keys_lists_only_populated_entries() {
        let store: KeyedStore<WindowId> = KeyedStore::new();
        store.insert(1, Layout::default());
        store.insert(3, Layout::default());

        let mut keys = store.keys::<Layout>();
        keys.sort();
        assert_eq!(keys, vec![1, 3]);
    }
}
