//! Gameplay script-crate actors: identity-tagged registration + native
//! hot reload (#653).
//!
//! A user's `scripts/` crate (scaffolded by `core_project_builder`) registers
//! its actor types through [`TickLoop::register_actor`] instead of the bare
//! `ActorRegistry::register`. That wrapper does two jobs:
//!
//! 1. **Identity tagging.** Every registered actor's entity is stamped with a
//!    [`ScriptTag`] naming the concrete actor type (`std::any::type_name`).
//!    The tag is what survives a dylib swap: the world lives in the PIE host,
//!    so entities and components persist across reload automatically; only
//!    the *behavior* (the boxed actor inside the old library) dies with it.
//! 2. **Reload rebinding.** After [`TickLoop::begin_script_reload`] collected
//!    the surviving tags, registrations whose type matches an unclaimed tag
//!    are bound to THAT entity — no spawn, no state loss — mirroring
//!    `BlueprintDispatcher::reload_blueprint`'s entity-preserving contract
//!    for VM classes (#648). Unmatched types spawn fresh; unmatched tags are
//!    left in place and logged (their behavior is gone, their data stays).
//!
//! Invariants: a tag is consumed at most once per reload (two instances of
//! one type rebind to their own two entities in registration order); rebinding
//! never despawns or mutates existing components; fresh sessions behave
//! exactly like plain `register` plus the tag.

use pulsar_scenedb::{Actor, Entity, World};

/// Marker component stamped on an actor's backing entity at
/// [`crate::tick::TickLoop::register_actor`] time.
///
/// `type_path` is `std::any::type_name::<A>()` of the registering actor —
/// a full Rust path (`mygame_scripts::Spinner`) that stays stable across
/// recompiles of the same source layout, which is exactly the identity a hot
/// reload must match on.
///
/// (`Component` comes from SceneDB's blanket impl for `Any + Send + Sync` —
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

/// An actor shell rebound onto an EXISTING entity after a hot reload.
///
/// SceneDB's pinned `ActorRegistry` can only box actors it spawned itself,
/// so rebound shells live here and are ticked by `tick_once` right after the
/// registry phase — same callbacks, same world borrow, second slice of the
/// same phase-2 lock scope.
pub(crate) struct ReboundActor {
    pub(crate) entity: Entity,
    pub(crate) actor: Box<dyn Actor>,
}

impl ReboundActor {
    /// Fire `begin_play` on the existing entity (parity with registry
    /// semantics, where begin_play runs at registration time).
    pub(crate) fn begin(&mut self, world: &mut World) {
        self.actor.begin_play(self.entity, world);
    }

    /// Advance the shell for this tick.
    pub(crate) fn tick(&mut self, world: &mut World) {
        self.actor.tick(self.entity, world);
    }
}

