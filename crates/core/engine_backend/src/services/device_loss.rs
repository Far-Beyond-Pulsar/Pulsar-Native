//! Telling when a GPU device is lost (a driver reset, a removed GPU, a TDR).
//!
//! wgpu reports a lost device once, through the device's lost callback, and
//! from then on every call on it fails; errors no error scope catches go to
//! the device's uncaptured-error handler, whose default panics. A device has
//! one slot for each, and the editor's viewports and Play-in-Editor share
//! one device, so [`DeviceLoss::watch`] hands every caller watching the same
//! device the same watcher instead of replacing the callback.
//!
//! Devices are told apart by wgpu's own identity, which is unique within one
//! wgpu instance; the engine makes one per process (wgpui's in the editor,
//! the game's own when standalone).
//!
//! In the editor the device is wgpui's, and wgpui holds both of its slots
//! (it rebuilds its windows on a new device after a loss). The editor hands
//! wgpui's flag over with [`DeviceLoss::adopt`] before a renderer watches the
//! device, so watching it reads that flag instead of taking the slots.
//!
//! What a loss means is the owner's to act on: the standalone game makes a
//! new device and rebuilds its windows on it; renderers stop drawing on the
//! dead one. The scene's GPU copy follows the device a renderer is built on
//! (`scene::ensure_gpu_mirror`), so a rebuild uploads the world again from
//! the data SceneDB holds.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

/// Whether a watched device was lost. Clones share the state.
#[derive(Clone, Debug, Default)]
pub struct DeviceLoss {
    lost: Arc<AtomicBool>,
    /// Keeps the device's entry in the watched list (a [`Self::watch`]):
    /// when the last clone drops, the entry and its device handle go, so a
    /// device nothing renders on any more is freed (a dropped renderer's
    /// resources are only released by the device, and a device held here is
    /// never polled again). `None` for an adopted device, whose owner keeps
    /// it for the process.
    release: Option<Arc<Release>>,
}

/// Removes its entry from the watched list when dropped.
#[derive(Debug)]
struct Release {
    id: u64,
}

impl Drop for Release {
    fn drop(&mut self) {
        let id = self.id;
        watched().retain(|entry| entry.id != id);
    }
}

/// A watched device and its loss flag.
struct Entry {
    id: u64,
    device: wgpu::Device,
    lost: Arc<AtomicBool>,
    /// The watchers' release (a watched device); `None`: adopted.
    release: Option<Weak<Release>>,
}

impl Entry {
    /// A handle on this entry's watcher, while it has one.
    fn loss(&self) -> Option<DeviceLoss> {
        let release = match &self.release {
            Some(weak) => Some(weak.upgrade()?),
            None => None,
        };
        Some(DeviceLoss {
            lost: Arc::clone(&self.lost),
            release,
        })
    }
}

/// Every device watched in this process, with its watcher.
fn watched() -> MutexGuard<'static, Vec<Entry>> {
    static WATCHED: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    let mut watched = WATCHED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    // A lost device is not used again; let it go.
    watched.retain(|entry| !entry.lost.load(Ordering::Acquire));
    watched
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The devices currently watched (the renderers'), for read-only diagnostics
/// such as Mission Control's GPU memory report. Lost devices are not listed.
pub fn watched_devices() -> Vec<wgpu::Device> {
    watched().iter().map(|entry| entry.device.clone()).collect()
}

impl DeviceLoss {
    /// Watches `device`, or returns the watcher already on it. Its lost
    /// callback marks the loss. Errors no error scope caught panic as
    /// wgpu's default handler does, except once the device is lost, when
    /// they are the expected echo of the loss and are only logged. The
    /// device stays listed while a clone of the returned watcher lives.
    pub fn watch(device: &wgpu::Device) -> Self {
        let mut watched = watched();
        if let Some(loss) = watched
            .iter()
            .find(|entry| &entry.device == device)
            .and_then(Entry::loss)
        {
            return loss;
        }
        // An entry whose watchers all dropped is being released.
        watched.retain(|entry| &entry.device != device);

        let lost = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&lost);
        device.set_device_lost_callback(move |reason, message| {
            flag.store(true, Ordering::Release);
            tracing::error!(?reason, %message, "GPU device lost");
        });
        let flag = Arc::clone(&lost);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            if flag.load(Ordering::Acquire) {
                tracing::debug!("GPU error after the device was lost: {error}");
            } else {
                panic!("wgpu error: {error}");
            }
        }));
        let id = next_id();
        let release = Arc::new(Release { id });
        watched.push(Entry {
            id,
            device: device.clone(),
            lost: Arc::clone(&lost),
            release: Some(Arc::downgrade(&release)),
        });
        DeviceLoss {
            lost,
            release: Some(release),
        }
    }

    /// Watches `device` through `lost`, a flag its owner sets when it is
    /// lost; the owner already holds the device's lost and error callbacks
    /// (and the device, for the process). Later [`Self::watch`] calls on
    /// the device share this watcher.
    pub fn adopt(device: &wgpu::Device, lost: Arc<AtomicBool>) -> Self {
        let mut watched = watched();
        if let Some(loss) = watched
            .iter()
            .find(|entry| &entry.device == device)
            .and_then(Entry::loss)
        {
            return loss;
        }
        watched.retain(|entry| &entry.device != device);
        watched.push(Entry {
            id: next_id(),
            device: device.clone(),
            lost: Arc::clone(&lost),
            release: None,
        });
        DeviceLoss {
            lost,
            release: None,
        }
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }
}
