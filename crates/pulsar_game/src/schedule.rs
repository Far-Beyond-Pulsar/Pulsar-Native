use crate::time::GameTime;
use crate::world::World;

/// A named system function that runs against a `World` every tick.
pub type SystemFn = Box<dyn FnMut(&mut World, GameTime) + Send + 'static>;

/// An ordered list of systems that runs sequentially.
///
/// Systems execute in the order they were added.  For parallel execution,
/// spawn them as async tasks in the `TaskPool` from within a system.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<(String, SystemFn)>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a system to the schedule.
    pub fn add_system<S>(&mut self, name: impl Into<String>, system: S) -> &mut Self
    where
        S: FnMut(&mut World, GameTime) + Send + 'static,
    {
        self.systems.push((name.into(), Box::new(system)));
        self
    }

    /// Run all systems in insertion order.
    pub fn run(&mut self, world: &mut World, time: GameTime) {
        profiling::profile_scope!("Schedule::run");
        for (_name, system) in &mut self.systems {
            system(world, time);
        }
    }

    /// Number of systems in this schedule.
    #[inline]
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}
