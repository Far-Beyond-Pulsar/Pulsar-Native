//! Non-blocking readback of the Tier-3 counters (`DenseStats`), the "overflow
//! counter surfaced in the debug UI" of plan section 5.4: dropped cluster-list
//! entries, dropped tiles, clamped peels. Three staging buffers, copies
//! recorded each frame the pass runs, results picked up a few frames later.

use crate::common::{DenseBuffers, DenseStats, STATE_HEADER_WORDS};
use std::sync::mpsc;
use std::sync::Mutex;

const SLOTS: usize = 3;
const BYTES: u64 = STATE_HEADER_WORDS as u64 * 4;

enum Slot {
    Free,
    Recorded,
    Mapping(Mutex<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>),
}

pub struct StatsReadback {
    staging: Vec<(wgpu::Buffer, Slot)>,
}

impl StatsReadback {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            staging: (0..SLOTS)
                .map(|_| {
                    (
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("vg_dense stats staging (async)"),
                            size: BYTES,
                            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                            mapped_at_creation: false,
                        }),
                        Slot::Free,
                    )
                })
                .collect(),
        }
    }

    /// Copy `buffers`' counter header into a free staging slot.
    pub fn record(&mut self, rec: &mut helio_core::CommandRecorder<'_>, buffers: &DenseBuffers) {
        if let Some((buffer, slot)) = self
            .staging
            .iter_mut()
            .find(|(_, s)| matches!(s, Slot::Free))
        {
            rec.copy_buffer_to_buffer(&buffers.state, 0, buffer, 0, BYTES);
            *slot = Slot::Recorded;
        }
    }

    /// Start mapping copies submitted with the previous frame.
    pub fn begin_maps(&mut self) {
        for (buffer, slot) in &mut self.staging {
            if matches!(slot, Slot::Recorded) {
                let (tx, rx) = mpsc::channel();
                buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
                *slot = Slot::Mapping(Mutex::new(rx));
            }
        }
    }

    /// The newest finished readback, if any (never waits).
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<DenseStats> {
        // The renderer polls the device once per frame (and the map callbacks fire there);
        // a second, explicit poll here measured 1.5 ms of CPU per call.
        let _ = device;
        let mut best = None;
        for (buffer, slot) in &mut self.staging {
            let Slot::Mapping(rx) = slot else { continue };
            let received = rx.lock().unwrap_or_else(|p| p.into_inner()).try_recv();
            match received {
                Ok(Ok(())) => {
                    let stats = {
                        let view = buffer.slice(..).get_mapped_range().expect("mapped range");
                        let words: &[u32] = bytemuck::cast_slice(&view);
                        DenseStats::from_words(words)
                    };
                    buffer.unmap();
                    *slot = Slot::Free;
                    best = Some(stats);
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => *slot = Slot::Free,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        best
    }
}
