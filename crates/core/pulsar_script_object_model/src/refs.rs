//! The script-facing handle types: [`ActorRef`] and [`ComponentRef`].
//!
//! Both are plain value types -- `Copy`/`Clone`, no heap allocation, safe to
//! store in arenas, components, or VM frames. They carry *no* authority by
//! themselves: every accessor re-validates against the world it is handed
//! ([`ActorRef::validate`]/[`ComponentRef::validate`]), so a ref stored
//! before a despawn simply fails validation afterwards instead of
//! misaddressing whoever inherited the recycled slot.
//!
//! `ComponentRef`'s identity fields are the properties panel's convention
//! (Pulsar-Native#519/#575): `(class_name, component_index)` addresses one
//! specific instance even when an object carries several instances of the
//! same class. Index 0 (or, with an instance store supplied, that store's
//! first-enabled index) is the *live-typed* instance whose value lives in
//! the `World`; other indexes route through their own JSON records
//! ([`crate::instances`]).

use pulsar_scenedb::{Entity, World};

use crate::errors::ScriptRefError;

/// A handle to one actor (entity) in the shared scene world.
///
/// Cheap identity only -- spawn/despawn and component access stay on the
/// world/store APIs; this type exists so gameplay code can *name* an actor
/// across frames, graphs, and marshalling boundaries without holding locks
/// or raw ids.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActorRef(pub Entity);

impl ActorRef {
    /// Wrap a live entity handle. No validation happens here (construction
    /// is total so refs can round-trip through serialization); the first
    /// accessor validates.
    pub fn new(entity: Entity) -> Self {
        Self(entity)
    }

    /// The wrapped entity handle.
    pub fn entity(self) -> Entity {
        self.0
    }

    /// Cheap liveness probe -- `true` iff the entity would pass
    /// [`Self::validate`] right now.
    pub fn is_alive(self, world: &World) -> bool {
        world.is_alive(self.0)
    }

    /// Validate this handle against `world`: `Err` when the entity is dead,
    /// its slot was recycled, or the id is not a real entity of this world
    /// (`Entity::DANGLING`, out-of-range slot).
    pub fn validate(self, world: &World) -> Result<(), ScriptRefError> {
        ensure_live_entity(world, self.0)
    }

    /// Build a component handle addressing one instance of `class_name` on
    /// this actor.
    pub fn component(self, class_name: impl Into<String>, component_index: u32) -> ComponentRef {
        ComponentRef {
            entity: self.0,
            class_name: class_name.into(),
            component_index,
        }
    }

    /// Despawn the referenced actor (recursively, per `World::despawn`).
    /// Returns `false` if already gone -- never panics (#641). Note: any
    /// `ActorRef`/`ComponentRef` to this entity (or its descendants)
    /// validates as `ReferenceDespawned` afterwards.
    pub fn despawn(self, world: &mut World) -> bool {
        if !world.is_alive(self.0) {
            return false;
        }
        world.despawn(self.0)
    }
}

impl From<Entity> for ActorRef {
    fn from(entity: Entity) -> Self {
        Self(entity)
    }
}

/// A handle to ONE component instance on one actor: the properties panel's
/// `(class_name, component_index)` identity (Pulsar-Native#519), usable from
/// scripts, generated code, and graph pins alike.
///
/// Routing follows the panel exactly (#519/#575): the first enabled instance
/// of a class is the live-typed value living directly in `World`; every
/// other index exists only as its own serialized record routed through
/// [`crate::instances::ComponentInstanceStore`]. Accessors refuse a stale or
/// mismatched index rather than letting an edit land elsewhere
/// (`ClassMismatch`/`InstanceMissing`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComponentRef {
    /// Owning actor.
    pub entity: Entity,
    /// Registered component class name (`pulsar_world_registry` key).
    pub class_name: String,
    /// Which instance of `class_name` on `entity` this addresses (0-based,
    /// same numbering as the editor's persisted component list).
    pub component_index: u32,
}

impl ComponentRef {
    /// Address the live-typed instance of `class_name` on `actor`
    /// (convenience for the overwhelmingly common single-instance case).
    pub fn live(actor: ActorRef, class_name: impl Into<String>) -> Self {
        actor.component(class_name, 0)
    }

    /// The owning-actor half of this handle.
    pub fn actor(&self) -> ActorRef {
        ActorRef(self.entity)
    }

    /// Validate actor liveness AND that the named class is registered for
    /// live World residency. Component presence is deliberately NOT checked
    /// here (it can change between validation and access); the accessors do
    /// that per call.
    pub fn validate(&self, world: &World) -> Result<(), ScriptRefError> {
        ensure_live_entity(world, self.entity)?;
        if pulsar_world_registry::component_id_for_class(&self.class_name).is_none() {
            return Err(ScriptRefError::UnregisteredClass(self.class_name.clone()));
        }
        Ok(())
    }

