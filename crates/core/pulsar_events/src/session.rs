//! The running Play-in-Editor session's event hub, for editor plugins
//! (Pulsar-Native#942).
//!
//! A PIE game runs in its own dynamic library with its own
//! [`EventHub`](crate::EventHub). The editor's PIE host gets the hub's bus
//! as a Gamma [`RawBus`] through the optional PIE ABI entry point
//! `pulsar_pie_event_bus`, wraps it in a [`ForeignBus`] and announces it on
//! the editor's [host bus](crate::host):
//!
//! - [`announce_session_started`] when the game is running: every
//!   subscriber of [`subscribe_pie_sessions`] receives
//!   [`PieSessionEvent::Started`] with its **own** `ForeignBus` onto the
//!   game's hub (one strong reference each). Through it a plugin can
//!   subscribe to gameplay events and publish into the game (publishes are
//!   deferred to the game's next flush and show in the events panel).
//! - [`announce_session_stopping`] right before the game is shut down and
//!   its library unloaded: subscribers receive
//!   [`PieSessionEvent::Stopping`] and **must drop** that `ForeignBus` and
//!   every subscription made through it before returning. After that the
//!   game's code is gone; a handle kept past it would call into unloaded
//!   code.
//!
//! On the host bus the announcement is the dynamic event `PieSession` with
//! fields `phase` (`"started"` / `"stopping"`) and `bus` (the address of a
//! `RawBus` valid only during the synchronous delivery; each subscriber
//! copies it and takes its own reference).

use gamma::ffi::{ForeignBus, RawBus};
use gamma::{Channel, DynEvent, DynValue, EventDescriptor, FieldType, SubscribeOptions};

use crate::host::{HostBus, HostSubscription, host_bus};

/// What a session subscriber receives.
pub enum PieSessionEvent {
    /// A game is running; the bus is the subscriber's own handle onto its
    /// event hub.
    Started(ForeignBus),
    /// The game stops now: drop the bus and its subscriptions.
    Stopping,
}

/// The `PieSession` descriptor on the host bus.
pub fn descriptor() -> EventDescriptor {
    EventDescriptor::dynamic("PieSession", [("phase", FieldType::Str), ("bus", FieldType::U64)])
}

fn registered(bus: &HostBus) -> Option<u64> {
    let descriptor = descriptor();
    let id = descriptor.id;
    match bus.register_descriptor(&descriptor) {
        Ok(()) => Some(id),
        Err(error) => {
            tracing::error!("session bus: cannot register PieSession: {error}");
            None
        }
    }
}

fn publish_on(host: &HostBus, phase: &str, bus: u64) {
    let Some(id) = registered(host) else { return };
    let event = DynEvent::new(id, vec![DynValue::Str(phase.to_owned()), DynValue::U64(bus)]);
    if let Err(error) = host.publish_dyn(Channel::Global, &event) {
        tracing::error!("session bus: publish failed: {error}");
    }
}

/// Announce the session's hub, `game`, to every session subscriber.
pub fn announce_session_started(game: &ForeignBus) {
    announce_started_on(host_bus(), game);
}

fn announce_started_on(host: &HostBus, game: &ForeignBus) {
    // A temporary reference for the delivery; subscribers take their own.
    let raw = game.clone().into_raw();
    publish_on(host, "started", &raw as *const RawBus as u64);
    // SAFETY: `raw` is the reference taken just above, released once.
    drop(unsafe { ForeignBus::from_raw(raw) });
}

/// Tell session subscribers the game stops now. Returns after every
/// subscriber ran (delivery is synchronous).
pub fn announce_session_stopping() {
    publish_on(host_bus(), "stopping", 0);
}

/// Call `callback` for every session announcement until the returned
/// subscription is dropped. See the module doc for the contract.
pub fn subscribe_pie_sessions(callback: impl Fn(PieSessionEvent) + Send + Sync + 'static) -> HostSubscription {
    subscribe_on(host_bus(), callback)
}

