//! A tapped export of a hub's bus (Pulsar-Native#942).
//!
//! [`EventHub::export_raw`](crate::EventHub::export_raw) hands plugins a
//! Gamma [`RawBus`] whose functions forward to the hub's real bus, except
//! that `publish` and `publish_deferred` first note the event in the hub's
//! debug tap (when it is on). So events an editor plugin publishes through
//! a `ForeignBus` show in the Play-in-Editor events panel like the hub's
//! own. The wrapper owns one strong reference to the real export and
//! releases it when the last reference to the wrapper is released.

use std::ffi::c_void;
use std::sync::{Arc, Weak};

use gamma::ffi::{RawBus, RawDropFn, RawEventRef, RawHandler, RawChannel, EVENT_DYN, STATUS_OK};
use gamma::DynEvent;

use crate::hub::Inner;
use crate::tap::{summarize, Pending};

struct TapCtx {
    inner: RawBus,
    hub: Weak<Inner>,
}

impl Drop for TapCtx {
    fn drop(&mut self) {
        // SAFETY: we own exactly one strong reference to the real export.
        unsafe { (self.inner.release)(self.inner.ctx) }
    }
}

/// Wrap `inner` (an export of `hub`'s bus, whose reference the wrapper
/// takes over) so publishes through it are noted in `hub`'s tap.
pub(crate) fn export_tapped(inner: RawBus, hub: Weak<Inner>) -> RawBus {
    let ctx = Arc::into_raw(Arc::new(TapCtx { inner, hub })) as *const c_void;
    RawBus {
        abi_version: inner.abi_version,
        struct_size: inner.struct_size,
        flags: inner.flags,
        ctx,
        retain: tap_retain,
        release: tap_release,
        downgrade: tap_downgrade,
        // Weak references are the real bus's: these take no ctx.
        weak_release: inner.weak_release,
        subscribe: tap_subscribe,
        unsubscribe_weak: inner.unsubscribe_weak,
        publish: tap_publish,
        publish_deferred: tap_publish_deferred,
        register_descriptor: tap_register_descriptor,
    }
}

/// # Safety
/// `ctx` is a live `TapCtx` (the caller holds a strong reference).
unsafe fn tap_ctx<'a>(ctx: *const c_void) -> &'a TapCtx {
    // SAFETY: see the function contract.
    unsafe { &*(ctx as *const TapCtx) }
}

unsafe extern "C" fn tap_retain(ctx: *const c_void) {
    // SAFETY: `ctx` came from `Arc::into_raw` and is alive.
    unsafe { Arc::increment_strong_count(ctx as *const TapCtx) }
}

unsafe extern "C" fn tap_release(ctx: *const c_void) {
    // SAFETY: releases one strong reference the caller owned.
    unsafe { drop(Arc::from_raw(ctx as *const TapCtx)) }
}

unsafe extern "C" fn tap_downgrade(ctx: *const c_void) -> *const c_void {
    // SAFETY: live ctx; forwarded.
    let tap = unsafe { tap_ctx(ctx) };
    unsafe { (tap.inner.downgrade)(tap.inner.ctx) }
}

unsafe extern "C" fn tap_subscribe(
    ctx: *const c_void,
    id: u64,
    channel: RawChannel,
    priority: i32,
    handler: RawHandler,
) -> u64 {
    // SAFETY: live ctx; forwarded.
    let tap = unsafe { tap_ctx(ctx) };
    unsafe { (tap.inner.subscribe)(tap.inner.ctx, id, channel, priority, handler) }
}

unsafe extern "C" fn tap_publish(ctx: *const c_void, event: *const RawEventRef) -> i32 {
    // SAFETY: live ctx; `event` is valid for the call.
    let tap = unsafe { tap_ctx(ctx) };
    let status = unsafe { (tap.inner.publish)(tap.inner.ctx, event) };
    if status == STATUS_OK {
        // SAFETY: valid for the call.
        unsafe { note(tap, event, "plugin, immediate") };
    }
    status
}

unsafe extern "C" fn tap_publish_deferred(
    ctx: *const c_void,
    event: *const RawEventRef,
    drop_fn: Option<RawDropFn>,
) -> i32 {
    // SAFETY: live ctx; `event` is valid for the call. Noted before the
    // forward: a typed event's value is moved into the bus by it.
    let tap = unsafe { tap_ctx(ctx) };
    let noted = unsafe { pending(tap, event, "plugin") };
    let status = unsafe { (tap.inner.publish_deferred)(tap.inner.ctx, event, drop_fn) };
    if status == STATUS_OK {
        if let (Some(pending), Some(hub)) = (noted, tap.hub.upgrade()) {
            hub.tap.note(pending);
        }
    }
    status
}

unsafe extern "C" fn tap_register_descriptor(ctx: *const c_void, data: *const u8, len: usize) -> i32 {
    // SAFETY: live ctx; forwarded.
    let tap = unsafe { tap_ctx(ctx) };
    unsafe { (tap.inner.register_descriptor)(tap.inner.ctx, data, len) }
}

/// # Safety
/// `event` is valid for the call.
unsafe fn note(tap: &TapCtx, event: *const RawEventRef, origin: &str) {
    // SAFETY: forwarded.
    if let (Some(pending), Some(hub)) = (unsafe { pending(tap, event, origin) }, tap.hub.upgrade()) {
        hub.tap.note(pending);
    }
}

/// What the tap records for `event`, when the tap is on.
///
/// # Safety
/// `event` is valid for the call.
unsafe fn pending(tap: &TapCtx, event: *const RawEventRef, origin: &str) -> Option<Pending> {
    let hub = tap.hub.upgrade()?;
    if !hub.tap.enabled() || event.is_null() {
        return None;
    }
    // SAFETY: valid for the call (caller).
    let event = unsafe { &*event };
    let channel = event.channel.to_channel()?;
    let descriptor = hub.bus.descriptor(event.id);
    let name = descriptor.as_ref().map_or_else(|| format!("#{:016x}", event.id), |d| d.name.clone());
    let decoded = (event.kind == EVENT_DYN && !event.data.is_null())
        // SAFETY: an EVENT_DYN payload is `len` encoded bytes.
        .then(|| DynEvent::decode(unsafe { std::slice::from_raw_parts(event.data, event.len) }).ok())
        .flatten();
    let mut summary = summarize(descriptor.as_deref(), decoded.as_ref());
    summary = if summary.is_empty() { format!("({origin})") } else { format!("({origin}) {summary}") };
    Some(Pending { id: event.id, name, channel, summary })
}
