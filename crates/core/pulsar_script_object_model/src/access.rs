//! Property and method accessors on [`ComponentRef`], with the properties
//! panel's live-typed-vs-metadata routing (Pulsar-Native#519/#575).
//!
//! Every accessor:
//! 1. validates actor liveness (`ReferenceDespawned`),
//! 2. checks class registration (`UnregisteredClass`),
//! 3. routes by `(class_name, component_index)` identity (see
//!    [`crate::routing`]): the live-typed index goes straight through the
//!    typed `World` value -- no JSON on the hot path, `Mut`-guard change
//!    events firing exactly as for panel edits; any other index is read and
//!    edited through its own JSON record;
//! 4. fails with a typed error rather than panicking or guessing (#641).
//!
//! Two spellings per operation: [`ComponentRef::get_property`]/
//! [`ComponentRef::set_property`] are the everyday live-typed calls scripts
//! and generated code use; [`ComponentRef::get_property_with_instances`]/
//! [`ComponentRef::set_property_with_instances`] additionally route
//! non-live indexes through their own records via a
//! [`ComponentInstanceStore`].
//!
//! Method dispatch always targets the live-typed value (the only instance
//! with executable behavior); `component_index` is not consulted for
//! methods.

use pulsar_reflection::{MethodArgs, MethodReturnValue, PropertyMetadata, REGISTRY};
use pulsar_scenedb::World;

use crate::errors::ScriptRefError;
use crate::instances::ComponentInstanceStore;
use crate::refs::ComponentRef;
use crate::routing::{deserialize_property, route, serialize_property, Route};

impl ComponentRef {
    /// Read one property of the referenced component instance as JSON.
    ///
    /// Live-typed path only: `component_index` must address the live-typed
    /// instance (default index 0). See [`Self::get_property_with_instances`]
    /// for duplicate-instance routing.
    pub fn get_property(&self, world: &World, property: &str) -> Result<serde_json::Value, ScriptRefError> {
        self.get_property_with_instances(world, None, property)
    }

    /// Read one property, routing duplicate indexes through `store`'s
    /// instance records exactly like the properties panel (#519/#561).
    /// Pass `None` to restrict to the live-typed path.
    pub fn get_property_with_instances(
        &self,
        world: &World,
        store: Option<&dyn ComponentInstanceStore>,
        property: &str,
    ) -> Result<serde_json::Value, ScriptRefError> {
        let meta = self.property_metadata(property)?;
        match route(self, world, store)? {
            Route::Live => {
                let value = (meta.getter)(self.live_instance(world)?);
                serialize_property(&self.class_name, property, &*value)
            }
            Route::Duplicate { record } => {
                let scratch = crate::routing::ScratchInstance::hydrate(&self.class_name, &record.data)?;
                let value = (meta.getter)(scratch.instance()?);
                serialize_property(&self.class_name, property, &*value)
            }
        }
    }

    /// Write one property of the referenced component instance from JSON.
    ///
    /// The value deserializes against the property's reflected type and the
    /// setter closure mutates the real storage -- nothing is written on
    /// failure. Live-typed path only; see
    /// [`Self::set_property_with_instances`] for duplicate routing.
    pub fn set_property(
        &self,
        world: &mut World,
        property: &str,
        value: serde_json::Value,
    ) -> Result<(), ScriptRefError> {
        self.set_property_with_instances(world, None, property, value)
    }

    /// Write one property, routing duplicate indexes through `store`.
    ///
    /// Live case: after the typed value changes, the FULL new shape is
    /// serialized back into that instance's own record (when supplied) so
    /// `World` and the records never diverge (Pulsar-Native#561, Bug B).
    /// Duplicate case: the edit lands only in THAT record.
    pub fn set_property_with_instances(
        &self,
        world: &mut World,
        mut store: Option<&mut dyn ComponentInstanceStore>,
        property: &str,
        value: serde_json::Value,
    ) -> Result<(), ScriptRefError> {
        let meta = self.property_metadata(property)?;
        let typed = deserialize_property(&self.class_name, property, meta.type_info, value)?;

        match route(self, world, store.as_deref())? {
            Route::Live => {
                // Scoped so the `&mut World` borrow ends before the record
                // persist-back below re-indexes the store.
                let persisted_json = {
                    let instance = self.live_instance_mut(world)?;
                    (meta.setter)(&mut *instance, typed);
                    instance.to_json().ok()
                };
                if let Some(json) = persisted_json {
                    if let Some(store) = store.as_mut() {
                        store.set_instance_data(self.entity, self.component_index, json);
                    }
                }
                Ok(())
            }
            Route::Duplicate { record } => {
                let json = {
                    let mut scratch =
                        crate::routing::ScratchInstance::hydrate(&self.class_name, &record.data)?;
                    {
                        let instance = scratch.instance_mut()?;
                        (meta.setter)(&mut *instance, typed);
                    }
                    scratch.persist()?
                };
                let wrote = store
                    .as_mut()
                    .map(|s| s.set_instance_data(self.entity, self.component_index, json))
                    .unwrap_or(false);
                if !wrote {
                    return Err(ScriptRefError::InstanceMissing {
                        entity: self.entity,
                        class_name: self.class_name.clone(),
                        component_index: self.component_index,
                    });
                }
                Ok(())
            }
        }
    }