fn subscribe_on(host: &HostBus, callback: impl Fn(PieSessionEvent) + Send + Sync + 'static) -> HostSubscription {
    let id = registered(host).unwrap_or_else(|| descriptor().id);
    host.subscribe_dyn(id, SubscribeOptions::default(), move |event| {
        let [DynValue::Str(phase), DynValue::U64(bus)] = event.fields.as_slice() else { return };
        match phase.as_str() {
            "started" if *bus != 0 => {
                // SAFETY: the announcer keeps the `RawBus` at this address
                // alive for the synchronous delivery; we copy it and take our
                // own strong reference, which the new `ForeignBus` owns.
                let raw = unsafe { std::ptr::read(*bus as *const RawBus) };
                if raw.abi_version != gamma::ffi::ABI_VERSION
                    || raw.struct_size != std::mem::size_of::<RawBus>() as u32
                {
                    tracing::warn!("session bus: the game's Gamma FFI version differs; not attached");
                    return;
                }
                unsafe { (raw.retain)(raw.ctx) };
                match unsafe { ForeignBus::from_raw(raw) } {
                    Ok(bus) => callback(PieSessionEvent::Started(bus)),
                    Err(error) => tracing::warn!("session bus: {error:?}"),
                }
            }
            "stopping" => callback(PieSessionEvent::Stopping),
            _ => {}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::KeyDown;
    use crate::{EventHub, FlushPoint};
    use std::sync::{Arc, Mutex};

    /// A plugin gets its own handle onto the session hub when the session
    /// starts, observes and sends gameplay events through it, and drops it
    /// when the session stops; nothing of it stays on the hub.
    #[test]
    fn plugins_attach_to_the_session_hub_and_detach_on_stop() {
        let host = HostBus::local();
        let plugin_view = unsafe { HostBus::foreign(host.export_raw().unwrap()) }.unwrap();
        let hub = EventHub::new();
        hub.set_tap(true, 16);
        let key_down = hub.descriptor_by_name("KeyDown").unwrap().id;

        // The plugin's state: its session handle and subscription.
        type Held = (Option<ForeignBus>, Option<gamma::ffi::ForeignSubscription>);
        let held: Arc<Mutex<Held>> = Arc::default();
        let keys = Arc::new(Mutex::new(Vec::new()));
        let (h, k) = (Arc::clone(&held), Arc::clone(&keys));
        let _session = subscribe_on(&plugin_view, move |event| match event {
            PieSessionEvent::Started(bus) => {
                let k = Arc::clone(&k);
                let sub = bus.subscribe_dyn(key_down, SubscribeOptions::default(), move |e| {
                    if let Some(DynValue::I64(key)) = e.fields.first() {
                        k.lock().unwrap().push(*key);
                    }
                });
                *h.lock().unwrap() = (Some(bus), Some(sub));
            }
            PieSessionEvent::Stopping => *h.lock().unwrap() = (None, None),
        });

        let game = unsafe { ForeignBus::from_raw(hub.export_raw()) }.unwrap();
        announce_started_on(&host, &game);
        assert!(held.lock().unwrap().0.is_some(), "attached");

        // The game publishes; the plugin sees it at the flush.
        hub.publish(Channel::Global, KeyDown { key: 3 });
        // The plugin publishes into the game.
        {
            let held = held.lock().unwrap();
            let bus = held.0.as_ref().unwrap();
            bus.publish_dyn_deferred(Channel::Global, &DynEvent::new(key_down, vec![DynValue::I64(4)])).unwrap();
        }
        hub.flush(FlushPoint::AfterInput);
        assert_eq!(*keys.lock().unwrap(), vec![3, 4]);
        assert!(hub.recent_events().iter().any(|r| r.summary.starts_with("(plugin)")));

        publish_on(&host, "stopping", 0);
        assert!(held.lock().unwrap().0.is_none(), "detached on stop");
        assert_eq!(hub.subscriber_count(key_down, Channel::Global), 0, "no plugin subscription left");
        drop(game);
    }
}