impl crate::tick::TickLoop {
    /// Register a gameplay actor under a stable script identity.
    ///
    /// Fresh session: identical semantics to `ActorRegistry::register`
    /// (spawn + `begin_play`), plus a [`ScriptTag`] stamp. Hot-reload session
    /// ([`Self::begin_script_reload`] called first): if an unclaimed tag for
    /// `A`'s type path exists, the actor is bound to that entity instead —
    /// `begin_play` fires again there (generated hydration is absent-only,
    /// so components are never duplicated) and the entity's world state is
    /// preserved untouched.
    ///
    /// Generated projects and `scripts/` crates MUST go through this entry
    /// point for the reload guarantee to apply; bare `game.actors.register`
    /// keeps its plain spawn-only semantics.
    pub fn register_actor<A: Actor>(&mut self, actor: A) -> Entity {
        let type_path = std::any::type_name::<A>().to_string();

        if let Some(entity) = self.take_rebind_target(&type_path) {
            tracing::info!(
                ty = %type_path,
                entity = entity.bits(),
                "native hot reload: rebinding actor to its existing entity"
            );
            let mut shell = ReboundActor {
                entity,
                actor: Box::new(actor),
            };
            let mut store = self.scene_store.write();
            shell.begin(store.world_mut());
            drop(store);
            self.rebinding.push(shell);
            return entity;
        }

        let mut store = self.scene_store.write();
        let entity = self.actors.register(actor, store.world_mut());
        store
            .world_mut()
            .insert(entity, ScriptTag { type_path });
        entity
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
                .world()
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
    use crate::tick::TickLoop;
    use engine_backend::scene::Transform;
    use pulsar_core::TickMode;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Counts ticks in a shared cell so tests can observe that the REBOUND
    /// shell (not just the registry copy) drives after the swap.
    #[derive(Default)]
    struct Probe(Arc<AtomicU32>);

    impl Actor for Probe {
        fn tick(&mut self, _entity: Entity, _world: &mut World) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn game() -> TickLoop {
        TickLoop::new(TickMode::default(), 0)
    }

    /// Fresh-session registration behaves like plain register plus the tag:
    /// begin_play fired, entity tagged with the full type path.
    #[test]
    fn fresh_registration_spawns_tags_and_ticks() {
        let mut g = game();
        assert!(!g.is_reload_session());

        let e = g.register_actor(Probe::default());
        let expected = ScriptTag {
            type_path: std::any::type_name::<Probe>().to_string(),
        };
        {
            let store = g.scene_store.read();
            assert_eq!(
                store.world().get::<ScriptTag>(e),
                Some(&expected),
                "registration must stamp the actor's full type path"
            );
        }

        // Ticks run through the normal phase-2 path without incident.
        g.tick_once();
        g.tick_once();
    }

    /// THE #653 hot-reload contract: same type + surviving tag ⇒ SAME entity,
    /// component state untouched, new shell drives future ticks.
    #[test]
    fn reload_rebinds_to_the_existing_entity_without_state_loss() {
        let probe = Probe::default();
        let counter = Arc::clone(&probe.0);

        let mut g = game();
        let original = g.register_actor(probe);
        assert_eq!(counter.load(Ordering::SeqCst), 0, "no ticks yet");

        // Gameplay state accumulated before the swap: a Transform the old
        // session mutated. Reload must not disturb it.
        {
            let mut store = g.scene_store.write();
            store.world_mut().insert(
                original,
                Transform {
                    position: [7.5, 0.0, 2.0],
                    ..Transform::default()
                },
            );
        }

        g.begin_script_reload();
        assert!(g.is_reload_session());

        // The NEW library registers the same type...
        let rebound = g.register_actor(Probe::default());
        assert_eq!(rebound, original, "reload must reuse the tagged entity");

        // ...without spawning anything extra nor touching state.
        {
            let store = g.scene_store.read();
            let world = store.world();
            assert_eq!(
                world.get::<Transform>(original).map(|t| t.position),
                Some([7.5, 0.0, 2.0]),
                "component state preserved across the rebind"
            );
            assert_eq!(
                world.query::<()>().count(),
                1,
                "rebinding spawns no duplicate entity"
            );
        }

        // The rebound shell drives ticks from now on.
        g.tick_once();
        assert_eq!(counter.load(Ordering::SeqCst), 1, "rebound shell ticks");
    }

    /// Two instances of one type rebind to their own two entities, each once
    /// — tags are consumed in order, never double-claimed.
    #[test]
    fn two_instances_rebind_one_to_one_in_registration_order() {
        let mut g = game();
        let a = g.register_actor(Probe::default());
        let b = g.register_actor(Probe::default());
        assert_ne!(a, b);

        g.begin_script_reload();
        let a2 = g.register_actor(Probe::default());
        let b2 = g.register_actor(Probe::default());

        assert!(
            (a2 == a && b2 == b) || (a2 == b && b2 == a),
            "each surviving tag claimed exactly once: {a:?}/{b:?} -> {a2:?}/{b2:?}"
        );

        // Third registration finds no tag left ⇒ fresh spawn.
        let c = g.register_actor(Probe::default());
        assert_ne!(c, a);
        assert_ne!(c, b);
    }

    /// An unknown type (class added between sessions) spawns fresh even mid-
    /// reload; a removed class leaves its entity orphaned but intact — data
    /// outlives behavior (cleanup policy belongs to a later phase).
    #[test]
    fn reload_spawns_new_types_and_leaves_removed_ones_orphaned() {
        struct Newcomer;
        impl Actor for Newcomer {}

        let mut g = game();
        let gone = g.register_actor(Probe::default());
        {
            let mut store = g.scene_store.write();
            store.world_mut().remove::<ScriptTag>(gone);
        }

        g.begin_script_reload();
        let added = g.register_actor(Newcomer);
        assert_ne!(added, gone, "no tag to claim ⇒ fresh spawn");

        assert!(
            g.scene_store.read().world().is_alive(gone),
            "orphaned entity keeps living; nothing despawns behind the user's back"
        );
    }
}
