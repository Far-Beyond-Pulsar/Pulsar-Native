//! Attaching the event hub and declaring a class's events on it.

use std::sync::Arc;

use pulsar_script_vm::{EventCatalog, LinkedSubscription, Module};

use super::ScriptRuntime;
use crate::event_host::{declare_events, EventHost};
use crate::reload::RelinkReport;
use crate::RuntimeError;

impl ScriptRuntime {
    /// Attach (or detach) the engine's event hub. Every loaded class's
    /// events are declared on it and every class relinks against its
    /// catalog.
    pub fn set_event_host(&mut self, events: Option<Arc<dyn EventHost>>) -> RelinkReport {
        self.events = events;
        if let Some(events) = &self.events {
            for class in self.classes.values() {
                if let Err(error) = declare_events(events.as_ref(), class.program.module()) {
                    tracing::error!("{error}");
                }
            }
        }
        self.relink_all()
    }

    pub fn event_host(&self) -> Option<&Arc<dyn EventHost>> {
        self.events.as_ref()
    }

    pub(crate) fn catalog(&self) -> Option<&dyn EventCatalog> {
        self.events.as_deref().map(|e| e as &dyn EventCatalog)
    }

    /// Declare `module`'s events on the event host now (before loading it),
    /// so classes loaded earlier can subscribe to them. No-op without a
    /// host.
    pub fn declare_events(&self, module: &Module) -> Result<(), RuntimeError> {
        match &self.events {
            Some(events) => declare_events(events.as_ref(), module),
            None => Ok(()),
        }
    }

    /// The checked event subscriptions of a loaded class.
    pub fn subscriptions(&self, class: &str) -> Option<&[LinkedSubscription]> {
        Some(self.classes.get(class)?.program.subscriptions())
    }
}
