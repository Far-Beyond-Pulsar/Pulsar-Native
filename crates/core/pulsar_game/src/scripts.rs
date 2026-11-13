//! Gameplay script-crate actors: identity-tagged registration + native
//! hot reload (#653).
//!
//! A user's `scripts/` crate (scaffolded by `core_project_builder`) registers
//! its actor types through [`TickLoop::register_native_actor`]. The actor runs
//! through the component lifecycle (`crate::lifecycle`): an owner entity, an
//! instance entity carrying an [`ActorCarrier`], `begin_play` on the first
//! frame, `tick` per frame, `end_play` at removal and at the session end. The
//! registration does two more jobs:
//!
//! 1. **Identity tagging.** Every registered actor's owner entity is stamped
//!    with a [`ScriptTag`] naming the concrete actor type (`std::any::type_name`).
//!    The tag is what survives a dylib swap: the world lives in the PIE host,
//!    so entities and components persist across reload automatically; only
//!    the *behavior* (the boxed actor inside the old library) dies with it.
//! 2. **Reload re-attach.** After [`TickLoop::begin_script_reload`] collected
//!    the surviving tags, a registration whose type matches an unclaimed tag
//!    attaches its actor to THAT owner entity (a new instance entity: the
//!    same class on the same owner): no spawn, no state loss, mirroring
//!    `ScriptRuntime::reload_class`'s entity-preserving contract for script
//!    classes. Unmatched types spawn fresh; unmatched tags are
//!    left in place and logged (their behavior is gone, their data stays).
//!
//! Invariants: a tag is consumed at most once per reload (two instances of
//! one type rebind to their own two entities in registration order); re-attach
//! never despawns the owner or mutates its components; fresh sessions behave
//! exactly like a plain registration plus the tag.

use pulsar_scenedb::Entity;

use crate::lifecycle::{ActorCarrier, NativeActor};

/// Marker component stamped on an actor's owner entity at registration time.
///
/// `type_path` is `std::any::type_name::<A>()` of the registering actor --
/// a full Rust path (`mygame_scripts::Spinner`) that stays stable across
/// recompiles of the same source layout, which is exactly the identity a hot
/// reload must match on.
///
/// (`Component` comes from SceneDB's blanket impl for `Any + Send + Sync` --
/// no manual impl, and therefore none to drift.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptTag {
    pub type_path: String,
}

/// One surviving tag awaiting a matching registration during this reload.
pub(crate) struct RebindTarget {
    pub(crate) entity: Entity,
    pub(crate) type_path: String,
}

impl crate::tick::TickLoop {
    /// Register a gameplay actor under a stable script identity.
    ///
    /// Fresh session: spawns the owner entity (stamped with a [`ScriptTag`])
    /// and the actor's instance entity; the actor receives `begin_play` on the
    /// first frame the loop ticks. Hot-reload session
    /// ([`Self::begin_script_reload`] called first): if an unclaimed tag for
    /// `A`'s type path exists, the actor attaches to that owner instead, and
    /// the owner's world state is preserved untouched.
    ///
    /// Generated projects and `scripts/` crates MUST go through a
    /// `register_*actor` entry point for the reload guarantee to apply.
    pub fn register_native_actor<A: NativeActor>(&mut self, actor: A) -> Entity {
        self.attach_native_actor(std::any::type_name::<A>().to_string(), Box::new(actor))
    }

    fn attach_native_actor(&mut self, type_path: String, actor: Box<dyn NativeActor>) -> Entity {
        let rebind = self.take_rebind_target(&type_path);
        let mut store = self.scene_store.write();
        let world = &mut store.world;

        let owner = match rebind.filter(|entity| world.is_alive(*entity)) {
            Some(entity) => {
                tracing::info!(
                    ty = %type_path,
                    entity = entity.bits(),
                    "native hot reload: re-attaching actor to its existing entity"
                );
                // The same class on the same owner replaces what was there: a
                // carrier of the old behavior is despawned, which ends it.
                let stale: Vec<Entity> = world
                    .query::<&ActorCarrier>()
                    .filter(|(_, carrier)| carrier.type_path == type_path)
                    .map(|(instance, _)| instance)
                    .collect();
                for instance in stale {
                    let owned = world
                        .get::<crate::lifecycle::ComponentOwner>(instance)
                        .is_some_and(|link| link.entity() == entity);
                    if owned {
                        world.despawn(instance);
                    }
                }
                entity
            }
            None => {
                let entity = world.spawn();
                world.insert(
                    entity,
                    ScriptTag {
                        type_path: type_path.clone(),
                    },
                );
                entity
            }
        };

        let instance = world.spawn();
        world.insert(instance, ActorCarrier { type_path });
        world.insert(instance, crate::lifecycle::ComponentOwner::new(owner, true));
        drop(store);

        self.bank_actor(instance, actor);
        owner
    }

