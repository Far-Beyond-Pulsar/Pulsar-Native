//! The session scope a lifecycle callback runs in.
//!
//! A session is one running (or edited) world: the editor's own scene, a
//! Play-in-Editor game on it, a standalone game or a headless run. It
//! carries the [`SessionKind`], the level's settings, and a bag of resources
//! a plugin registers by type. Callbacks reach it through
//! [`ActorCx::session`](crate::ActorCx::session).
//!
//! The scope is the data half; `pulsar_world_registry::Session` (which
//! `ComponentRuntimeState` aliases) owns one beside the lifecycle bookkeeping
//! that needs the world's change journals.

use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::HashMap;

/// What kind of session a callback runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionKind {
    /// The editor's own world, being edited. Only `construct` is delivered;
    /// no `begin_play`, `tick` or `end_play`.
    Editor,
    /// A Play-in-Editor game running on the editor's world.
    PlayInEditor,
    /// A shipped game (windowed).
    Standalone,
    /// A run with no window or renderer (tests, tools, servers).
    Headless,
}

impl SessionKind {
    /// Whether this session plays the world (`begin_play`, `tick`,
    /// `end_play`), as opposed to editing it.
    pub fn is_playing(self) -> bool {
        !matches!(self, Self::Editor)
    }

    /// Whether this session is the editor's (not a game).
    pub fn is_editor(self) -> bool {
        matches!(self, Self::Editor)
    }
}

/// The session's data: its kind, the level's settings and its resources.
pub struct SessionScope {
    kind: SessionKind,
    settings: Value,
    resources: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl SessionScope {
    pub fn new(kind: SessionKind) -> Self {
        Self {
            kind,
            settings: Value::Null,
            resources: HashMap::new(),
        }
    }

    pub fn kind(&self) -> SessionKind {
        self.kind
    }

    pub fn set_kind(&mut self, kind: SessionKind) {
        self.kind = kind;
    }

    /// The level's `world_settings` object (what a plugin keeps there:
    /// physics' gravity, the input mapping). `Null` until the host filled it.
    /// Read-only to a callback: settings are the level's, not the session's.
    pub fn settings(&self) -> &Value {
        &self.settings
    }

    /// One of the level's settings by key.
    pub fn setting(&self, key: &str) -> Option<&Value> {
        self.settings.get(key)
    }

    /// The host fills it when it has read the level.
    pub fn set_settings(&mut self, settings: Value) {
        self.settings = settings;
    }

    /// The session's `T`, if a plugin registered one.
    pub fn resource<T: Any + Send>(&self) -> Option<&T> {
        self.resources
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref())
    }

    pub fn resource_mut<T: Any + Send>(&mut self) -> Option<&mut T> {
        self.resources
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut())
    }

    /// Register `value`, returning the one it replaced.
    pub fn insert_resource<T: Any + Send>(&mut self, value: T) -> Option<T> {
        self.resources
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|old| old.downcast().ok().map(|boxed| *boxed))
    }

    /// The session's `T`, built by `make` on first use. It lives until the
    /// session ends, and its `Drop` is the plugin's shutdown hook (a service
    /// thread joins there).
    pub fn get_or_insert_with<T: Any + Send>(&mut self, make: impl FnOnce() -> T) -> &mut T {
        self.resources
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(make()))
            .downcast_mut()
            .expect("a resource is stored under its own type id")
    }

    pub fn remove_resource<T: Any + Send>(&mut self) -> Option<T> {
        self.resources
            .remove(&TypeId::of::<T>())
            .and_then(|old| old.downcast().ok().map(|boxed| *boxed))
    }

    /// How many resources are registered.
    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }

    /// Drop every resource (the session ended, or its world was replaced).
    pub fn clear_resources(&mut self) {
        self.resources.clear();
    }
}

impl Default for SessionScope {
    fn default() -> Self {
        Self::new(SessionKind::Headless)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Counter(u32);
    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn resources_are_found_by_type_and_dropped_with_the_scope() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut scope = SessionScope::new(SessionKind::Standalone);
        assert!(scope.resource::<Counter>().is_none());
        scope.get_or_insert_with(|| Counter(1)).0 += 1;
        assert_eq!(scope.get_or_insert_with(|| Counter(100)).0, 2);
        scope.insert_resource(Guard(dropped.clone()));
        assert_eq!(scope.resource_count(), 2);
        scope.clear_resources();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert_eq!(scope.resource_count(), 0);
    }

    #[test]
    fn settings_read_by_key() {
        let mut scope = SessionScope::default();
        assert_eq!(scope.kind(), SessionKind::Headless);
        assert!(scope.setting("gravity").is_none());
        scope.set_settings(serde_json::json!({ "gravity": -9.8 }));
        assert_eq!(scope.setting("gravity").and_then(Value::as_f64), Some(-9.8));
    }

    #[test]
    fn only_the_editor_does_not_play() {
        assert!(!SessionKind::Editor.is_playing());
        for kind in [
            SessionKind::PlayInEditor,
            SessionKind::Standalone,
            SessionKind::Headless,
        ] {
            assert!(kind.is_playing());
        }
    }
}
