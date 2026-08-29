//! Change-notification helpers over SceneDB#47's World subscriptions.
//!
//! Scripts react to changes the same way the properties panel does:
//! subscribe to `(entity, ComponentId)` and drain batched change events.
//! These helpers just translate a [`ComponentRef`]'s class name into its
//! erased [`pulsar_scenedb::ComponentId`] (via `pulsar_world_registry`,
//! same as the panel) so scripts never touch raw ids themselves.
//!
//! Delivery semantics are SceneDB#47's own: at-least-once per real
//! mutation, in order, not coalesced; `Mut`-guard writes fire only on real
//! changes. Despawn auto-unsubscribes with a final `Removed` event, which
//! is how a script notices its watched target died.

use pulsar_scenedb::{ComponentChangeEvent, SubscriptionId, World};

use crate::refs::ComponentRef;

/// Watch one referenced component for changes. Returns `None` when the
/// actor is already dead or the class isn't registered for live World
/// residency -- never panics (#641).
pub fn subscribe_component(world: &mut World, r: &ComponentRef) -> Option<SubscriptionId> {
    let cid = pulsar_world_registry::component_id_for_class(&r.class_name)?;
    world.subscribe_id(r.entity, cid)
}

/// Drain this world's queued change events, keeping only those belonging to
/// `subscription`. Other subscriptions' events are preserved in the queue --
/// each subscription sees exactly its own events on its own drain.
///
/// Note the queue is drained globally per `World`; consumers with many live
/// subscriptions should prefer draining everything once per tick and
/// routing by [`ComponentChangeEvent::subscription`] themselves.
pub fn take_change_events_for(
    world: &mut World,
    subscription: SubscriptionId,
) -> Vec<ComponentChangeEvent> {
    world
        .take_component_change_events()
        .into_iter()
        .filter(|event| event.subscription == subscription)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ScriptRefError;
    use crate::test_support::TestGizmo;

    /// #640 acceptance: set_property through a ref fires exactly one
    /// Mutated event for the referenced component -- and none for a sibling
    /// entity's component of the same class.
    #[test]
    fn setting_through_a_ref_fires_subscription_events_for_that_target_only() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();

        let ref_a = ComponentRef::live(a.into(), "TestGizmo");
        let ref_b = ComponentRef::live(b.into(), "TestGizmo");
        world.insert(a, TestGizmo { charges: 1 });
        world.insert(b, TestGizmo { charges: 2 });

        let sub_a = subscribe_component(&mut world, &ref_a).expect("subscribes");
        let _sub_b = subscribe_component(&mut world, &ref_b).expect("subscribes");

        ref_a.set_property(&mut world, "charges", serde_json::json!(42)).unwrap();

        let events = take_change_events_for(&mut world, sub_a);
        assert_eq!(events.len(), 1, "exactly one event for sub_a");
        assert_eq!(events[0].entity, a);
        assert_eq!(events[0].kind, pulsar_scenedb::ComponentChangeKind::Mutated);

        // The write landed on A only.
        assert_eq!(world.get::<TestGizmo>(a).unwrap().charges, 42);
        assert_eq!(world.get::<TestGizmo>(b).unwrap().charges, 2);
    }

    /// Subscriptions die with their target: after despawn, the watcher gets
    /// a final Removed event instead of silence or a panic (#641).
    #[test]
    fn despawn_delivers_removed_and_later_writes_are_typed_errors() {
        let mut world = World::new();
        let e = world.spawn();
        let r = ComponentRef::live(e.into(), "TestGizmo");
        world.insert(e, TestGizmo { charges: 5 });

        let sub = subscribe_component(&mut world, &r).expect("subscribes");
        r.actor().despawn(&mut world);

        let events = take_change_events_for(&mut world, sub);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, pulsar_scenedb::ComponentChangeKind::Removed);
        assert_eq!(events[0].entity, e);

        let err = r.set_property(&mut world, "charges", serde_json::json!(1)).unwrap_err();
        assert!(matches!(err, ScriptRefError::ReferenceDespawned { .. }));
    }

    /// Subscribing to an unregistered class is `None`, not a panic.
    #[test]
    fn subscribing_to_unregistered_class_is_none() {
        let mut world = World::new();
        let e = world.spawn();
        let r = ComponentRef::live(e.into(), "NeverRegistered");
        assert!(subscribe_component(&mut world, &r).is_none());
    }
}
