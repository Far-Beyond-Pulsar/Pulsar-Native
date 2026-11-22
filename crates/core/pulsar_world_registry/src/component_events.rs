//! Component events: Gamma registrations, the per-entity outbox and its flush, the callback context, and value-dependent event sources.

use pulsar_scenedb::{Entity, World};

/// A stable Gamma event descriptor registered by a component event macro.
pub struct ComponentEventRegistration {
    pub class_name: &'static str,
    pub descriptor: fn() -> pulsar_events::gamma::EventDescriptor,
    pub declaration: fn() -> pulsar_script_vm::EventDecl,
}

inventory::collect!(ComponentEventRegistration);

/// Register all link-time component event descriptors on a session hub.
/// Calling this more than once is safe when descriptors are identical.
/// Every descriptor is tried: one that the hub refuses (a name or id clash)
/// does not keep the ones after it off the hub. The error lists every
/// refused descriptor.
pub fn register_component_events(hub: &pulsar_events::EventHub) -> Result<(), String> {
    let mut failures = Vec::new();
    for registration in crate::runtime::events().iter().copied() {
        let descriptor = (registration.descriptor)();
        let name = descriptor.name.to_string();
        if let Err(error) = hub.register(descriptor, pulsar_events::EventCategory::Gameplay) {
            failures.push(format!("`{name}` ({}): {error}", registration.class_name));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// Link-time component event declarations used to install typed local
/// signatures in the Blueprint bridge after their Gamma descriptors exist.
pub fn component_event_registrations() -> impl Iterator<Item = &'static ComponentEventRegistration>
{
    crate::runtime::events().iter().copied()
}

/// Short-lived, per-entity outbox for events emitted by reflected World
/// methods. It is transport state only: it does not mirror component fields
/// and is removed as soon as queued messages have been handed to Gamma.
#[derive(Default)]
pub struct ComponentEventOutbox {
    pub events: Vec<QueuedComponentEvent>,
}

#[derive(Clone)]
pub struct QueuedComponentEvent {
    pub name: String,
    pub fields: Vec<pulsar_events::gamma::DynValue>,
}

/// Append a component event to the owner's outbox. Delivery occurs at the
/// runtime's `AfterSystems` flush, after the World-mutating call returns.
pub fn queue_component_event(
    world: &mut World,
    entity: Entity,
    name: impl Into<String>,
    fields: Vec<pulsar_events::gamma::DynValue>,
) {
    let event = QueuedComponentEvent {
        name: name.into(),
        fields,
    };
    if let Some(mut outbox) = world.get_mut::<ComponentEventOutbox>(entity) {
        outbox.events.push(event);
        return;
    }
    world.insert(
        entity,
        ComponentEventOutbox {
            events: vec![event],
        },
    );
}

/// Event names [`flush_component_events`] has already warned about: an
/// event the hub refuses is refused every tick, so it is reported once.
static REFUSED_EVENTS_WARNED: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Publish queued outbox events after all World borrows for the emitting
/// phase have ended. An event the hub refuses (its name is not registered,
/// or its fields do not match the descriptor) is dropped with a warning
/// (once per event name): nothing registers events between phases, so
/// keeping it queued would only retry, and warn, every tick forever.
pub fn flush_component_events(world: &mut World, hub: &pulsar_events::EventHub) -> usize {
    profiling::profile_scope!("Lifecycle::flush_component_events");
    let entities: Vec<_> = world
        .query::<&ComponentEventOutbox>()
        .map(|(entity, _)| entity)
        .collect();
    let mut delivered = 0;
    for entity in entities {
        let Some(mut outbox) = world.remove::<ComponentEventOutbox>(entity) else {
            continue;
        };
        // A component instance's events are addressed to the instance (for
        // subscriptions to that component) and to its owning object (for
        // subscriptions to the object), as they were when both were one
        // entity.
        let owner = pulsar_scene_model::attachments::owner_of(world, entity);
        let channels: Vec<Entity> = std::iter::once(entity).chain(owner).collect();
        for event in outbox.events.drain(..) {
            let mut published = 0;
            for channel in &channels {
                match hub.publish_named(
                    pulsar_events::gamma::Channel::Entity(channel.bits()),
                    &event.name,
                    event.fields.clone(),
                ) {
                    Ok(()) => published += 1,
                    Err(error) => {
                        let first = REFUSED_EVENTS_WARNED
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(event.name.clone());
                        if first {
                            tracing::warn!(event = %event.name, %error, "component event refused by the hub; dropping it (reported once per event)");
                        }
                        break;
                    }
                }
            }
            if published > 0 {
                delivered += 1;
            }
        }
    }
    delivered
}

/// Services available to a live component callback. The component itself is
/// borrowed from SceneDB for the callback's duration. The context exposes
/// the owner and a deferred, owner-scoped event writer, but intentionally no
/// `World` reference: handlers can therefore never re-enter a World borrow.
pub struct ComponentContext<'a> {
    pub entity: Entity,
    pub events: ComponentEventWriter<'a>,
}

impl<'a> ComponentContext<'a> {
    #[doc(hidden)]
    pub fn for_instance(instance: Entity, owner: Entity, hub: &'a pulsar_events::EventHub) -> Self {
        Self {
            entity: owner,
            events: ComponentEventWriter {
                entity: owner,
                instance: Some(instance),
                hub,
            },
        }
    }

    #[doc(hidden)]
    pub fn new(entity: Entity, hub: &'a pulsar_events::EventHub) -> Self {
        Self {
            entity,
            events: ComponentEventWriter::new(entity, hub),
        }
    }
}

pub struct ComponentEventWriter<'a> {
    entity: Entity,
    instance: Option<Entity>,
    hub: &'a pulsar_events::EventHub,
}

impl<'a> ComponentEventWriter<'a> {
    pub fn hub(&self) -> &'a pulsar_events::EventHub {
        self.hub
    }

    #[doc(hidden)]
    pub fn new(entity: Entity, hub: &'a pulsar_events::EventHub) -> Self {
        Self {
            entity,
            instance: None,
            hub,
        }
    }

    /// queue a named event for the owner and, during a live callback, its
    /// component instance. Gamma never invokes handlers inside this borrow.
    pub fn emit_named(
        &self,
        name: &str,
        fields: Vec<pulsar_events::gamma::DynValue>,
    ) -> Result<(), String> {
        self.hub
            .publish_named(
                pulsar_events::gamma::Channel::Entity(self.entity.bits()),
                name,
                fields.clone(),
            )
            .map_err(|error| error.to_string())?;
        if let Some(instance) = self.instance.filter(|instance| *instance != self.entity) {
            self.hub
                .publish_named(
                    pulsar_events::gamma::Channel::Entity(instance.bits()),
                    name,
                    fields,
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Emit a component event with its original Rust payload type. The VM
    /// codec wraps it in a stable, versioned byte envelope before Gamma sees
    /// it; only bytes and the declared event name cross the DLL boundary.
    pub fn emit_value<T: Clone + Send + Sync + 'static>(
        &self,
        name: &str,
        payload_type: &'static str,
        payload: &T,
    ) -> Result<(), String> {
        let object = pulsar_script_vm::Object::new(payload_type, payload.clone());
        let bytes = pulsar_script_vm::TypeRegistry::global().encode_event_value(&object)?;
        self.emit_named(name, vec![pulsar_events::gamma::DynValue::Bytes(bytes)])
    }
}

/// An event a component offers scripts because of its values (a mode that
/// changes what it raises), not because of an asset it names.
///
/// The descriptor must be namespaced (`EventDescriptor::dynamic_in`): a
/// namespaced name is how a script editor tells an event the component's data
/// declares from one the component class declares statically
/// (`#[component_events]`), and it is delivered on the component's entity
/// channel like any other component event. The component raises it itself,
/// by that name.
#[derive(Clone, Debug)]
pub struct DynamicEvent {
    /// The node is titled "On <label>".
    pub label: String,
    /// The palette group under the component.
    pub group: String,
    pub descriptor: pulsar_events::gamma::EventDescriptor,
}

/// The events a component's data adds for scripts, from
/// `#[bp_event_source] fn events(&self) -> Vec<DynamicEvent>` in its
/// `#[world_component]` impl; submitted by that macro, not by hand.
///
/// Pure: the same values give the same events. A script editor calls it with a
/// component's saved data, so the events follow the property values the same way
/// the asset events follow the assets a property names.
pub struct ComponentEventSourceRegistration {
    pub class_name: &'static str,
    pub events: fn(&serde_json::Value) -> Vec<DynamicEvent>,
}

inventory::collect!(ComponentEventSourceRegistration);

/// The value-dependent event sources of `class_name`.
pub fn component_event_sources(
    class_name: &str,
) -> impl Iterator<Item = &'static ComponentEventSourceRegistration> + '_ {
    inventory::iter::<ComponentEventSourceRegistration>
        .into_iter()
        .filter(move |source| source.class_name == class_name)
}
