//! Test plugin for `pulsar_events/tests/plugin_bus.rs` (and
//! `pulsar_game`'s script-event tests).
//!
//! Everything goes through its own copy of `pulsar_events` and Gamma:
//!
//! - the host bus, attached by the loader through `_plugin_attach_event_bus`
//!   exactly like `export_plugin!` does, carries `AssetUpdated` both ways;
//! - a session's engine hub, handed over as a `RawBus`
//!   (`EventHub::export_raw`), receives the plugin's `Hit`s (its own copy of
//!   the built-in type) and a plugin-declared dynamic event.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use pulsar_events::gamma::ffi::{ForeignBus, ForeignSubscription, RawBus};
use pulsar_events::gamma::{Channel, DynEvent, DynValue, EventDescriptor, FieldType};
use pulsar_events::{AssetKind, AssetSubscription, AssetUpdated};

pulsar_events::export_host_bus_attach!();

static ASSET_UPDATES_SEEN: AtomicU64 = AtomicU64::new(0);

struct State {
    hub: Option<ForeignBus>,
    asset_sub: Option<AssetSubscription>,
    _hub_subs: Vec<ForeignSubscription>,
}

static STATE: Mutex<State> = Mutex::new(State { hub: None, asset_sub: None, _hub_subs: Vec::new() });

fn scored() -> EventDescriptor {
    EventDescriptor::dynamic("Plugin.Scored", [("target", FieldType::U64), ("points", FieldType::I64)])
}

/// Subscribe to Blueprint asset updates on the (attached) host bus.
#[no_mangle]
pub extern "C" fn fixture_watch_assets() {
    let sub = pulsar_events::subscribe_asset_updates(Some(AssetKind::Blueprint), |_| {
        ASSET_UPDATES_SEEN.fetch_add(1, Ordering::SeqCst);
    });
    STATE.lock().unwrap().asset_sub = Some(sub);
}

#[no_mangle]
pub extern "C" fn fixture_asset_updates_seen() -> u64 {
    ASSET_UPDATES_SEEN.load(Ordering::SeqCst)
}

/// Publish a Blueprint update for class `plugin-class` on the host bus.
#[no_mangle]
pub extern "C" fn fixture_publish_asset() {
    pulsar_events::publish_asset_updated(AssetUpdated::new(AssetKind::Blueprint).with_id("plugin-class"));
}

/// 1 when the plugin's host bus is the host's (attached), 0 otherwise.
#[no_mangle]
pub extern "C" fn fixture_is_attached() -> u32 {
    pulsar_events::host::host_bus().is_foreign() as u32
}

/// Take a session hub. Returns 0 on success.
///
/// # Safety
/// `raw` must be a fresh `EventHub::export_raw()`.
#[no_mangle]
pub unsafe extern "C" fn fixture_attach_hub(raw: RawBus) -> u32 {
    let Ok(bus) = ForeignBus::from_raw(raw) else { return 1 };
    if bus.register_descriptor(&scored()).is_err() {
        return 2;
    }
    STATE.lock().unwrap().hub = Some(bus);
    0
}

/// Queue a `Hit` for `entity` (on its entity channel) on the hub.
#[no_mangle]
pub extern "C" fn fixture_publish_hit(entity: u64, other: u64, impulse: f64) -> u32 {
    let state = STATE.lock().unwrap();
    let Some(hub) = &state.hub else { return 1 };
    let hit = pulsar_events::builtin::Hit { entity, other, impulse };
    hub.publish_deferred_to(Channel::Entity(entity), hit).map_or(2, |()| 0)
}

/// Queue `Plugin.Scored(target, points)` on the global channel.
#[no_mangle]
pub extern "C" fn fixture_publish_scored(target: u64, points: i64) -> u32 {
    let state = STATE.lock().unwrap();
    let Some(hub) = &state.hub else { return 1 };
    let event = DynEvent::new(scored().id, vec![DynValue::U64(target), DynValue::I64(points)]);
    hub.publish_dyn_deferred(Channel::Global, &event).map_or(2, |()| 0)
}

/// Queue an event whose fields do not match its descriptor: refused by
/// the host (returns non-zero), never delivered.
#[no_mangle]
pub extern "C" fn fixture_publish_bad() -> u32 {
    let state = STATE.lock().unwrap();
    let Some(hub) = &state.hub else { return 1 };
    let event = DynEvent::new(scored().id, vec![DynValue::Str("nope".into())]);
    hub.publish_dyn_deferred(Channel::Global, &event).map_or(2, |()| 0)
}

/// Drop everything this plugin holds on host buses.
#[no_mangle]
pub extern "C" fn fixture_shutdown() {
    let mut state = STATE.lock().unwrap();
    state.asset_sub = None;
    state._hub_subs.clear();
    state.hub = None;
}
