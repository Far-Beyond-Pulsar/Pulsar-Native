use pulsar_core::GameTime;

use crate::entity::Entity;
use crate::world::World;

pub trait Actor: Send + Sync + 'static {
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {}
    fn end_play(&mut self, _entity: Entity, _world: &mut World) {}
    fn tick(&mut self, _entity: Entity, _world: &mut World, _time: GameTime) {}
}

pub(crate) struct ActorEntry {
    pub entity: Entity,
    pub actor: Box<dyn Actor>,
    pub alive: bool,
}

#[derive(Default)]
pub struct ActorRegistry {
    pub(crate) entries: Vec<ActorEntry>,
}

impl ActorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

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

    pub fn tick_all(&mut self, world: &mut World, time: GameTime) {
        for entry in &mut self.entries {
            if entry.alive {
                profiling::profile_scope!(format!("Actor::Tick::{}", entry.entity));
                entry.actor.tick(entry.entity, world, time);
            }
        }
    }
}
