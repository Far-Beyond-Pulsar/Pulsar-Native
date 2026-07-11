use crate::plugin::EditorPlugin;

// ============================================================================
// Plugin Subsystem Extension Trait
// ============================================================================

/// Trait for plugins that provide engine subsystems.
///
/// Subsystems are type-erased engine components (renderer, physics, audio, etc.)
/// that participate in the engine's lifecycle (init, shutdown, per-frame update).
/// Consumers downcast via `std::any::Any` to get their concrete type.
///
/// # Example
///
/// ```rust,ignore
/// impl EditorPluginSubsystems for MyPlugin {
///     fn subsystems(&self) -> Vec<Box<dyn engine_subsystems::Subsystem>> {
///         vec![Box::new(MyCustomRenderer::new())]
///     }
/// }
/// ```
pub trait EditorPluginSubsystems: EditorPlugin {
    /// Returns all subsystems provided by this plugin.
    fn subsystems(&self) -> Vec<Box<dyn engine_subsystems::Subsystem>>;
}

// Re-export subsystem types for plugin convenience.
// Well-known subsystem IDs are not defined centrally — each consumer
// that needs to downcast uses `SubsystemId::new("...")` directly.
pub use engine_subsystems::{
    Subsystem, SubsystemContext, SubsystemError, SubsystemId, SubsystemRegistry,
};