    /// Invoke one blueprint-callable method on the referenced component's
    /// live-typed value.
    ///
    /// DELEGATES to `pulsar_world_registry::invoke_component_method` -- the
    /// one unified dispatcher (#643): same `MethodMetadata.caller` closures
    /// every other Blueprint caller uses, plus argument arity/type
    /// validation that reports typed errors where the raw generated callers
    /// would panic. Duplicate instances share their class's behavior, so
    /// this always runs against the live-typed value regardless of
    /// `component_index`.
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

    // ── shared lookup helpers ───────────────────────────────────────────

    fn live_instance<'w>(
        &self,
        world: &'w World,
    ) -> Result<&'w dyn pulsar_reflection::EngineClass, ScriptRefError> {
        pulsar_world_registry::get_world_component_as_engine_class(&self.class_name, world, self.entity)
            .ok_or_else(|| ScriptRefError::ComponentMissing {
                entity: self.entity,
                class_name: self.class_name.clone(),
            })
    }

    fn live_instance_mut<'w>(
        &self,
        world: &'w mut World,
    ) -> Result<&'w mut dyn pulsar_reflection::EngineClass, ScriptRefError> {
        pulsar_world_registry::get_world_component_as_engine_class_mut(
            &self.class_name,
            world,
            self.entity,
        )
        .ok_or_else(|| ScriptRefError::ComponentMissing {
            entity: self.entity,
            class_name: self.class_name.clone(),
        })
    }

    /// Reflected metadata for one property, looked up through a throwaway
    /// instance exactly like the properties panel does -- only the type-
    /// bound getter/setter closures are used, never the throwaway's values.
    fn property_metadata(&self, property: &str) -> Result<PropertyMetadata, ScriptRefError> {
        REGISTRY
            .create_instance(&self.class_name)
            .and_then(|instance| {
                instance.get_properties().into_iter().find(|p| p.name == property)
            })
            .ok_or_else(|| ScriptRefError::UnknownProperty {
                class_name: self.class_name.clone(),
                property: property.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::InstanceRecord;
    use crate::test_support::{FakeInstanceStore, TestGizmo};
    use pulsar_scenedb::Entity;

    fn actor(e: Entity) -> crate::refs::ActorRef {
        crate::refs::ActorRef(e)
    }

    fn gizmo_record(charges: i64, enabled: bool) -> InstanceRecord {
        InstanceRecord {
            class_name: "TestGizmo".into(),
            enabled,
            data: serde_json::json!({ "charges": charges }),
        }
    }

    /// #640 acceptance: two entities carrying the same component class --
    /// writes through each ref land on exactly that ref's target, reads see
    /// only their own target, and nothing panics.
    #[test]
    fn refs_across_duplicate_class_entities_write_only_their_own_target() {
        let mut world = World::new();
        let door = world.spawn();
        let chest = world.spawn();
        world.insert(door, TestGizmo { charges: 10 });
        world.insert(chest, TestGizmo { charges: 20 });

        let door_ref = ComponentRef::live(actor(door), "TestGizmo");
        let chest_ref = ComponentRef::live(actor(chest), "TestGizmo");

        door_ref.set_property(&mut world, "charges", serde_json::json!(11)).unwrap();
        chest_ref.set_property(&mut world, "charges", serde_json::json!(22)).unwrap();

        assert_eq!(world.get::<TestGizmo>(door).unwrap().charges, 11);
        assert_eq!(world.get::<TestGizmo>(chest).unwrap().charges, 22);
        assert_eq!(door_ref.get_property(&world, "charges").unwrap(), serde_json::json!(11));
        assert_eq!(chest_ref.get_property(&world, "charges").unwrap(), serde_json::json!(22));
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

        let result = stale.set_property(&mut world, "charges", serde_json::json!(0));
        assert!(
            matches!(result, Err(ScriptRefError::ReferenceDespawned { .. })),
            "stale ref must be refused, got {result:?}"
        );
        // Whatever inherited the slot was never touched.
        assert_eq!(world.get::<TestGizmo>(successor).map(|g| g.charges), Some(99));
    }

    /// #640 acceptance: subscription events observe exactly the writes made
    /// through the ref (event-level assertions live in subscribe.rs).
    #[test]
    fn set_property_is_observable_through_subscriptions() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 0 });
        let r = ComponentRef::live(actor(e), "TestGizmo");
        let sub = crate::subscribe::subscribe_component(&mut world, &r).unwrap();

        r.set_property(&mut world, "charges", serde_json::json!(5)).unwrap();

        let events = crate::subscribe::take_change_events_for(&mut world, sub);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].entity, e);
    }

    /// Duplicate instances route by index: index 0 is live-typed; index 1
    /// reads/writes ONLY its own JSON record (panel parity, #519/#561).
    #[test]
    fn duplicate_instances_route_by_index_like_the_panel() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 100 }); // hydrated from record 0

        let mut store = FakeInstanceStore::default();
        store.attach(e, &[gizmo_record(100, true), gizmo_record(200, true)]);

        let live = ComponentRef::live(actor(e), "TestGizmo");
        let dup = actor(e).component("TestGizmo", 1);

        // Reads come from different storages and never cross.
        assert_eq!(
            live.get_property_with_instances(&world, Some(&store), "charges").unwrap(),
            serde_json::json!(100)
        );
        assert_eq!(
            dup.get_property_with_instances(&world, Some(&store), "charges").unwrap(),
            serde_json::json!(200)
        );

        // Writes likewise.
        dup.set_property_with_instances(&mut world, Some(&mut store), "charges", serde_json::json!(222))
            .unwrap();
        assert_eq!(
            dup.get_property_with_instances(&world, Some(&store), "charges").unwrap(),
            serde_json::json!(222)
        );
        assert_eq!(
            live.get_property_with_instances(&world, Some(&store), "charges").unwrap(),
            serde_json::json!(100)
        );
        assert_eq!(world.get::<TestGizmo>(e).unwrap().charges, 100, "live World value untouched");

        // Live writes persist back into the record so they never diverge.
        live.set_property_with_instances(&mut world, Some(&mut store), "charges", serde_json::json!(111))
            .unwrap();
        assert_eq!(world.get::<TestGizmo>(e).unwrap().charges, 111);
        assert_eq!(store.record_data(0).unwrap()["charges"], serde_json::json!(111));
        assert_eq!(store.record_data(1).unwrap()["charges"], serde_json::json!(222));
    }

    /// Missing targets are typed, distinct errors (#641 taxonomy).
    #[test]
    fn missing_targets_are_distinct_typed_errors() {
        // Registered class, never hydrated -> ComponentMissing.
        let mut world = World::new();
        let e = world.spawn();
        let r = actor(e).component("TestGizmo", 0);
        let err = r.get_property(&world, "charges").unwrap_err();
        assert!(matches!(err, ScriptRefError::ComponentMissing { .. }));

        // Non-live index without a store -> InstanceMissing.
        world.insert(e, TestGizmo { charges: 1 });
        let dup = actor(e).component("TestGizmo", 3);
        let err = dup.get_property(&world, "charges").unwrap_err();
        assert!(matches!(err, ScriptRefError::InstanceMissing { .. }));
    }

    /// Index holding another class's record is refused (#519 discipline).
    #[test]
    fn mismatched_index_class_pair_is_refused() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 1 });

        let mut store = FakeInstanceStore::default();
        store.attach(
            e,
            &[
                gizmo_record(1, true),
                InstanceRecord { class_name: "Other".into(), enabled: true, data: serde_json::json!({}) },
            ],
        );

        let r = actor(e).component("TestGizmo", 1);
        let err = r.get_property_with_instances(&world, Some(&store), "charges").unwrap_err();
        assert!(matches!(err, ScriptRefError::ClassMismatch { .. }));
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

    /// Malformed JSON for a duplicate instance is a typed Marshalling error
    /// -- not a panic, not a silent fallback to defaults.
    #[test]
    fn malformed_duplicate_record_is_a_marshalling_error() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 1 });

        let mut store = FakeInstanceStore::default();
        store.attach(
            e,
            &[
                gizmo_record(1, true),
                InstanceRecord {
                    class_name: "TestGizmo".into(),
                    enabled: true,
                    data: serde_json::json!({ "charges": "not-a-number" }),
                },
            ],
        );

        let dup = actor(e).component("TestGizmo", 1);
        let err = dup.get_property_with_instances(&world, Some(&store), "charges").unwrap_err();
        assert!(matches!(err, ScriptRefError::Marshalling { .. }));
    }

    /// Wrong JSON type for a property is a typed Marshalling error, and
    /// nothing is written on failure.
    #[test]
    fn wrong_value_type_is_a_marshalling_error_and_writes_nothing() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 1 });
        let r = ComponentRef::live(actor(e), "TestGizmo");

        let err = r.set_property(&mut world, "charges", serde_json::json!("nope")).unwrap_err();
        assert!(matches!(err, ScriptRefError::Marshalling { .. }));
        assert_eq!(world.get::<TestGizmo>(e).unwrap().charges, 1);
    }
}
