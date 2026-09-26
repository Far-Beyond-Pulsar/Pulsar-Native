//! A bounded record of recently flushed hub events, for debugging (the
//! editor's Play-in-Editor events panel).
//!
//! Gamma has no monitor hook, so the tap lives at the hub level: every
//! event published through [`EventHub`](crate::EventHub)'s own publish
//! methods is noted while it waits in the queue, and moved into a ring
//! buffer when a flush delivers it, with the subscriber count of its
//! channel at that moment. Events a plugin publishes through the hub's
//! exported FFI table ([`EventHub::export_raw`](crate::EventHub::export_raw))
//! are noted the same way (summaries start with `(plugin)`); only
//! publishes made on [`EventHub::bus`](crate::EventHub::bus) directly are
//! not seen.
//!
//! The tap is off by default (the editor turns it on for PIE sessions);
//! when off, publishing costs one relaxed atomic load more.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gamma::{Channel, DynEvent, DynValue, EventDescriptor};

use crate::hub::FlushPoint;

/// A serialisable view of a hub's debug state, for tools on the other side
/// of a library boundary (the editor reading a Play-in-Editor game through
/// the PIE ABI's `pulsar_pie_events_snapshot`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EventsSnapshot {
    pub frame: u64,
    pub queued: usize,
    pub tap_enabled: bool,
    /// Recently flushed events, oldest first.
    pub recent: Vec<SnapshotRecord>,
    /// Every registered event with its global-channel subscriber count,
    /// sorted by name.
    pub events: Vec<SnapshotEvent>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotRecord {
    pub seq: u64,
    pub frame: u64,
    pub point: String,
    pub name: String,
    /// `global`, `entity:<hex bits>` or `class:<hex id>`.
    pub channel: String,
    pub summary: String,
    pub subscribers: usize,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotEvent {
    pub name: String,
    pub category: String,
    pub global_subscribers: usize,
}

/// `global`, `entity:<hex>` or `class:<hex>`.
pub fn channel_label(channel: Channel) -> String {
    match channel {
        Channel::Global => "global".into(),
        Channel::Entity(bits) => format!("entity:{bits:x}"),
        Channel::Class(id) => format!("class:{id:x}"),
    }
}

impl From<&TapRecord> for SnapshotRecord {
    fn from(r: &TapRecord) -> Self {
        Self {
            seq: r.seq,
            frame: r.frame,
            point: r.point.to_string(),
            name: r.name.clone(),
            channel: channel_label(r.channel),
            summary: r.summary.clone(),
            subscribers: r.subscribers,
        }
    }
}

/// One delivered event.
#[derive(Clone, Debug, PartialEq)]
pub struct TapRecord {
    /// Increasing per hub.
    pub seq: u64,
    /// The hub's frame counter when it was flushed.
    pub frame: u64,
    pub point: FlushPoint,
    pub name: String,
    pub channel: Channel,
    /// `field=value` pairs, shortened.
    pub summary: String,
    /// Subscribers on the event's channel when it was delivered.
    pub subscribers: usize,
}

pub(crate) struct Pending {
    pub id: u64,
    pub name: String,
    pub channel: Channel,
    pub summary: String,
}

pub(crate) struct Tap {
    enabled: AtomicBool,
    capacity: AtomicU64,
    seq: AtomicU64,
    pending: Mutex<Vec<Pending>>,
    ring: Mutex<VecDeque<TapRecord>>,
}

impl Tap {
    pub(crate) fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            capacity: AtomicU64::new(256),
            seq: AtomicU64::new(0),
            pending: Mutex::new(Vec::new()),
            ring: Mutex::new(VecDeque::new()),
        }
    }

    #[inline]
    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub(crate) fn set_enabled(&self, on: bool, capacity: usize) {
        self.capacity.store(capacity.max(1) as u64, Ordering::Relaxed);
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            self.clear();
        }
    }

    pub(crate) fn clear(&self) {
        lock(&self.pending).clear();
        lock(&self.ring).clear();
    }

    pub(crate) fn note(&self, pending: Pending) {
        lock(&self.pending).push(pending);
    }

    /// Move everything noted so far into the ring.
    pub(crate) fn delivered(&self, frame: u64, point: FlushPoint, count: impl Fn(u64, Channel) -> usize) {
        let pending = std::mem::take(&mut *lock(&self.pending));
        if pending.is_empty() {
            return;
        }
        let capacity = self.capacity.load(Ordering::Relaxed) as usize;
        let mut ring = lock(&self.ring);
        for p in pending {
            let subscribers = count(p.id, p.channel);
            ring.push_back(TapRecord {
                seq: self.seq.fetch_add(1, Ordering::Relaxed),
                frame,
                point,
                name: p.name,
                channel: p.channel,
                summary: p.summary,
                subscribers,
            });
            while ring.len() > capacity {
                ring.pop_front();
            }
        }
    }

    pub(crate) fn recent(&self) -> Vec<TapRecord> {
        lock(&self.ring).iter().cloned().collect()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// `a=1, b=entity:2a` for a payload, at most ~120 characters.
pub(crate) fn summarize(descriptor: Option<&EventDescriptor>, event: Option<&DynEvent>) -> String {
    let Some(event) = event else { return String::new() };
    let mut out = String::new();
    for (i, value) in event.fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        if let Some((name, _)) = descriptor.and_then(|d| d.fields.get(i)) {
            out.push_str(name);
            out.push('=');
        }
        match value {
            DynValue::Bool(b) => out.push_str(&b.to_string()),
            DynValue::I64(v) => out.push_str(&v.to_string()),
            DynValue::F64(v) => out.push_str(&format!("{v:.3}")),
            DynValue::U64(v) => out.push_str(&format!("#{v:x}")),
            DynValue::Str(s) => out.push_str(&format!("{s:?}")),
            DynValue::Bytes(b) => out.push_str(&format!("<{} bytes>", b.len())),
        }
        if out.len() > 120 {
            let mut cut = 117;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push_str("...");
            break;
        }
    }
    out
}
