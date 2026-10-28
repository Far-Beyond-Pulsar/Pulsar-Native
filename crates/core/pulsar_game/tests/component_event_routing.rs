//! Component callbacks retain owner compatibility while exposing instance-directed events.

use engine_class_derive::{engine_class, register_component_runtime, register_world_component};
use pulsar_events::{
    gamma::{Channel, DynValue, EventDescriptor, FieldType, SubscribeOptions, SyncSubscription},
    EventCategory, EventHub, FlushPoint,
};
use pulsar_scenedb::World;
use pulsar_world_registry::{
    attach_component, pulsar_scene_model::attachments::NewInstance, tick_live_components,
    ComponentContext, ComponentPayload, ComponentRuntimeState,
};
use std::sync::{Arc, Mutex};

const EVENT: &str = "routing::RoutingProbe.pulse";

#[engine_class(
    category = "Test",
    default,
    clone,
    debug,
    serialize,
    deserialize,
    no_register
)]
pub struct RoutingProbe {
    #[property]
    pub value: i64,
}

#[register_world_component]
impl RoutingProbe {}

#[register_component_runtime]
impl RoutingProbe {
    fn tick(&mut self, context: &mut ComponentContext, _delta_seconds: f32) {
        context
            .events
            .emit_named(EVENT, vec![DynValue::I64(self.value)])
            .unwrap();
    }
}

fn hub() -> (EventHub, u64) {
    let hub = EventHub::new();
    let descriptor = EventDescriptor::dynamic_in(
        "routing",
        "RoutingProbe.pulse",
        vec![("value".to_owned(), FieldType::I64)],
    )
    .unwrap();
    let id = descriptor.id;
    hub.register(descriptor, EventCategory::Custom("Routing".into()))
        .unwrap();
    (hub, id)
}

fn listen(hub: &EventHub, id: u64, channel: Channel) -> (SyncSubscription, Arc<Mutex<Vec<i64>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let output = Arc::clone(&seen);
    let subscription =
        hub.bus()
            .subscribe_dyn(id, SubscribeOptions::channel(channel), move |event| {
                let DynValue::I64(value) = event.fields[0] else {
                    panic!("unexpected payload");
                };
                output.lock().unwrap().push(value);
            });
    (subscription, seen)
}

#[test]
fn live_callbacks_reach_the_owner_and_only_the_emitting_instance_after_flush() {
    let (hub, id) = hub();
    let mut world = World::new();
    let owner = world.spawn();
    let unrelated = world.spawn();
    let mut attach = |value| {
        attach_component(
            &mut world,
            owner,
            NewInstance::new("RoutingProbe"),
            ComponentPayload::Value(Box::new(RoutingProbe { value })),
        )
        .unwrap()
    };
    let a = attach(11);
    let b = attach(22);
    let channels = [
        Channel::Entity(owner.bits()),
        Channel::Entity(a.bits()),
        Channel::Entity(b.bits()),
        Channel::Entity(unrelated.bits()),
        Channel::Global,
    ];
    let listeners: Vec<_> = channels
        .into_iter()
        .map(|channel| listen(&hub, id, channel))
        .collect();

    tick_live_components(
        &mut world,
        &hub,
        0.016,
        1,
        &mut ComponentRuntimeState::default(),
    );
    assert!(listeners
        .iter()
        .all(|(_, seen)| seen.lock().unwrap().is_empty()));
    hub.flush(FlushPoint::AfterScripts);

    let received: Vec<_> = listeners
        .iter()
        .map(|(_, seen)| seen.lock().unwrap().clone())
        .collect();
    let mut owner_values = received[0].clone();
    owner_values.sort();
    assert_eq!(owner_values, [11, 22]);
    assert_eq!(received[1], [11]);
    assert_eq!(received[2], [22]);
    assert!(received[3].is_empty());
    assert!(received[4].is_empty());
}

#[test]
fn legacy_contexts_stay_owner_only_and_matching_owner_instance_does_not_duplicate() {
    let (hub, id) = hub();
    let mut world = World::new();
    let owner = world.spawn();
    let instance = world.spawn();
    let (_owner_subscription, owner_values) = listen(&hub, id, Channel::Entity(owner.bits()));
    let (_instance_subscription, instance_values) =
        listen(&hub, id, Channel::Entity(instance.bits()));
    let context = ComponentContext::new(owner, &hub);
    assert_eq!(context.entity, owner);
    assert!(std::ptr::eq(context.events.hub(), &hub));
    context
        .events
        .emit_named(EVENT, vec![DynValue::I64(1)])
        .unwrap();
    ComponentContext::for_instance(owner, owner, &hub)
        .events
        .emit_named(EVENT, vec![DynValue::I64(2)])
        .unwrap();
    hub.flush(FlushPoint::AfterScripts);
    assert_eq!(*owner_values.lock().unwrap(), [1, 2]);
    assert!(instance_values.lock().unwrap().is_empty());
}
