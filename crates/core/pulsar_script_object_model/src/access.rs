//! Property and method accessors on [`ComponentRef`].
//!
//! Every attached component instance is its own entity holding its own
//! typed value (Pulsar-Native#1035, D1), so a reference resolves to exactly
//! one instance and every accessor goes straight through that typed value
//! (`pulsar_world_registry`'s dispatcher) -- no JSON copy of "other"
//! instances, no instance-zero aliasing. Every accessor:
//! 1. validates actor liveness (`ReferenceDespawned`),
//! 2. checks class registration (`UnregisteredClass`),
//! 3. resolves `(class_name, component_index)` -- the actor's
//!    `component_index`-th instance of the class -- to its instance entity
//!    (`ComponentMissing`/`InstanceMissing` when absent),
//! 4. fails with a typed error rather than panicking or guessing (#641).
//!
//! Writes go through SceneDB's write guard, so change events and GPU rows
//! follow exactly as for properties-panel edits.

use pulsar_reflection::{MethodArgs, MethodReturnValue};
use pulsar_scenedb::World;

use crate::errors::ScriptRefError;
use crate::refs::ComponentRef;

impl ComponentRef {
    /// Read one property of the referenced component instance, typed (as
    /// the property's getter returns it).
    pub fn get_property(
        &self,
        world: &World,
        property: &str,
    ) -> Result<Box<dyn std::any::Any>, ScriptRefError> {
        self.validate(world)?;
        pulsar_world_registry::get_component_property_boxed(
            world,
            self.entity,
            &self.class_name,
            self.component_index,
            property,
        )
    }

    /// Write one property of the referenced component instance from a
    /// typed value. Its type is checked against the property's first;
    /// nothing is written on failure.
    pub fn set_property(
        &self,
        world: &mut World,
        property: &str,
        value: Box<dyn std::any::Any>,
    ) -> Result<(), ScriptRefError> {
        self.validate(world)?;
        pulsar_world_registry::set_component_property_boxed(
            world,
            self.entity,
            &self.class_name,
            self.component_index,
            property,
            value,
        )
    }

    /// Invoke a reflected method on the referenced component instance.
    pub fn call_method(
        &self,
        world: &mut World,
        method: &str,
        args: MethodArgs,
    ) -> Result<MethodReturnValue, ScriptRefError> {
        pulsar_world_registry::invoke_component_method(
            world,
            self.entity,
            &self.class_name,
            self.component_index,
            method,
            args,
        )
    }

    /// The instance entity this reference addresses right now, if any.
    pub fn instance(&self, world: &World) -> Option<pulsar_scenedb::Entity> {
        pulsar_world_registry::instances::resolve_instance(
            world,
            self.entity,
            &self.class_name,
            self.component_index,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestGizmo;
    use pulsar_scenedb::Entity;

    fn actor(e: Entity) -> crate::refs::ActorRef {
        crate::refs::ActorRef(e)
    }

    #[test]
    fn refs_across_duplicate_class_entities_write_only_their_own_target() {
        let mut world = World::new();
        let door = world.spawn();
        let chest = world.spawn();
        world.insert(door, TestGizmo { charges: 10 });
        world.insert(chest, TestGizmo { charges: 20 });

        let door_ref = ComponentRef::live(actor(door), "TestGizmo");
        let chest_ref = ComponentRef::live(actor(chest), "TestGizmo");

        door_ref
            .set_property(&mut world, "charges", Box::new(11))
            .unwrap();
        chest_ref
            .set_property(&mut world, "charges", Box::new(22))
            .unwrap();

        assert_eq!(world.get::<TestGizmo>(door).unwrap().charges, 11);
        assert_eq!(world.get::<TestGizmo>(chest).unwrap().charges, 22);
        assert_eq!(
            door_ref
                .get_property(&world, "charges")
                .unwrap()
                .downcast_ref::<i32>()
                .copied(),
            Some(11)
        );
        assert_eq!(
            chest_ref
                .get_property(&world, "charges")
                .unwrap()
                .downcast_ref::<i32>()
                .copied(),
            Some(22)
        );
    }

    /// #640 acceptance: refs held across a despawn (and slot churn) report
    /// staleness instead of writing into whoever inherited the slot.
    #[test]
    fn ref_held_across_despawn_and_reuse_reports_staleness() {
        let mut world = World::new();
        let victim = world.spawn();
        world.insert(victim, TestGizmo { charges: 1 });
        let stale = ComponentRef::live(actor(victim), "TestGizmo");

        stale.actor().despawn(&mut world);
        let successor = world.spawn(); // may inherit `victim`'s recycled slot
        world.insert(successor, TestGizmo { charges: 99 });

        let result = stale.set_property(&mut world, "charges", Box::new(0));
        assert!(
            matches!(result, Err(ScriptRefError::ReferenceDespawned { .. })),
            "stale ref must be refused, got {result:?}"
        );
        // Whatever inherited the slot was never touched.
        assert_eq!(
            world.get::<TestGizmo>(successor).map(|g| g.charges),
            Some(99)
        );
    }

    /// #640 acceptance: a change watch observes the writes made through the
    /// ref (the detailed assertions live in subscribe.rs).
    #[test]
    fn set_property_is_observable_through_a_change_watch() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 0 });
        let r = ComponentRef::live(actor(e), "TestGizmo");
        let mut watch = crate::subscribe::ComponentRefWatch::new();
        assert!(watch.watch(&world, &r));

