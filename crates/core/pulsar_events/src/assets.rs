//! The asset-update bus.
//!
//! - [`publish_asset_updated`] delivers an event synchronously, on the
//!   publishing thread, to every subscriber of its kind (and to
//!   subscribers of all kinds). Subscribers must be quick and must not
//!   assume a particular thread; one that has thread-affine work to do
//!   (e.g. the Play-In-Editor game, which runs on the render thread)
//!   queues the event and handles it there.
//! - [`subscribe_asset_updates`] returns an [`AssetSubscription`]; dropping
//!   it unsubscribes.
//!
//! The bus is the process-wide [host bus](crate::host) (Gamma v2). A
//! plugin compiled as a separate dynamic library that was attached to the
//! host's bus when it loaded publishes and receives the same events as the
//! editor. On the bus an update is the dynamic event `AssetUpdated`
//! ([`descriptor`]) with three string fields: `kind` (the [`AssetKind`] as
//! JSON), `id` and `path` (empty when not given).
//!
//! The Play-In-Editor game dylib is not a plugin: it keeps its own bus and
//! the host forwards events to it explicitly.

use std::path::PathBuf;

use gamma::{Channel, DynEvent, DynValue, EventDescriptor, FieldType, SubscribeOptions};
use serde::{Deserialize, Serialize};
use ui_types_common::AssetKind;

use crate::host::{HostBus, HostSubscription, host_bus};

/// An asset was rewritten on disk.
///
/// Identify the asset by `id` when it has a stable identity (a class GUID,
/// a content id), by `path` otherwise; set both when known. Consumers match
/// on whichever they index by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetUpdated {
    /// What kind of asset changed. Class assets (Blueprint class
    /// directories) use [`AssetKind::Blueprint`].
    pub kind: AssetKind,
    /// Stable asset id, if the asset has one (e.g. a class GUID).
    #[serde(default)]
    pub id: Option<String>,
    /// Asset location on disk (a file, or a directory for class assets).
    #[serde(default)]
    pub path: Option<PathBuf>,
}

