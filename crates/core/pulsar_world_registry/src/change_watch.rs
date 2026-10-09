//! Per-subscriber change watching over SceneDB's change journals
//! (Pulsar-Native#1035, Phase 5).
//!
//! A [`ComponentWatch`] follows a set of `(entity, component)` pairs, each
//! under a caller-chosen key, through its own [`ChangeCursor`]s -- one per
//! component type. Reading never consumes anything another reader needs:
//! several scripts each hold their own watch and see every change in their
//! own order. Reading needs only `&World`. A view that displays one object
//! (the properties panel) follows it through an
//! [`ObjectFeed`](crate::ObjectFeed) instead, which delivers values.
//!
//! Notifications are invalidations, not payloads: [`ComponentWatch::poll`]
//! returns the keys whose component changed since the last poll, and the
//! caller re-reads the current value. Changes are coalesced per key. When a
//! journal overflowed, or the watched `World` was replaced (undo/redo, level
//! load), every key on that component type comes back, which is the
//! "resync required" signal: re-reading everything is the recovery.
//! Callers that need every transition rather than the latest state use
//! gameplay or component events instead.
//!
//! Protocol: [`ComponentWatch::watch`] first, then read the current state.
//! The cursor is open before the read, so a change landing between the two
//! is reported on the next poll rather than lost.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use pulsar_scenedb::{ChangeCursor, ChangeRead, ComponentChange, ComponentId, Entity, World};

/// Watches `(entity, component)` pairs under keys `K` through independent
/// change cursors. See the module doc.
pub struct ComponentWatch<K> {
    cursors: HashMap<ComponentId, ChangeCursor>,
    targets: HashMap<(Entity, ComponentId), Vec<K>>,
    keys: HashMap<K, (Entity, ComponentId)>,
    scratch: Vec<ComponentChange>,
}

impl<K> Default for ComponentWatch<K> {
    fn default() -> Self {
        Self {
            cursors: HashMap::new(),
            targets: HashMap::new(),
            keys: HashMap::new(),
            scratch: Vec::new(),
        }
    }
}

