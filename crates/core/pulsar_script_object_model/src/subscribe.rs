//! Change notification for scripts, over SceneDB's change journals
//! (Pulsar-Native#1035, Phase 5).
//!
//! A script keeps a [`ComponentRefWatch`], watches the [`ComponentRef`]s it
//! cares about, and polls once per tick. Each watch reads through its own
//! cursors, so scripts, panels and the renderer never take changes from one
//! another. Notifications are invalidations: [`ComponentRefWatch::changed`]
//! names the refs whose component changed since the last poll (coalesced),
//! and the script re-reads current state. A watched component that was
//! removed, or whose actor despawned, reports changed; re-reading it is how
//! a script notices its target died. A replaced `World` reports every ref
//! once. Scripts that need every transition use gameplay events.
//!
//! Protocol: watch first, then read the current value.

use pulsar_scenedb::World;
use pulsar_world_registry::ComponentWatch;

use crate::refs::ComponentRef;

/// The refs one script watches, each through this watch's own cursors.
#[derive(Default)]
pub struct ComponentRefWatch {
    watch: ComponentWatch<ComponentRef>,
}

impl ComponentRefWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Watch one referenced component. `false` when the actor is already
    /// dead, has no such instance, or the class isn't registered for live
    /// World residency -- never panics (#641).
    pub fn watch(&mut self, world: &World, r: &ComponentRef) -> bool {
        let Some(instance) = r.instance(world) else {
            return false;
        };
        self.watch
            .watch_class(world, r.clone(), instance, &r.class_name)
    }

    /// Stop watching `r`. `false` when it was not watched.
    pub fn unwatch(&mut self, r: &ComponentRef) -> bool {
        self.watch.unwatch(r)
    }

    pub fn is_watching(&self, r: &ComponentRef) -> bool {
        self.watch.is_watching(r)
    }

    /// The watched refs whose component changed since the previous call,
    /// each once.
    pub fn changed(&mut self, world: &World) -> Vec<ComponentRef> {
        self.watch.poll(world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ScriptRefError;
    use crate::test_support::TestGizmo;

    /// #640 acceptance: set_property through a ref invalidates exactly that
    /// ref -- not a sibling entity's component of the same class.
    #[test]
    fn setting_through_a_ref_reports_that_target_only() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();

        let ref_a = ComponentRef::live(a.into(), "TestGizmo");
        let ref_b = ComponentRef::live(b.into(), "TestGizmo");
        world.insert(a, TestGizmo { charges: 1 });
        world.insert(b, TestGizmo { charges: 2 });

        let mut watch = ComponentRefWatch::new();
        assert!(watch.watch(&world, &ref_a));
        assert!(watch.watch(&world, &ref_b));

        ref_a
            .set_property(&mut world, "charges", Box::new(42))
            .unwrap();

        assert_eq!(watch.changed(&world), vec![ref_a.clone()]);
        assert!(watch.changed(&world).is_empty());

        // The write landed on A only.
        assert_eq!(world.get::<TestGizmo>(a).unwrap().charges, 42);
        assert_eq!(world.get::<TestGizmo>(b).unwrap().charges, 2);
    }

    /// Two scripts watching the same ref both see the change, whichever
    /// reads first.
    #[test]
    fn two_scripts_each_see_the_change() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 1 });
        let r = ComponentRef::live(e.into(), "TestGizmo");

        let mut first = ComponentRefWatch::new();
        let mut second = ComponentRefWatch::new();
        first.watch(&world, &r);
        second.watch(&world, &r);

        r.set_property(&mut world, "charges", Box::new(7)).unwrap();
        assert_eq!(second.changed(&world), vec![r.clone()]);
        assert_eq!(first.changed(&world), vec![r.clone()]);
    }

    /// A despawned target reports changed once; re-reading it is a typed
    /// error, not a panic (#641).
    #[test]
    fn despawn_reports_changed_and_later_writes_are_typed_errors() {
        let mut world = World::new();
        let e = world.spawn();
        let r = ComponentRef::live(e.into(), "TestGizmo");
        world.insert(e, TestGizmo { charges: 5 });

        let mut watch = ComponentRefWatch::new();
        assert!(watch.watch(&world, &r));
        r.actor().despawn(&mut world);

        assert_eq!(watch.changed(&world), vec![r.clone()]);

        let err = r
            .set_property(&mut world, "charges", Box::new(1))
            .unwrap_err();
        assert!(matches!(err, ScriptRefError::ReferenceDespawned { .. }));
    }

    /// Watching an unregistered class is `false`, not a panic.
    #[test]
    fn watching_an_unregistered_class_is_false() {
        let mut world = World::new();
        let e = world.spawn();
        let r = ComponentRef::live(e.into(), "NeverRegistered");
        let mut watch = ComponentRefWatch::new();
        assert!(!watch.watch(&world, &r));
        assert!(!watch.is_watching(&r));
    }
}
