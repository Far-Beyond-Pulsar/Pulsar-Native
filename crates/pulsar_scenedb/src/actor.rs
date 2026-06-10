use pulsar_core::GameTime;

use crate::entity::Entity;
use crate::world::World;

/// Trait for autonomous objects that receive lifecycle callbacks.
///
/// Actors are registered in an [`ActorRegistry`] and are driven by
/// [`TickLoop`](https://docs.rs/pulsar_game/latest/pulsar_game/tick/struct.TickLoop.html).
/// Each actor is backed by a single entity in the [`World`].
///
/// # Lifecycle
///
/// 1. `begin_play` â€” called once when registered (after the world is ready).
/// 2. `tick` â€” called every frame with the current `GameTime`.
/// 3. `end_play` â€” called once on deregistration or shutdown.
///
/// # Example
///
/// ```
/// use pulsar_scenedb::{Actor, World, Entity, GameTime};
///
/// struct Player;
///
/// impl Actor for Player {
///     fn begin_play(&mut self, entity: Entity, world: &mut World) {
///         // Spawn child entities, attach components, etc.
///     }
///     fn tick(&mut self, entity: Entity, world: &mut World, time: GameTime) {
///         // Read input, apply movement, etc.
///     }
///     fn end_play(&mut self, entity: Entity, world: &mut World) {
///         // Clean up resources.
///     }
/// }
/// ```
pub trait Actor: Send + Sync + 'static {
    /// Called once after the actor is registered with the world.
    ///
    /// Guaranteed to fire after the primary window's GPU context exists
    /// (when used via [`TickLoop`]).
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {}

    /// Called once when the actor is deregistered or the engine shuts down.
    fn end_play(&mut self, _entity: Entity, _world: &mut World) {}

    /// Called every tick with the current simulation time.
    fn tick(&mut self, _entity: Entity, _world: &mut World, _time: GameTime) {}
}

pub(crate) struct ActorEntry {
    pub entity: Entity,
    pub actor: Box<dyn Actor>,
    pub alive: bool,
}

/// Registry of active [`Actor`] instances.
///
/// Owns the boxed actor objects and drives their lifecycle.  On each tick
/// every alive actor receives `tick()`.  Actors can be deregistered by
/// entity handle.
///
/// The registry is owned by [`TickLoop`] and iterated together with the
/// ECS [`Schedule`](crate::Schedule).
#[derive(Default)]
pub struct ActorRegistry {
    pub(crate) entries: Vec<ActorEntry>,
}

impl ActorRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new actor, spawning its backing entity and calling
    /// [`Actor::begin_play`].
    ///
    /// Returns the entity handle assigned to this actor.
    pub fn register<A: Actor>(&mut self, mut actor: A, world: &mut World) -> Entity {
        let entity = world.spawn();
        actor.begin_play(entity, world);
        self.entries.push(ActorEntry {
            entity,
            actor: Box::new(actor),
            alive: true,
        });
        entity
    }

    /// Deregister an actor by entity handle.
    ///
    /// Calls [`Actor::end_play`], despawns the entity, and removes the
    /// entry from the registry.  Safe to call multiple times â€” second call
    /// is a no-op.
    pub fn deregister(&mut self, entity: Entity, world: &mut World) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.entity == entity && e.alive)
        {
            entry.actor.end_play(entity, world);
            entry.alive = false;
            world.despawn(entity);
        }
        self.entries.retain(|e| e.alive);
    }

    /// Call [`Actor::tick`] on every alive actor.
    pub fn tick_all(&mut self, world: &mut World, time: GameTime) {
        for entry in &mut self.entries {
            if entry.alive {
                profiling::profile_scope!(format!("Actor::Tick::{}", entry.entity));
                entry.actor.tick(entry.entity, world, time);
            }
        }
    }
}