impl<K: Clone + Eq + Hash> ComponentWatch<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Watch `component` on `entity` under `key`, replacing whatever `key`
    /// watched before. Opens a cursor for the component type if this watch
    /// has none yet. Read the current state after this call.
    pub fn watch(&mut self, world: &World, key: K, entity: Entity, component: ComponentId) {
        self.unwatch(&key);
        self.cursors
            .entry(component)
            .or_insert_with(|| world.open_change_cursor_id(component));
        self.targets
            .entry((entity, component))
            .or_default()
            .push(key.clone());
        self.keys.insert(key, (entity, component));
    }

    /// Watch the component registered for `class_name` on `entity`. `false`
    /// (and nothing watched) when the class has no World component.
    pub fn watch_class(&mut self, world: &World, key: K, entity: Entity, class_name: &str) -> bool {
        match crate::component_id_for_class(class_name) {
            Some(component) => {
                self.watch(world, key, entity, component);
                true
            }
            None => false,
        }
    }

    /// Stop watching `key`. `false` when it was not watched. A component
    /// type's cursor closes with its last key.
    pub fn unwatch(&mut self, key: &K) -> bool {
        let Some(target) = self.keys.remove(key) else {
            return false;
        };
        if let Some(keys) = self.targets.get_mut(&target) {
            keys.retain(|k| k != key);
            if keys.is_empty() {
                self.targets.remove(&target);
            }
        }
        if !self
            .keys
            .values()
            .any(|(_, component)| *component == target.1)
        {
            self.cursors.remove(&target.1);
        }
        true
    }

    /// Stop watching everything.
    pub fn clear(&mut self) {
        self.cursors.clear();
        self.targets.clear();
        self.keys.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_watching(&self, key: &K) -> bool {
        self.keys.contains_key(key)
    }

    /// The keys whose component changed (inserted, written or removed)
    /// since the previous poll, each once. On an overflowed journal or a
    /// replaced `World`, every key on that component type.
    pub fn poll(&mut self, world: &World) -> Vec<K> {
        let mut changed = Vec::new();
        let mut seen = HashSet::new();
        for (component, cursor) in &mut self.cursors {
            self.scratch.clear();
            match world.read_changes(cursor, &mut self.scratch) {
                ChangeRead::Complete => {
                    for change in &self.scratch {
                        if let Some(keys) = self.targets.get(&(change.entity, *component)) {
                            for key in keys {
                                if seen.insert(key.clone()) {
                                    changed.push(key.clone());
                                }
                            }
                        }
                    }
                }
                ChangeRead::Overflowed => {
                    for (key, (_, c)) in &self.keys {
                        if c == component && seen.insert(key.clone()) {
                            changed.push(key.clone());
                        }
                    }
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scenedb::component_id;

    #[derive(Clone, Debug, PartialEq)]
    struct Health(i32);
    #[derive(Clone, Debug, PartialEq)]
    struct Mana(i32);

    fn sorted(mut keys: Vec<&'static str>) -> Vec<&'static str> {
        keys.sort();
        keys
    }

    #[test]
    fn two_watchers_see_the_same_changes_in_any_read_order() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        world.insert(a, Health(1));
        world.insert(b, Health(2));

        let mut panel = ComponentWatch::new();
        let mut script = ComponentWatch::new();
        panel.watch(&world, "a", a, component_id::<Health>());
        panel.watch(&world, "b", b, component_id::<Health>());
        script.watch(&world, "a", a, component_id::<Health>());

        world.insert(a, Health(10));
        world.insert(a, Health(11));

        assert_eq!(script.poll(&world), vec!["a"], "coalesced per key");
        assert_eq!(
            panel.poll(&world),
            vec!["a"],
            "the script's read took nothing"
        );
        assert!(panel.poll(&world).is_empty());

        world.insert(b, Health(20));
        world.remove::<Health>(a);
        assert_eq!(sorted(panel.poll(&world)), vec!["a", "b"]);
        assert_eq!(script.poll(&world), vec!["a"], "removal invalidates too");
    }

    /// Observer fanout (Pulsar-Native#1035 acceptance, #1081): watchers
    /// polling at different rates (every write, every seventh, once at the
    /// end) each learn of every key written since their previous poll, and
    /// an object feed receives every write's value, whatever the others do.
    #[test]
    fn watchers_at_different_poll_rates_miss_nothing() {
        let mut world = World::new();
        let entities: Vec<_> = (0..3).map(|_| world.spawn()).collect();
        for &e in &entities {
            world.insert(e, crate::tests::TestComponent { value: 0 });
        }
        let keys = ["a", "b", "c"];
        let mut watchers: Vec<ComponentWatch<&str>> =
            (0..3).map(|_| ComponentWatch::new()).collect();
        for watcher in &mut watchers {
            for (key, &e) in keys.iter().zip(&entities) {
                watcher.watch(
                    &world,
                    *key,
                    e,
                    component_id::<crate::tests::TestComponent>(),
                );
            }
        }
        let feed = crate::ObjectFeed::subscribe(&mut world, entities[1], || {}).unwrap();

        // Each watcher's keys written since its last poll.
        let mut pending: Vec<std::collections::BTreeSet<&str>> = vec![Default::default(); 3];
        let periods = [1usize, 7, usize::MAX];
        let mut written_b = Vec::new();
        for step in 0..100usize {
            let target = (step * 7 + step / 3) % 3;
            world
                .get_mut::<crate::tests::TestComponent>(entities[target])
                .unwrap()
                .value = step as i32;
            if target == 1 {
                written_b.push(step as i32);
            }
            for (watcher, (pending, &period)) in
                watchers.iter_mut().zip(pending.iter_mut().zip(&periods))
            {
                pending.insert(keys[target]);
                if (step + 1) % period == 0 {
                    let got: std::collections::BTreeSet<_> =
                        watcher.poll(&world).into_iter().collect();
                    assert_eq!(&got, pending, "period {period} at step {step}");
                    pending.clear();
                }
            }
        }
        for (watcher, pending) in watchers.iter_mut().zip(&pending) {
            let got: std::collections::BTreeSet<_> = watcher.poll(&world).into_iter().collect();
            assert_eq!(&got, pending, "final poll");
        }
        let delivered: Vec<i32> = feed
            .take()
            .into_iter()
            .filter_map(|update| match update {
                crate::ObjectUpdate::Changed(delta) => delta
                    .value?
                    .downcast_ref::<crate::tests::TestComponent>()
                    .map(|c| c.value),
                crate::ObjectUpdate::Despawned => None,
            })
            .collect();
        assert_eq!(delivered, written_b, "the feed sees every write's value");
    }

    #[test]
    fn other_entities_and_components_stay_quiet() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        world.insert(a, Health(1));
        let mut watch = ComponentWatch::new();
        watch.watch(&world, "a", a, component_id::<Health>());

        world.insert(b, Health(5));
        world.insert(a, Mana(5));
        assert!(watch.poll(&world).is_empty());
    }

    #[test]
    fn a_change_between_watch_and_first_read_is_reported() {
        let mut world = World::new();
        let a = world.spawn();
        let mut watch = ComponentWatch::new();
        watch.watch(&world, "a", a, component_id::<Health>());
        // The caller reads the current state here; a write lands after.
        world.insert(a, Health(3));
        assert_eq!(watch.poll(&world), vec!["a"]);
    }

    #[test]
    fn unwatched_keys_stop_reporting() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut watch = ComponentWatch::new();
        watch.watch(&world, "a", a, component_id::<Health>());
        watch.watch(&world, "b", b, component_id::<Health>());
        assert!(watch.unwatch(&"a"));
        assert!(!watch.unwatch(&"a"));

        world.insert(a, Health(1));
        world.insert(b, Health(1));
        assert_eq!(watch.poll(&world), vec!["b"]);

        watch.unwatch(&"b");
        assert!(watch.is_empty());
        world.insert(b, Health(2));
        assert!(watch.poll(&world).is_empty());
    }

    #[test]
    fn rewatching_a_key_moves_it() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut watch = ComponentWatch::new();
        watch.watch(&world, "k", a, component_id::<Health>());
        watch.watch(&world, "k", b, component_id::<Health>());
        world.insert(a, Health(1));
        assert!(watch.poll(&world).is_empty());
        world.insert(b, Health(1));
        assert_eq!(watch.poll(&world), vec!["k"]);
    }

    #[test]
    fn a_replaced_world_resyncs_every_key_once() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut watch = ComponentWatch::new();
        watch.watch(&world, "a", a, component_id::<Health>());
        watch.watch(&world, "b", b, component_id::<Health>());
        watch.watch(&world, "mana", a, component_id::<Mana>());

        let mut replacement = World::new();
        let a2 = replacement.spawn();
        assert_eq!(sorted(watch.poll(&replacement)), vec!["a", "b", "mana"]);
        assert!(watch.poll(&replacement).is_empty());

        watch.watch(&replacement, "a", a2, component_id::<Health>());
        replacement.insert(a2, Health(1));
        assert_eq!(watch.poll(&replacement), vec!["a"]);
    }

    #[test]
    fn an_overflowed_journal_resyncs_its_keys() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        world.insert(a, Health(0));
        let mut watch = ComponentWatch::new();
        watch.watch(&world, "a", a, component_id::<Health>());
        watch.watch(&world, "b", b, component_id::<Health>());
        watch.watch(&world, "mana", a, component_id::<Mana>());

        for i in 0..(pulsar_scenedb::change_journal::DEFAULT_JOURNAL_CAPACITY as i32 + 1) {
            world.insert(a, Health(i));
        }
        assert_eq!(sorted(watch.poll(&world)), vec!["a", "b"]);
        assert!(watch.poll(&world).is_empty());
    }
}
