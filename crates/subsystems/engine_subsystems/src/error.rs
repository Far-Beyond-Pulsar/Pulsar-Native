/// Errors that can occur during subsystem operations.
///
/// Returned by [`SubsystemRegistry`](crate::SubsystemRegistry) methods
/// and by individual [`Subsystem`](crate::Subsystem) lifecycle hooks.
///
/// # Error handling patterns in the engine
///
/// - **`InitFailed`** — wraps errors from a subsystem's `init()`. The
///   registry prefixes the error with the subsystem name so callers can
///   identify which subsystem failed. `EngineBackend::inject_plugin_subsystems`
///   maps individual init failures through this variant.
///
/// - **`ShutdownFailed`** — wraps errors from a subsystem's `shutdown()`.
///
/// - **`DependencyCycle`** — caught by `resolve_dependencies()` via Kahn's
///   algorithm. Lists the IDs involved in the cycle for debugging.
///
/// - **`MissingDependency`** — a subsystem declared a dependency that was
///   never registered. Caught during topological sort.
///
/// - **`AlreadyRegistered`** — two subsystems with the same ID tried to
///   register. `EngineBackend` logs this at debug level and silently keeps
///   the first registration (first-registered-wins policy).
///
/// - **`NotFound`** — `get()` / `get_mut()` was called with an ID that
///   isn't in the registry.
#[derive(Debug, thiserror::Error)]
pub enum SubsystemError {
    #[error("Subsystem initialization failed: {0}")]
    InitFailed(String),

    #[error("Subsystem shutdown failed: {0}")]
    ShutdownFailed(String),

    #[error("Dependency cycle detected involving: {subsystems:?}")]
    DependencyCycle { subsystems: Vec<&'static str> },

    #[error("Missing dependency: {dependency} required by {subsystem}")]
    MissingDependency {
        subsystem: &'static str,
        dependency: &'static str,
    },

    #[error("Subsystem already registered: {0}")]
    AlreadyRegistered(&'static str),

    #[error("Subsystem not found: {0}")]
    NotFound(&'static str),
}
