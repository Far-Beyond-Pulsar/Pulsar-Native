//! Per-frame dense-instance candidates for the ray-traced backend.
//!
//! The RT backend needs a CPU list of instances (transform, asset) to build a
//! TLAS, but the instance data lives on the GPU and the dense mask is only
//! known after the raster. [`RtCollect`] closes that gap without a CPU stall:
//!
//! 1. `collect` (compute, one thread per visible cluster, dispatched like
//!    `bin`) marks every instance that has a visible cluster from any raster
//!    tier overlapping a dense tile, deduplicated GPU-side through a per-instance bitmap, and
//!    appends one 64-byte record per distinct instance (transform, asset slot,
//!    error scale) to a bounded candidate buffer.
//! 2. The header + records (at most `capacity` of them) and the dense-tile
//!    count are copied into one of three staging buffers and mapped
//!    asynchronously; [`RtCollect::poll`] picks the result up a few frames
//!    later without ever waiting on the GPU.
//!
//! The CPU therefore reads O(dense instances) records (not O(all instances)),
//! one to three frames late. `uncovered` counts dense instances that are not in
//! the TLAS the GPU was told about (the `covered` bitmap), so the CPU learns
//! when its instance set is stale.

use crate::common::*;
use crate::config::{self, DenseConfig};
use crate::nonrt::DenseNonRtInputs;
use std::sync::mpsc;

/// Candidate record capacity of the readback.
// Dense tiles can contain many small foliage instances. Keep enough room for
// their deduplicated instance list so RT does not silently fall back at the
// old 2k-instance stress-scene ceiling.
pub const CAND_CAPACITY: u32 = 16_384;
const HEADER_WORDS: u32 = 4;
const STRIDE_WORDS: u32 = 16;
const SLOTS: usize = 3;

/// One dense instance as the GPU reported it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// Row of `vg_instances`.
    pub instance: u32,
    /// `GpuVgInstance::asset_slot` (a store slot, not a stable key).
    pub asset_slot: u32,
    pub lod_error_scale: f32,
    /// Row-major 3x4 object-to-world transform.
    pub transform: [f32; 12],
}

/// A decoded readback.
#[derive(Clone, Debug, Default)]
pub struct CollectResult {
    /// Distinct dense instances the GPU saw (may exceed `records.len()`).
    pub total: u32,
    /// Of those, instances not in the `covered` set at that frame.
    pub uncovered: u32,
    /// Dense tiles of that frame.
    pub dense_tiles: u32,
    /// Alpha-masked clusters overlapping a dense tile in that frame.
    pub masked_clusters: u32,
    pub records: Vec<Candidate>,
    /// Frame counter value when it was recorded.
    pub recorded_frame: u64,
}

impl CollectResult {
    pub fn overflowed(&self) -> bool {
        self.total as usize > self.records.len()
    }
}

enum SlotState {
    Free,
    /// Copy recorded this frame, not yet mapped.
    Recorded(u64),
    Mapping(
        u64,
        std::sync::Mutex<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
    ),
}

struct Staging {
    buffer: wgpu::Buffer,
    state: SlotState,
}

struct Grow {
    buffer: wgpu::Buffer,
    words: u32,
}

impl Grow {
    fn new(device: &wgpu::Device, label: &str, words: u32) -> Self {
        Self {
            buffer: Self::alloc(device, label, words),
            words,
        }
    }
    fn alloc(device: &wgpu::Device, label: &str, words: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: words.max(4) as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }
}

/// See the module docs.
pub struct RtCollect {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    capacity: u32,
    inst_bits: Grow,
    covered: Grow,
    cand: wgpu::Buffer,
    staging: Vec<Staging>,
    /// CPU mirror of `covered` (sorted instance ids it was built from).
    covered_ids: Vec<u32>,
    covered_cpu: Vec<u32>,
    frame: u64,
    covered_epoch: u64,
}