    /// Snapshot the world's [`ScriptTag`]s as pending rebind targets for the
    /// NEXT registrations. Called by the PIE embed layer when the host marked
    /// the session `pulsar_pie_abi::session_flags::RELOAD`, BEFORE project
    /// `setup()` runs. Takes one short write scope, per the TickLoop locking
    /// protocol.
    pub fn begin_script_reload(&mut self) {
        let targets: Vec<RebindTarget> = {
            let store = self.scene_store.read();
            store
                .world
                .query::<&ScriptTag>()
                .map(|(entity, tag)| RebindTarget {
                    entity,
                    type_path: tag.type_path.clone(),
                })
                .collect()
        };
        tracing::info!(
            count = targets.len(),
            "native hot reload: collecting tagged actors for rebinding"
        );
        self.pending_rebinds = targets;
        self.reload_armed = true;
    }

    /// True while this session will re-bind matching registrations rather
    /// than spawn them (i.e. [`Self::begin_script_reload`] ran).
    pub fn is_reload_session(&self) -> bool {
        self.reload_armed
    }

    /// Pop the first unclaimed target matching `type_path`: exact match wins
    /// first, then a `::{type_path}` suffix match (generated classes live at
    /// `<crate>::classes::<Name>` while hand-written crates register short
    /// names; suffix matching keeps two same-short-named types distinct).
    fn take_rebind_target(&mut self, type_path: &str) -> Option<Entity> {
        let wanted_suffix = format!("::{type_path}");
        let idx = self
            .pending_rebinds
            .iter()
            .position(|t| t.type_path == type_path || t.type_path.ends_with(&wanted_suffix))?;
        Some(self.pending_rebinds.remove(idx).entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::ActorCx;
    use crate::tick::TickLoop;
    use engine_backend::scene::Transform;
    use pulsar_core::TickMode;
    use pulsar_scenedb::World;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// Counts ticks in a shared cell so tests can observe which behavior
    /// drives after the swap.
    #[derive(Default)]
    struct Probe(Arc<AtomicU32>);

    impl NativeActor for Probe {
        fn tick(&mut self, _: Entity, _: &mut World, _: &mut ActorCx<'_>, _: f32) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn game() -> TickLoop {
        TickLoop::new(TickMode::default(), 0)
    }

    /// Fresh-session registration: owner entity tagged with the full type
    /// path, ticks run through the lifecycle.
    #[test]
    fn fresh_registration_spawns_tags_and_ticks() {
        let mut g = game();
        assert!(!g.is_reload_session());

        let probe = Probe::default();
        let counter = Arc::clone(&probe.0);
        let e = g.register_native_actor(probe);
        let expected = ScriptTag {
            type_path: std::any::type_name::<Probe>().to_string(),
        };
        {
            let store = g.scene_store.read();
            assert_eq!(
                store.world.get::<ScriptTag>(e),
                Some(&expected),
                "registration must stamp the actor's full type path"
            );
        }

        g.tick_once();
        g.tick_once();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    /// THE #653 hot-reload contract: same type + surviving tag => SAME owner
    /// entity, component state untouched, the new behavior drives future
    /// ticks and the old one is gone.
    #[test]
    fn reload_rebinds_to_the_existing_entity_without_state_loss() {
        let old = Probe::default();
        let old_counter = Arc::clone(&old.0);

        let mut g = game();
        let original = g.register_native_actor(old);
        g.tick_once();
        assert_eq!(old_counter.load(Ordering::SeqCst), 1);

        // Gameplay state accumulated before the swap: a Transform the old
        // session mutated. Reload must not disturb it.
        {
            let mut store = g.scene_store.write();
            store.world.insert(
                original,
                Transform {
                    position: [7.5, 0.0, 2.0],
                    ..Transform::default()
                },
            );
        }

        // The old library goes away with its session; the world survives.
        g.end_component_runtime();
        g.begin_script_reload();
        assert!(g.is_reload_session());

        // The NEW library registers the same type...
        let new = Probe::default();
        let new_counter = Arc::clone(&new.0);
        let rebound = g.register_native_actor(new);
        assert_eq!(rebound, original, "reload must reuse the tagged entity");

        // ...without spawning another owner nor touching state.
        {
            let store = g.scene_store.read();
            let world = &store.world;
            assert_eq!(
                world.get::<Transform>(original).map(|t| t.position),
                Some([7.5, 0.0, 2.0]),
                "component state preserved across the rebind"
            );
            assert_eq!(
                world.query::<&ScriptTag>().count(),
                1,
                "rebinding spawns no duplicate owner"
            );
        }

        g.tick_once();
        assert_eq!(
            new_counter.load(Ordering::SeqCst),
            1,
            "the new behavior ticks"
        );
        assert_eq!(old_counter.load(Ordering::SeqCst), 1, "the old one is gone");
    }

    /// A re-attach over a live actor of the same class on the same owner (no
    /// session end between) replaces it, and the old one is ended.
    #[test]
    fn a_reattach_replaces_the_live_actor_of_the_same_class() {
        let old = Probe::default();
        let old_counter = Arc::clone(&old.0);
        let mut g = game();
        g.register_native_actor(old);
        g.tick_once();

        g.begin_script_reload();
        let new = Probe::default();
        let new_counter = Arc::clone(&new.0);
        g.register_native_actor(new);
        g.tick_once();
        g.tick_once();

        assert_eq!(
            old_counter.load(Ordering::SeqCst),
            1,
            "the replaced actor stops"
        );
        assert_eq!(new_counter.load(Ordering::SeqCst), 2);
    }

    /// Two instances of one type rebind to their own two entities, each once
    /// -- tags are consumed in order, never double-claimed.
    #[test]
    fn two_instances_rebind_one_to_one_in_registration_order() {
        let mut g = game();
        let a = g.register_native_actor(Probe::default());
        let b = g.register_native_actor(Probe::default());
        assert_ne!(a, b);

        g.end_component_runtime();
        g.begin_script_reload();
        let a2 = g.register_native_actor(Probe::default());
        let b2 = g.register_native_actor(Probe::default());

        assert!(
            (a2 == a && b2 == b) || (a2 == b && b2 == a),
            "each surviving tag claimed exactly once: {a:?}/{b:?} -> {a2:?}/{b2:?}"
        );

        // Third registration finds no tag left => fresh spawn.
        let c = g.register_native_actor(Probe::default());
        assert_ne!(c, a);
        assert_ne!(c, b);
    }

    /// An unknown type (class added between sessions) spawns fresh even mid-
    /// reload; a removed class leaves its entity orphaned but intact -- data
    /// outlives behavior (cleanup policy belongs to a later phase).
    #[test]
    fn reload_spawns_new_types_and_leaves_removed_ones_orphaned() {
        struct Newcomer;
        impl NativeActor for Newcomer {}

        let mut g = game();
        let gone = g.register_native_actor(Probe::default());
        {
            let mut store = g.scene_store.write();
            store.world.remove::<ScriptTag>(gone);
        }

        g.end_component_runtime();
        g.begin_script_reload();
        let added = g.register_native_actor(Newcomer);
        assert_ne!(added, gone, "no tag to claim => fresh spawn");

        assert!(
            g.scene_store.read().world.is_alive(gone),
            "orphaned entity keeps living; nothing despawns behind the user's back"
        );
    }

    /// The session end ends every actor (SceneDB's registry never did), and
    /// leaves no carrier or boxed actor behind in the world.
    #[test]
    fn the_session_end_ends_actors_and_clears_their_carriers() {
        struct Ender(Arc<AtomicU32>);
        impl NativeActor for Ender {
            fn end_play(&mut self, _: Entity, _: &mut World, _: &mut ActorCx<'_>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let ended = Arc::new(AtomicU32::new(0));
        let mut g = game();
        g.register_native_actor(Ender(Arc::clone(&ended)));
        g.tick_once();
        g.end_session();
        assert_eq!(ended.load(Ordering::SeqCst), 1);
        assert_eq!(
            g.scene_store.read().world.query::<&ActorCarrier>().count(),
            0
        );
    }
}
