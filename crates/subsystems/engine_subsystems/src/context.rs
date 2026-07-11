/// Shared context provided to all subsystems during initialization.
///
/// Currently empty — reserved for future cross-subsystem services.
/// No async runtime handle is exposed because tokio is not DLL-safe
/// and the engine uses GPUI's own async primitives.
///
/// # Usage
///
/// `SubsystemContext` is created once in `EngineBackend::inject_plugin_subsystems`
/// (`engine_backend/src/lib.rs:90`) and passed to each subsystem's `init()`.
/// Every subsystem — whether built-in or plugin-provided — receives the same
/// context instance.
#[derive(Clone, Debug)]
pub struct SubsystemContext {}

impl SubsystemContext {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for SubsystemContext {
    fn default() -> Self {
        Self::new()
    }
}
