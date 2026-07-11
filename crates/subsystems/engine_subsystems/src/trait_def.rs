use std::any::Any;

use crate::{SubsystemContext, SubsystemError, SubsystemId};

/// Trait that all engine subsystems must implement.
///
/// A subsystem is an independently lifecycle-managed engine component
/// that can declare dependencies on other subsystems.
///
/// # Type erasure with `Any`
///
/// The `Subsystem` trait extends `Any`, allowing consumers to downcast
/// from `&dyn Subsystem` back to the concrete type:
///
/// ```rust,ignore
/// use std::any::Any;
///
/// let ss = registry.get(MY_SUBSYSTEM_ID).unwrap();
/// let any: &dyn Any = ss;
/// let concrete = any.downcast_ref::<MySubsystem>().unwrap();
/// ```
///
/// # Lifecycle
///
/// 1. **Registration** — `SubsystemRegistry::register()` or `register_boxed()`
/// 2. **Initialization** — `init()` called during `init_all()` in dependency order
/// 3. **Per-frame update** — `on_frame()` called each tick
/// 4. **Shutdown** — `shutdown()` called in reverse init order
///
/// # Implementing
///
/// ```rust,ignore
/// use engine_subsystems::*;
///
/// struct MySubsystem;
///
/// impl Subsystem for MySubsystem {
///     fn id(&self) -> SubsystemId { SubsystemId::new("my_subsystem") }
///     fn dependencies(&self) -> Vec<SubsystemId> { vec![] }
///     fn init(&mut self, _ctx: &SubsystemContext) -> Result<(), SubsystemError> { Ok(()) }
///     fn shutdown(&mut self) -> Result<(), SubsystemError> { Ok(()) }
/// }
/// ```
pub trait Subsystem: Send + Sync + Any {
    /// Unique identifier for this subsystem.
    fn id(&self) -> SubsystemId;

    /// Subsystems that must be initialized before this one.
    ///
    /// Returned IDs are resolved against the registry via topological sort
    /// (Kahn's algorithm). An empty vec means no dependencies.
    fn dependencies(&self) -> Vec<SubsystemId>;

    /// Initialize the subsystem.
    ///
    /// Called once during startup. Use this to allocate GPU resources,
    /// spawn background tasks, load assets, or connect to services.
    fn init(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError>;

    /// Shut down the subsystem and release resources.
    ///
    /// Called once during engine teardown, in reverse initialization order.
    fn shutdown(&mut self) -> Result<(), SubsystemError>;

    /// Called once per frame after all subsystems are initialized.
    ///
    /// `delta_time` is the time in seconds since the last frame.
    /// Default implementation is a no-op.
    fn on_frame(&mut self, _delta_time: f32) {}
}