impl RtCollect {
    pub fn new(device: &wgpu::Device, cfg: &DenseConfig, capacity: u32) -> Self {
        let bgl = bgl(
            device,
            "vg_dense collect",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(2, storage(true)),
                bgl_entry(3, storage(true)),
                bgl_entry(5, storage(true)),
                bgl_entry(6, storage(true)),
                bgl_entry(8, storage(false)),
                bgl_entry(10, storage(true)),
                bgl_entry(11, storage(true)),
                bgl_entry(12, storage(false)),
                bgl_entry(13, storage(false)),
                bgl_entry(14, storage(true)),
            ],
        );
        let src = config::collect_source(cfg);
        let cand_words = HEADER_WORDS + capacity * STRIDE_WORDS;
        let staging_bytes = (cand_words as u64 + 1) * 4;
        Self {
            pipeline: pipeline(device, "vg_dense collect", &src, "collect", &bgl),
            bgl,
            capacity,
            inst_bits: Grow::new(device, "vg_dense inst_bits", 64),
            covered: Grow::new(device, "vg_dense covered", 64),
            cand: Grow::alloc(device, "vg_dense candidates", cand_words),
            staging: (0..SLOTS)
                .map(|_| Staging {
                    buffer: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("vg_dense candidate staging"),
                        size: staging_bytes,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    }),
                    state: SlotState::Free,
                })
                .collect(),
            covered_ids: Vec::new(),
            covered_cpu: Vec::new(),
            frame: 0,
            covered_epoch: 0,
        }
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Number of [`Self::record_rec`] calls so far (the `recorded_frame` clock).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Bumped every time [`Self::set_covered`] changes the covered set.
    pub fn covered_epoch(&self) -> u64 {
        self.covered_epoch
    }

    /// Size the bitmaps for `instance_rows` rows of `vg_instances` (grown in
    /// powers of two). A reallocated `covered` is refilled from the CPU mirror.
    fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instance_rows: u32) {
        let words = instance_rows.div_ceil(32).max(1).next_power_of_two();
        if words > self.inst_bits.words {
            self.inst_bits = Grow::new(device, "vg_dense inst_bits", words);
        }
        if words > self.covered.words {
            self.covered = Grow::new(device, "vg_dense covered", words);
            if self.covered_cpu.len() < words as usize {
                self.covered_cpu.resize(words as usize, 0);
            }
            queue.write_buffer(
                &self.covered.buffer,
                0,
                bytemuck::cast_slice(&self.covered_cpu[..words as usize]),
            );
        }
    }

    /// Tell the GPU which instances the TLAS currently holds. Touches only the
    /// bitmap words of the old and new members, so the cost is
    /// O(|old| + |new|), not O(instance rows); a no-op when unchanged.
    pub fn set_covered(&mut self, queue: &wgpu::Queue, ids: &[u32]) {
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted == self.covered_ids {
            return;
        }
        let need = sorted.last().map_or(0, |&m| m as usize / 32 + 1);
        if self.covered_cpu.len() < need {
            self.covered_cpu.resize(need.next_power_of_two(), 0);
        }
        let mut touched = Vec::with_capacity(self.covered_ids.len() + sorted.len());
        for &i in &self.covered_ids {
            self.covered_cpu[i as usize / 32] = 0;
            touched.push(i as usize / 32);
        }
        for &i in &sorted {
            self.covered_cpu[i as usize / 32] |= 1 << (i % 32);
            touched.push(i as usize / 32);
        }
        touched.sort_unstable();
        touched.dedup();
        // Words past the GPU buffer are written by `ensure` when it grows.
        let gpu_words = self.covered.words as usize;
        let mut i = 0;
        while i < touched.len() {
            let mut j = i;
            while j + 1 < touched.len() && touched[j + 1] == touched[j] + 1 {
                j += 1;
            }
            let (a, b) = (touched[i], touched[j]);
            if b < gpu_words {
                queue.write_buffer(
                    &self.covered.buffer,
                    a as u64 * 4,
                    bytemuck::cast_slice(&self.covered_cpu[a..=b]),
                );
            }
            i = j + 1;
        }
        self.covered_ids = sorted;
        self.covered_epoch += 1;
    }

    /// Record the collection (after the NonRT setup stages of the same frame)
    /// and the readback copy. `dense` is the NonRT buffer set whose
    /// `tile_slot`/`state`/`args` the setup just filled.
    #[allow(clippy::too_many_arguments)]
    pub fn record_rec(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rec: &mut helio_core::CommandRecorder<'_>,
        dense: &DenseBuffers,
        inputs: &DenseNonRtInputs<'_>,
        instance_rows: u32,
    ) {
        self.record_scoped(
            device,
            queue,
            rec,
            dense,
            inputs,
            instance_rows,
            &mut crate::common::GpuScopes::none(),
        );
    }

    /// [`Self::record_rec`] with GPU timestamp scopes around the collect kernel.
    #[allow(clippy::too_many_arguments)]
    pub fn record_scoped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rec: &mut helio_core::CommandRecorder<'_>,
        dense: &DenseBuffers,
        inputs: &DenseNonRtInputs<'_>,
        instance_rows: u32,
        scopes: &mut crate::common::GpuScopes<'_>,
    ) {
        self.frame += 1;
        self.ensure(device, queue, instance_rows);
        rec.clear_buffer(&self.inst_bits.buffer, 0, None);
        rec.clear_buffer(&self.cand, 0, Some(HEADER_WORDS as u64 * 4));
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense collect"),
            layout: &self.bgl,
            entries: &[
                bg_entry(0, &dense.params),
                bg_entry(2, inputs.visible),
                bg_entry(3, inputs.page_pool),
                bg_entry(5, inputs.vg_instances),
                bg_entry(6, &dense.tile_slot),
                bg_entry(8, &dense.state),
                bg_entry(10, inputs.objects),
                bg_entry(11, &dense.args),
                bg_entry(12, &self.inst_bits.buffer),
                bg_entry(13, &self.cand),
                bg_entry(14, &self.covered.buffer),
            ],
        });
        scopes.begin(rec, "vg_dense collect");
        {
            let mut pass = rec.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("vg_dense collect"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups_indirect(&dense.args, 12);
        }
        scopes.end(rec, "vg_dense collect");
        // Readback: header + records, then the dense tile count, into a free slot.
        if let Some(slot) = self
            .staging
            .iter_mut()
            .find(|s| matches!(s.state, SlotState::Free))
        {
            let cand_bytes = (HEADER_WORDS + self.capacity * STRIDE_WORDS) as u64 * 4;
            rec.copy_buffer_to_buffer(&self.cand, 0, &slot.buffer, 0, cand_bytes);
            rec.copy_buffer_to_buffer(&dense.state, 0, &slot.buffer, cand_bytes, 4);
            slot.state = SlotState::Recorded(self.frame);
        }
    }

    /// Start mapping the staging buffers whose copies were submitted with the
    /// previous frame. Call once per frame before [`Self::poll`].
    pub fn begin_maps(&mut self) {
        for slot in &mut self.staging {
            if let SlotState::Recorded(frame) = slot.state {
                let (tx, rx) = mpsc::channel();
                slot.buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| {
                        let _ = tx.send(r);
                    });
                slot.state = SlotState::Mapping(frame, std::sync::Mutex::new(rx));
            }
        }
    }

    /// The most recent finished readback, if any arrived since the last call.
    /// Never blocks (a non-waiting `device.poll` only).
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<CollectResult> {
        // The renderer polls the device once per frame (and the map callbacks fire there);
        // a second, explicit poll here measured 1.5 ms of CPU per call.
        let _ = device;
        let mut best: Option<CollectResult> = None;
        for slot in &mut self.staging {
            let SlotState::Mapping(frame, rx) = &slot.state else {
                continue;
            };
            let frame = *frame;
            let received = rx.lock().unwrap_or_else(|p| p.into_inner()).try_recv();
            match received {
                Ok(Ok(())) => {
                    let result = {
                        let view = slot
                            .buffer
                            .slice(..)
                            .get_mapped_range()
                            .expect("mapped range");
                        let words: &[u32] = bytemuck::cast_slice(&view);
                        parse(words, self.capacity, frame)
                    };
                    slot.buffer.unmap();
                    slot.state = SlotState::Free;
                    if best
                        .as_ref()
                        .is_none_or(|b| result.recorded_frame > b.recorded_frame)
                    {
                        best = Some(result);
                    }
                }
                Ok(Err(_)) => {
                    slot.state = SlotState::Free;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => slot.state = SlotState::Free,
            }
        }
        best
    }
}

fn parse(words: &[u32], capacity: u32, recorded_frame: u64) -> CollectResult {
    let total = words[0];
    let uncovered = words[1];
    let n = total.min(capacity) as usize;
    let mut records = Vec::with_capacity(n);
    for i in 0..n {
        let base = (HEADER_WORDS + i as u32 * STRIDE_WORDS) as usize;
        let w = &words[base..base + STRIDE_WORDS as usize];
        let mut transform = [0.0f32; 12];
        for (k, t) in transform.iter_mut().enumerate() {
            *t = f32::from_bits(w[4 + k]);
        }
        records.push(Candidate {
            instance: w[0],
            asset_slot: w[1],
            lod_error_scale: f32::from_bits(w[2]),
            transform,
        });
    }
    let dense_tiles = words[(HEADER_WORDS + capacity * STRIDE_WORDS) as usize];
    CollectResult {
        total,
        uncovered,
        dense_tiles,
        masked_clusters: words[2],
        records,
        recorded_frame,
    }
}
