use crate::entity::Entity;
use crate::time::GameTime;
use crate::world::World;

/// Lifecycle callbacks for game objects.
///
/// Implementing `Actor` on a struct and registering it with the `ActorRegistry`
/// gives it `begin_play`, `end_play`, and `tick` invocations driven by the
/// `TickLoop`.
///
/// All methods have default no-op implementations so you only override what you need.
pub trait Actor: Send + Sync + 'static {
    /// Called once when this actor is added to the world.
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {}

    /// Called once immediately before this actor is removed from the world.
    fn end_play(&mut self, _entity: Entity, _world: &mut World) {}

    /// Called every tick while this actor is alive.
    fn tick(&mut self, _entity: Entity, _world: &mut World, _time: GameTime) {}
}

/// Registered actor entry — owns the boxed `Actor` and its `Entity`.
pub(crate) struct ActorEntry {
    pub entity: Entity,
    pub actor: Box<dyn Actor>,
    pub alive: bool,
}

/// Registry of all live actors.
///
/// The `TickLoop` holds one of these and drives the lifecycle calls.
#[derive(Default)]
pub struct ActorRegistry {
    pub(crate) entries: Vec<ActorEntry>,
}

impl ActorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an actor, calling `begin_play` immediately.
    pub fn register<A: Actor>(
        &mut self,
        mut actor: A,
        world: &mut World,
    ) -> Entity {
        let entity = world.spawn();
        actor.begin_play(entity, world);
        self.entries.push(ActorEntry {
            entity,
            actor: Box::new(actor),
            alive: true,
        });
        entity
    }

    /// Deregister an actor, calling `end_play` then despawning its entity.
    pub fn deregister(&mut self, entity: Entity, world: &mut World) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.entity == entity && e.alive) {
            entry.actor.end_play(entity, world);
            entry.alive = false;
            world.despawn(entity);
        }
        self.entries.retain(|e| e.alive);
    }

    /// Tick all live actors.
    pub(crate) fn tick_all(&mut self, world: &mut World, time: GameTime) {
        for entry in &mut self.entries {
            if entry.alive {
                entry.actor.tick(entry.entity, world, time);
            }
        }
    }
}
