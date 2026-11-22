//! The event hub as the runtime sees it, and declaring a class's events on it.

use pulsar_script_vm::{EventCatalog, EventDecl, EventSink, Module};

use crate::RuntimeError;

/// The engine's event hub, as the runtime sees it: a sink for the event
/// natives, a catalog to link handlers against, and a registry for the
/// events classes declare.
pub trait EventHost: EventSink + EventCatalog + Send + Sync {
    /// Register event `decl`, declared by class (module) `class`.
    /// Registering the same declaration again is a no-op; a different
    /// event under the same name is an error.
    fn declare(&self, class: &str, decl: &EventDecl) -> Result<(), String>;
}

pub(crate) fn declare_events(events: &dyn EventHost, module: &Module) -> Result<(), RuntimeError> {
    for decl in &module.events {
        events
            .declare(&module.name, decl)
            .map_err(|reason| RuntimeError::EventDeclaration {
                class: module.name.clone(),
                event: decl.name.clone(),
                reason,
            })?;
    }
    Ok(())
}
