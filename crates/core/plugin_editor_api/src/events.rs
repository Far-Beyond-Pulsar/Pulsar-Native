//! A thin namespaced wrapper over Gamma's real API, for engine plugins.

use gamma::{
    Channel, DynEvent, DynValue, EventDescriptor, FieldType, RegistryError, SyncEventBus,
    SubscribeOptions, SyncSubscription,
};

#[derive(Debug)]
pub enum PluginBusError {
    Registry(RegistryError),
    UnknownEvent,
    Publish(gamma::DynEventError),
}

#[derive(Clone)]
pub struct PluginEventBus {
    bus: SyncEventBus,
}

impl PluginEventBus {
    pub fn new() -> Self {
        Self { bus: SyncEventBus::new() }
    }

    /// Declare an event. `namespace: None` is the main namespace.
    pub fn declare(
        &self,
        namespace: Option<&str>,
        event: &str,
        fields: &[(&str, FieldType)],
    ) -> Result<(), PluginBusError> {
        let fields = fields.iter().map(|&(n, t)| (n, t));
        let desc = match namespace {
            Some(ns) => EventDescriptor::dynamic_in(ns, event, fields),
            None => Ok(EventDescriptor::dynamic(event, fields)),
        }
        .map_err(PluginBusError::Registry)?;
        self.bus.register_descriptor(desc).map(drop).map_err(PluginBusError::Registry)
    }

    pub fn lookup(&self, namespace: Option<&str>, event: &str) -> Option<u64> {
        let d = match namespace {
            Some(ns) => self.bus.descriptor_by_name_in(ns, event),
            None => self.bus.descriptor_by_name(event),
        };
        d.map(|d| d.id)
    }

    pub fn publish(
        &self,
        namespace: Option<&str>,
        event: &str,
        values: Vec<DynValue>,
    ) -> Result<(), PluginBusError> {
        let id = self.lookup(namespace, event).ok_or(PluginBusError::UnknownEvent)?;
        self.bus
            .publish_dyn(Channel::Global, &DynEvent::new(id, values))
            .map_err(PluginBusError::Publish)
    }

    /// The handler stays registered while the returned handle lives.
    pub fn subscribe(
        &self,
        namespace: Option<&str>,
        event: &str,
        handler: impl Fn(&DynEvent) + Send + Sync + 'static,
    ) -> Result<SyncSubscription, PluginBusError> {
        let id = self.lookup(namespace, event).ok_or(PluginBusError::UnknownEvent)?;
        Ok(self.bus.subscribe_dyn(id, SubscribeOptions::default(), handler))
    }
}


#[test]
fn test_plugin_events() {
    let bus = PluginEventBus::new();
    bus.declare(Some("engine"), "Tick", &[("dt", FieldType::F64)]).unwrap();
    bus.declare(None, "Tick", &[("dt", FieldType::F64)]).unwrap(); // main, distinct
    assert!(bus.declare(Some("main"), "Tick", &[]).is_err()); // reserved

    let sub = bus
        .subscribe(Some("engine"), "Tick", |e| println!("engine tick: {:?}", e.fields))
        .unwrap();
    bus.publish(Some("engine"), "Tick", vec![DynValue::F64(0.016)]).unwrap(); // printed
    bus.publish(None, "Tick", vec![DynValue::F64(0.016)]).unwrap(); // main: not delivered
    sub.unsubscribe(); // unsubscribing is done through the handle
}

impl Default for PluginEventBus {
    fn default() -> Self {
        Self::new()
    }
}