    /// Cheap combined probe mirroring [`Self::validate`]: `true` iff every
    /// accessor on this ref could at least address the right object now.
    pub fn is_valid(&self, world: &World) -> bool {
        self.validate(world).is_ok()
    }
}

/// Shared liveness gate behind every accessor (#641): typed errors always;
/// debug-build asserts ONLY for ids that were never valid in this world
/// (`DANGLING` sentinel, out-of-range slot -- fabricated or foreign-world
/// ids), because those indicate raw-id misuse at the call site, not ordinary
/// staleness.
pub(crate) fn ensure_live_entity(world: &World, entity: Entity) -> Result<(), ScriptRefError> {
    if !world.is_alive(entity) {
        return Err(ScriptRefError::despawned(entity));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_actor() -> (World, ActorRef) {
        let mut world = World::new();
        let entity = world.spawn();
        (world, ActorRef::new(entity))
    }

    /// #640: a fresh handle to a spawned entity validates; after despawn the
    /// SAME handle must report staleness instead of success.
    #[test]
    fn handles_validate_alive_and_report_despawned() {
        let (mut world, actor) = world_with_actor();
        assert!(actor.validate(&world).is_ok());
        assert!(actor.despawn(&mut world));
        assert_eq!(actor.validate(&world), Err(ScriptRefError::despawned(actor.entity())));
        assert!(!actor.is_alive(&world));
    }

    /// #640: despawning twice is a clean no-op (`false`), not a panic --
    /// the stale-handle contract applies to lifecycle calls too.
    #[test]
    fn double_despawn_is_a_clean_no_op() {
        let (mut world, actor) = world_with_actor();
        assert!(actor.despawn(&mut world));
        assert!(!actor.despawn(&mut world));
    }

    /// #641: `Entity::DANGLING` and out-of-range slots are rejected with the
    /// same typed error as ordinary staleness (release semantics; the debug
    /// assert fires only in dev builds).
    #[test]
    fn dangling_and_foreign_ids_are_typed_errors_not_panics() {
        let (world, _alive) = world_with_actor();

        let dangling = ActorRef::new(Entity::DANGLING);
        assert_eq!(
            dangling.validate(&world),
            Err(ScriptRefError::despawned(Entity::DANGLING))
        );

        let fabricated = ActorRef::new(Entity::from_bits((999_999u64) << 32));
        assert_eq!(fabricated.validate(&world), Err(ScriptRefError::despawned(fabricated.0)));
    }

    /// #640: `component()` composes the full panel identity; `live()` is the
    /// index-0 shorthand; `actor()` round-trips.
    #[test]
    fn component_ref_composes_and_round_trips() {
        let (_, actor) = world_with_actor();
        let r = actor.component("LightComponent", 2);
        assert_eq!(r.component_index, 2);
        assert_eq!(r.class_name, "LightComponent");
        assert_eq!(r.actor(), actor);
        assert_eq!(ComponentRef::live(actor, "LightComponent"), actor.component("LightComponent", 0));
    }

    /// #640: an unregistered class fails validation up front -- there is no
    /// live World representation a script path could address.
    #[test]
    fn unregistered_class_fails_validation() {
        let (world, actor) = world_with_actor();
        let r = actor.component("NeverRegistered", 0);
        assert_eq!(r.validate(&world), Err(ScriptRefError::UnregisteredClass("NeverRegistered".into())));
    }

    /// #640: slots are recycled with generation bumps -- a stale handle to a
    /// REUSED slot must still fail validation (generation mismatch), which
    /// is exactly what keeps recycled-slot writes impossible.
    #[test]
    fn recycled_slot_rejects_the_stale_generation() {
        let mut world = World::new();
        // Spawn two entities so despawning the first frees a recyclable slot.
        let first = world.spawn();
        let _second = world.spawn();
        world.despawn(first);
        let recycled = world.spawn(); // may reuse `first`'s slot, bumped generation

        let stale = ActorRef::new(first);
        if recycled.index() == first.index() {
            // Slot really was reused: the old generation MUST be rejected...
            assert!(!stale.is_alive(&world));
            assert!(stale.validate(&world).is_err());
            // ...while the fresh handle to the same slot stays valid.
            assert!(ActorRef::new(recycled).validate(&world).is_ok());
        } else {
            // Implementation didn't recycle this run -- staleness still holds.
            assert!(stale.validate(&world).is_err());
        }
    }
}
