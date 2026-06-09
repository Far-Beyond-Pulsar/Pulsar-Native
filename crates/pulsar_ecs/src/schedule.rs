use pulsar_core::GameTime;

use crate::world::World;

pub type SystemFn = Box<dyn FnMut(&mut World, GameTime) + Send + 'static>;

#[derive(Default)]
pub struct Schedule {
    systems: Vec<(String, SystemFn)>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_system<S>(&mut self, name: impl Into<String>, system: S) -> &mut Self
    where
        S: FnMut(&mut World, GameTime) + Send + 'static,
    {
        self.systems.push((name.into(), Box::new(system)));
        self
    }

    pub fn run(&mut self, world: &mut World, time: GameTime) {
        profiling::profile_scope!("Schedule::run");
        for (name, system) in &mut self.systems {
            profiling::profile_scope!(format!("Schedule::System::{}", name));
            system(world, time);
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}
