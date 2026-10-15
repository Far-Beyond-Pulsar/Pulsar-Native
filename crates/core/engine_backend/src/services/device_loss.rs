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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Whether a watched device was lost. Clones share the state.
#[derive(Clone, Debug, Default)]
pub struct DeviceLoss {
    lost: Arc<AtomicBool>,
}

/// Every device watched in this process, with its watcher.
fn watched() -> MutexGuard<'static, Vec<(wgpu::Device, DeviceLoss)>> {
    static WATCHED: OnceLock<Mutex<Vec<(wgpu::Device, DeviceLoss)>>> = OnceLock::new();
    let mut watched = WATCHED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    // A lost device is not used again; let it go.
    watched.retain(|(_, loss)| !loss.is_lost());
    watched
}

impl DeviceLoss {
    /// Watches `device`, or returns the watcher already on it. Its lost
    /// callback marks the loss. Errors no error scope caught panic as
    /// wgpu's default handler does, except once the device is lost, when
    /// they are the expected echo of the loss and are only logged.
    pub fn watch(device: &wgpu::Device) -> Self {
        let mut watched = watched();
        if let Some((_, loss)) = watched.iter().find(|(watched, _)| watched == device) {
            return loss.clone();
        }

        let loss = DeviceLoss::default();
        let lost = Arc::clone(&loss.lost);
        device.set_device_lost_callback(move |reason, message| {
            lost.store(true, Ordering::Release);
            tracing::error!(?reason, %message, "GPU device lost");
        });
        let lost = Arc::clone(&loss.lost);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            if lost.load(Ordering::Acquire) {
                tracing::debug!("GPU error after the device was lost: {error}");
            } else {
                panic!("wgpu error: {error}");
            }
        }));
        watched.push((device.clone(), loss.clone()));
        loss
    }

    /// Watches `device` through `lost`, a flag its owner sets when it is
    /// lost; the owner already holds the device's lost and error callbacks.
    /// Later [`Self::watch`] calls on the device share this watcher.
    pub fn adopt(device: &wgpu::Device, lost: Arc<AtomicBool>) -> Self {
        let mut watched = watched();
        if let Some((_, loss)) = watched.iter().find(|(watched, _)| watched == device) {
            return loss.clone();
        }
        let loss = DeviceLoss { lost };
        watched.push((device.clone(), loss.clone()));
        loss
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }
}
