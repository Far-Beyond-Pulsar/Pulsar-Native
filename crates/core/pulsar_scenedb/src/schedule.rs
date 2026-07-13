use pulsar_core::GameTime;

use crate::world::World;

/// Type alias for a boxed system function.
///
/// Systems receive `&mut World` and `GameTime` each tick.  They are stored
/// in a [`Schedule`] and executed in registration order.
pub type SystemFn = Box<dyn FnMut(&mut World, GameTime) + Send + 'static>;

/// An ordered list of systems executed sequentially each tick.
///
/// Systems are stored by name (for profiling) and run in insertion order.
/// There is no parallel execution or dependency graph â€” systems that need
/// ordering should be added in the desired sequence.
///
/// # Example
///
/// ```
/// use pulsar_scenedb::{Schedule, World, GameTime};
///
/// let mut schedule = Schedule::new();
/// schedule.add_system("physics", |world, time| {
///     // update velocities, apply forces, etc.
/// });
/// schedule.add_system("render", |world, time| {
///     // sync transform data to GPU, etc.
/// });
/// ```
#[derive(Default)]
pub struct Schedule {
    systems: Vec<(String, SystemFn)>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a system and append it to the execution list.
    ///
    /// `name` is used in profiling scopes; it does not affect semantics.
    /// Systems execute in the order they are added.
    pub fn add_system<S>(&mut self, name: impl Into<String>, system: S) -> &mut Self
    where
        S: FnMut(&mut World, GameTime) + Send + 'static,
    {
        self.systems.push((name.into(), Box::new(system)));
        self
    }

    /// Execute all registered systems in order.
    ///
    /// Each system receives `&mut world` and `time`.  Profiling scopes are
    /// emitted for the overall run and for each named system.
    pub fn run(&mut self, world: &mut World, time: GameTime) {
        profiling::profile_scope!("Schedule::run");
        for (name, system) in &mut self.systems {
            profiling::profile_scope!(format!("Schedule::System::{}", name));
            system(world, time);
        }
    }

    /// Returns the number of registered systems.
    #[inline]
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// Returns `true` if no systems are registered.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}