        r.set_property(&mut world, "charges", Box::new(5)).unwrap();

        assert_eq!(watch.changed(&world), vec![r.clone()]);
    }

    /// Attach a `TestGizmo` instance to `owner` through the registry, the
    /// way the editor and level loading do.
    fn attach_gizmo(world: &mut World, owner: Entity, charges: i32) -> Entity {
        pulsar_world_registry::attach_component(
            world,
            owner,
            pulsar_scene_model::NewInstance::new("TestGizmo"),
            pulsar_world_registry::ComponentPayload::Value(Box::new(TestGizmo { charges })),
        )
        .unwrap()
    }

    /// Several instances of one class on one actor: each index addresses
    /// its own instance's typed value, for reads, writes and methods alike.
    #[test]
    fn each_index_addresses_its_own_instance() {
        let mut world = World::new();
        let e = world.spawn();
        let first = attach_gizmo(&mut world, e, 100);
        let second = attach_gizmo(&mut world, e, 200);

        let live = ComponentRef::live(actor(e), "TestGizmo");
        let dup = actor(e).component("TestGizmo", 1);
        assert_eq!(live.instance(&world), Some(first));
        assert_eq!(dup.instance(&world), Some(second));

        assert_eq!(
            live.get_property(&world, "charges")
                .unwrap()
                .downcast_ref::<i32>()
                .copied(),
            Some(100)
        );
        assert_eq!(
            dup.get_property(&world, "charges")
                .unwrap()
                .downcast_ref::<i32>()
                .copied(),
            Some(200)
        );

        dup.set_property(&mut world, "charges", Box::new(222))
            .unwrap();
        assert_eq!(world.get::<TestGizmo>(second).unwrap().charges, 222);
        assert_eq!(
            world.get::<TestGizmo>(first).unwrap().charges,
            100,
            "the other instance is untouched"
        );

        let total = dup
            .call_method(&mut world, "add_charges", vec![Box::new(1i32)])
            .unwrap()
            .expect("method returns new total");
        assert_eq!(total.downcast_ref::<i32>(), Some(&223));
        assert_eq!(
            world.get::<TestGizmo>(first).unwrap().charges,
            100,
            "methods hit their own instance too"
        );
    }

    /// Missing targets are typed, distinct errors (#641 taxonomy).
    #[test]
    fn missing_targets_are_distinct_typed_errors() {
        // Registered class, no instance -> ComponentMissing.
        let mut world = World::new();
        let e = world.spawn();
        let r = actor(e).component("TestGizmo", 0);
        let err = r.get_property(&world, "charges").unwrap_err();
        assert!(matches!(err, ScriptRefError::ComponentMissing { .. }));

        // An index past the actor's instances -> InstanceMissing.
        attach_gizmo(&mut world, e, 1);
        let dup = actor(e).component("TestGizmo", 3);
        let err = dup.get_property(&world, "charges").unwrap_err();
        assert!(matches!(err, ScriptRefError::InstanceMissing { .. }));
    }

    /// Unknown property/method names are typed errors, never panics.
    #[test]
    fn unknown_property_and_method_are_typed_errors() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 3 });
        let r = ComponentRef::live(actor(e), "TestGizmo");

        let err = r.get_property(&world, "nope").unwrap_err();
        assert!(matches!(err, ScriptRefError::UnknownProperty { .. }));

        let err = r.call_method(&mut world, "nope", vec![]).unwrap_err();
        assert!(matches!(err, ScriptRefError::UnknownMethod { .. }));
    }

    /// Method dispatch runs the registered caller against the real World
    /// value: args marshal in, return values marshal out.
    #[test]
    fn call_method_dispatches_against_the_live_value() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 3 });
        let r = ComponentRef::live(actor(e), "TestGizmo");

        let result = r
            .call_method(&mut world, "add_charges", vec![Box::new(7i32)])
            .unwrap()
            .expect("method returns new total");
        assert_eq!(result.downcast_ref::<i32>(), Some(&10));
        assert_eq!(world.get::<TestGizmo>(e).unwrap().charges, 10);
    }

    /// A value of the wrong type for a property is a typed error, and
    /// nothing is written on failure.
    #[test]
    fn wrong_value_type_is_an_argument_type_error_and_writes_nothing() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 1 });
        let r = ComponentRef::live(actor(e), "TestGizmo");

        let err = r
            .set_property(&mut world, "charges", Box::new("nope"))
            .unwrap_err();
        assert!(matches!(err, ScriptRefError::ArgumentType { .. }));
        assert_eq!(world.get::<TestGizmo>(e).unwrap().charges, 1);
    }
}
