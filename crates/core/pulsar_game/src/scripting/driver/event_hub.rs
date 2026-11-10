//! Event hub integration of the driver: attaching the hub, declaring project
//! events and (re)subscribing script instances to them. Entry point:
//! [`ScriptDriver::attach_events`].

use pulsar_scenedb::{Entity, World};

use super::config::{module_file, read_module_bytes};
use super::{DriverReport, ScriptDriver};
use crate::scripting::events::ScriptEvents;
use pulsar_events::EventHub;

impl ScriptDriver {
    /// Put the scripts on `hub` (the session's event hub): declared events
    /// register there, handlers link against it, natives publish to it and
    /// every instance, running or started later, subscribes its handlers.
    /// See [`super::events`].
    pub fn attach_events(&mut self, hub: EventHub) {
        if self.events.as_ref().is_some_and(|e| e.hub().ptr_eq(&hub)) {
            return;
        }
        if let Some(mut old) = self.events.take() {
            old.clear();
        }
        // Before any class links: each plugin's session hook declares the
        // events scripts may handle (the input plugin's `input::<Action>`),
        // so a Blueprint can handle one whichever loads first.
        for hook in engine_backend::scene::game_hooks::game_sessions() {
            if let Some(begin) = hook.begin {
                begin(&hub, &self.project_root);
            }
        }
        let events = ScriptEvents::new(hub);
        for registration in pulsar_world_registry::component_event_registrations() {
            let declaration = (registration.declaration)();
            if let Err(error) = events
                .bridge()
                .register_event_decl(registration.class_name, &declaration)
            {
                tracing::warn!(event = %declaration.name, %error, "component event could not be linked to Blueprint");
            }
        }
        for entry in self.registry.entries() {
            events.bridge().add_class(&entry.name, entry.id.as_str());
        }
        let report = self.runtime.set_event_host(Some(events.host()));
        for (class, error) in report.failed {
            tracing::warn!(class = %class, "script class does not link against the event hub: {error}");
        }
        self.events = Some(events);
        self.predeclare_project_events();
        // Component-scoped subscriptions are resolved at the next script
        // phase, when the driver has a read/write borrow of the live World.
        self.resubscribe(None, None);
    }

    /// Declare the events of every compiled class in the project on the
    /// hub, so a class can subscribe to another class's events whichever
    /// loads first.
    pub(super) fn predeclare_project_events(&self) {
        if self.events.is_none() {
            return;
        }
        for entry in self.registry.entries() {
            let path = module_file(entry);
            let Some(Ok(bytes)) = read_module_bytes(&path) else {
                continue;
            };
            match pulsar_script_vm::Module::decode(&bytes) {
                Ok(module) => {
                    if let Err(error) = self.runtime.declare_events(&module) {
                        tracing::warn!("{error}");
                    }
                }
                Err(error) => {
                    tracing::debug!(path = %path.display(), "unreadable script module: {error}")
                }
            }
        }
    }

    /// The attached event integration.
    pub fn events(&self) -> Option<&ScriptEvents> {
        self.events.as_ref()
    }

    /// The attached event hub.
    pub fn event_hub(&self) -> Option<&EventHub> {
        self.events.as_ref().map(ScriptEvents::hub)
    }

    /// Name the level `LevelLoaded` reports.
    pub fn set_level_name(&mut self, level: impl Into<String>) {
        self.level = level.into();
    }

    /// (Re)subscribe the live instances of `class` (the runtime's class
    /// name), or of every class when `None`.
    pub(super) fn resubscribe(&mut self, class: Option<&str>, world: Option<&World>) {
        let Some(events) = self.events.as_mut() else {
            return;
        };
        let targets: Vec<(String, String, String, Option<Entity>)> = self
            .runtime
            .instance_ids()
            .iter()
            .filter_map(|id| {
                let class_name = self.runtime.class_of(id)?;
                if class.is_some_and(|c| c != class_name) {
                    return None;
                }
                let guid = self
                    .loaded
                    .iter()
                    .find(|(_, name)| name.as_str() == class_name)
                    .map(|(guid, _)| guid.as_str().to_owned())
                    .unwrap_or_default();
                Some((
                    id.clone(),
                    class_name.to_owned(),
                    guid,
                    self.by_instance.get(id).copied(),
                ))
            })
            .collect();
        for (id, class_name, guid, entity) in targets {
            // Timers and queued handler calls survive a class reload.
            for failure in events.resubscribe(&self.runtime, &id, &class_name, &guid, entity, world)
            {
                tracing::warn!("{failure}");
            }
        }
    }

    pub(super) fn subscribe_instance(
        &mut self,
        id: &str,
        class: &str,
        guid: &str,
        entity: Option<Entity>,
        world: Option<&World>,
        report: &mut DriverReport,
    ) {
        if let Some(events) = self.events.as_mut() {
            events.bridge().add_class(class, guid);
            for failure in events.subscribe(&self.runtime, id, class, guid, entity, world) {
                tracing::warn!("{failure}");
                report.failures.push(failure);
            }
        }
    }

    pub(super) fn publish<T: pulsar_events::gamma::Event + Send>(
        &self,
        channel: pulsar_events::gamma::Channel,
        event: T,
    ) {
        if let Some(events) = &self.events {
            events.hub().publish(channel, event);
        }
    }
}