impl AssetUpdated {
    pub fn new(kind: AssetKind) -> Self {
        Self {
            kind,
            id: None,
            path: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
}

/// The `AssetUpdated` descriptor on the host bus.
pub fn descriptor() -> EventDescriptor {
    EventDescriptor::dynamic(
        "AssetUpdated",
        [("kind", FieldType::Str), ("id", FieldType::Str), ("path", FieldType::Str)],
    )
}

fn registered(bus: &HostBus) -> Option<u64> {
    let descriptor = descriptor();
    let id = descriptor.id;
    match bus.register_descriptor(&descriptor) {
        Ok(()) => Some(id),
        Err(error) => {
            tracing::error!("asset bus: cannot register AssetUpdated: {error}");
            None
        }
    }
}

fn to_dyn(event: &AssetUpdated, id: u64) -> DynEvent {
    let kind = serde_json::to_string(&event.kind).unwrap_or_default();
    let path = event.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    DynEvent::new(
        id,
        vec![
            DynValue::Str(kind),
            DynValue::Str(event.id.clone().unwrap_or_default()),
            DynValue::Str(path),
        ],
    )
}

fn from_dyn(event: &DynEvent) -> Option<AssetUpdated> {
    let [DynValue::Str(kind), DynValue::Str(id), DynValue::Str(path)] = event.fields.as_slice() else {
        return None;
    };
    Some(AssetUpdated {
        kind: serde_json::from_str(kind).ok()?,
        id: (!id.is_empty()).then(|| id.clone()),
        path: (!path.is_empty()).then(|| PathBuf::from(path)),
    })
}

/// Deliver `event` to every subscriber of its kind on the host bus.
/// Subscribers run after the bus lock is released, so they may subscribe,
/// unsubscribe or publish.
pub fn publish_asset_updated(event: AssetUpdated) {
    publish_asset_updated_on(host_bus(), &event);
}

/// [`publish_asset_updated`] on an explicit bus.
pub fn publish_asset_updated_on(bus: &HostBus, event: &AssetUpdated) {
    let Some(id) = registered(bus) else { return };
    tracing::debug!(kind = ?event.kind, id = ?event.id, path = ?event.path, "asset updated");
    if let Err(error) = bus.publish_dyn(Channel::Global, &to_dyn(event, id)) {
        tracing::error!("asset bus: publish failed: {error}");
    }
}

/// Call `callback` for every published [`AssetUpdated`] of `kind` (every
/// kind when `None`) until the returned subscription is dropped.
pub fn subscribe_asset_updates(
    kind: Option<AssetKind>,
    callback: impl Fn(&AssetUpdated) + Send + Sync + 'static,
) -> AssetSubscription {
    subscribe_asset_updates_on(host_bus(), kind, callback)
}

/// [`subscribe_asset_updates`] on an explicit bus.
pub fn subscribe_asset_updates_on(
    bus: &HostBus,
    kind: Option<AssetKind>,
    callback: impl Fn(&AssetUpdated) + Send + Sync + 'static,
) -> AssetSubscription {
    let id = registered(bus).unwrap_or_else(|| descriptor().id);
    let inner = bus.subscribe_dyn(id, SubscribeOptions::default(), move |event| {
        let Some(update) = from_dyn(event) else {
            tracing::warn!("asset bus: malformed AssetUpdated ignored");
            return;
        };
        if kind.as_ref().is_none_or(|k| *k == update.kind) {
            callback(&update);
        }
    });
    AssetSubscription { _inner: inner }
}

/// A live subscription; unsubscribes on drop.
#[must_use = "dropping the subscription unsubscribes immediately"]
#[derive(Debug)]
pub struct AssetSubscription {
    _inner: HostSubscription,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn subscribers_get_their_kind_until_dropped() {
        let meshes = Arc::new(AtomicUsize::new(0));
        let all = Arc::new(AtomicUsize::new(0));
        let m = Arc::clone(&meshes);
        let sub_mesh = subscribe_asset_updates(Some(AssetKind::Mesh), move |e| {
            assert_eq!(e.kind, AssetKind::Mesh);
            m.fetch_add(1, Ordering::SeqCst);
        });
        let a = Arc::clone(&all);
        let sub_all = subscribe_asset_updates(None, move |_| {
            a.fetch_add(1, Ordering::SeqCst);
        });

        publish_asset_updated(AssetUpdated::new(AssetKind::Mesh).with_path("/x.mesh"));
        publish_asset_updated(AssetUpdated::new(AssetKind::Blueprint).with_id("guid"));
        assert_eq!(meshes.load(Ordering::SeqCst), 1);
        assert_eq!(all.load(Ordering::SeqCst), 2);

        drop(sub_mesh);
        publish_asset_updated(AssetUpdated::new(AssetKind::Mesh));
        assert_eq!(meshes.load(Ordering::SeqCst), 1, "unsubscribed");
        assert_eq!(all.load(Ordering::SeqCst), 3);
        drop(sub_all);
    }

    #[test]
    fn a_foreign_view_shares_the_host_bus() {
        // In-process stand-in for a plugin: the same FFI table a plugin gets.
        let host = HostBus::local();
        let plugin = unsafe { HostBus::foreign(host.export_raw().unwrap()) }.unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let s = Arc::clone(&seen);
        let _sub = subscribe_asset_updates_on(&host, Some(AssetKind::Blueprint), move |e| {
            assert_eq!(e.id.as_deref(), Some("guid"));
            assert_eq!(e.path, None);
            s.fetch_add(1, Ordering::SeqCst);
        });
        publish_asset_updated_on(&plugin, &AssetUpdated::new(AssetKind::Blueprint).with_id("guid"));
        assert_eq!(seen.load(Ordering::SeqCst), 1);

        let back = Arc::new(AtomicUsize::new(0));
        let b = Arc::clone(&back);
        let _plugin_sub = subscribe_asset_updates_on(&plugin, None, move |e| {
            assert_eq!(e.path.as_deref(), Some(std::path::Path::new("/m.mesh")));
            b.fetch_add(1, Ordering::SeqCst);
        });
        publish_asset_updated_on(&host, &AssetUpdated::new(AssetKind::Mesh).with_path("/m.mesh"));
        assert_eq!(back.load(Ordering::SeqCst), 1);
    }
}
