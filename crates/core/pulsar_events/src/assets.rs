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
//! The bus is process-global. Code compiled into a separate dynamic library
//! with its own copy of this crate (the Play-In-Editor game dylib) has its
//! own bus; the host forwards events to it explicitly.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use ui_types_common::AssetKind;

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

type Callback = Arc<dyn Fn(&AssetUpdated) + Send + Sync>;

struct Subscriber {
    id: u64,
    /// `None` = every kind.
    kind: Option<AssetKind>,
    callback: Callback,
}

fn subscribers() -> &'static Mutex<Vec<Subscriber>> {
    static SUBSCRIBERS: OnceLock<Mutex<Vec<Subscriber>>> = OnceLock::new();
    SUBSCRIBERS.get_or_init(|| Mutex::new(Vec::new()))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Deliver `event` to every subscriber of its kind. Subscribers run after
/// the bus lock is released, so they may subscribe, unsubscribe or publish.
pub fn publish_asset_updated(event: AssetUpdated) {
    let targets: Vec<Callback> = subscribers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter(|s| s.kind.as_ref().is_none_or(|k| *k == event.kind))
        .map(|s| Arc::clone(&s.callback))
        .collect();
    tracing::debug!(kind = ?event.kind, id = ?event.id, path = ?event.path, subscribers = targets.len(), "asset updated");
    for callback in targets {
        callback(&event);
    }
}

/// Call `callback` for every published [`AssetUpdated`] of `kind` (every
/// kind when `None`) until the returned subscription is dropped.
pub fn subscribe_asset_updates(
    kind: Option<AssetKind>,
    callback: impl Fn(&AssetUpdated) + Send + Sync + 'static,
) -> AssetSubscription {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    subscribers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(Subscriber {
            id,
            kind,
            callback: Arc::new(callback),
        });
    AssetSubscription { id }
}

/// A live subscription; unsubscribes on drop.
#[must_use = "dropping the subscription unsubscribes immediately"]
#[derive(Debug)]
pub struct AssetSubscription {
    id: u64,
}

impl Drop for AssetSubscription {
    fn drop(&mut self) {
        subscribers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|s| s.id != self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

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
}
