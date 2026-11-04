//! Small, bounded, async CPU readback of the GPU-computed range tables and
//! draw/shadow counts.
//!
//! Draw passes pick a pipeline per `(material_class, graph_hash)`, so the
//! CPU has to know which material keys exist. This table tells
//! `helio-pass-occlusion-cull` which keys to give a draw segment (see
//! `helio_pass_gbuffer::DrawSegments`); the draws themselves use the GPU's
//! current tables, so the range `start`/`count` read back here are never used
//! as draw offsets. Every OTHER byte this pipeline produces
//! (`instances`/`aabbs`/`draw_calls`/the shadow-partitioned indirect lists)
//! is read by later passes entirely on the GPU with zero CPU involvement --
//! only this one small, already-bounded table (`#ranges <= #groups <=
//! #objects`, and a real scene's distinct material combinations stay far
//! below even that) and the counts reach the CPU, through an async,
//! non-blocking `map_async`, read whenever its callback has actually fired
//! (never stalling the frame to wait for it). The copy made in frame N's
//! `prepare()` holds frame N-1's results and is harvested in frame N+1's, so
//! the CPU's view trails the GPU by two frames or more.
//!
//! The counts don't size the main view's culling: the frustum and occlusion
//! culls dispatch one workgroup per live group from a size the GPU writes the
//! same frame (`ObjectBatchFrameData::group_dispatch`), and draws read the GPU
//! count; the shadow and portal passes read the GPU counts too. The read-back
//! counts remain for what has no GPU count or is a CPU decision: Hi-Z warm-up,
//! the shadow pass's static-layer cache invalidation
//! (`shadow_static_generation`), and draws on devices without
//! `MULTI_DRAW_INDIRECT_COUNT`.
//!
//! This is emphatically NOT a readback of per-instance data: `instances`/
//! `aabbs` (the only per-object-sized buffers) never touch the CPU.
//!
//! The staging copies are sized to the number of ranges actually observed
//! (one per distinct `(material_class, graph_hash)` pipeline), not to the
//! object capacity, so the per-frame copy and map cost does not grow with
//! scene size. A harvested generation whose range count exceeds its staging
//! capacity is discarded (the previous tables stay current) and the staging
//! grows before the next copy; a truncated table is never published.
//!
//! # Ordering (why this is correct, not racy)
//!
//! [`RangeReadback::poll_and_kick_off`] is called from `ObjectBatchPass::
//! prepare()`, which the render graph runs BEFORE this same pass's
//! `execute()` each frame (the same ordering every other pass's `prepare()`-
//! side uniform upload already relies on). So at the moment this runs for
//! frame N, `scratch`'s buffers still hold whatever frame (N-1)'s
//! `execute()` last wrote -- copying them now is copying valid, complete
//! data, and it is submitted (hence GPU-ordered) strictly before frame N's
//! own `execute()` dispatch overwrites them with fresh values.

use std::sync::{Arc, Mutex};

use super::{RangeTuple, ScratchBuffers};

type MapResult = Result<(), wgpu::BufferAsyncError>;
/// `map_async`'s callback can fire from a driver/polling thread, so its
/// slot needs `Send + Sync` -- an `mpsc::Receiver` isn't `Sync` (and
/// `ObjectBatchPass` must be, to be stored as a graph pass), so each
/// pending map is a shared slot the callback fills and `try_recv`-style
/// polling below drains with a non-blocking `lock()` + `take()`.
type MapSlot = Arc<Mutex<Option<MapResult>>>;

fn new_map_slot() -> MapSlot {
    Arc::new(Mutex::new(None))
}

fn try_recv(slot: &MapSlot) -> Option<MapResult> {
    slot.lock().expect("map slot mutex poisoned").take()
}

/// One staging "generation": a copy of last-known counts + range tables,
/// with up to four independent `map_async` calls in flight (or already
/// resolved) against it.
struct Generation {
    counts_staging: wgpu::Buffer,
    opaque_staging: wgpu::Buffer,
    transparent_staging: wgpu::Buffer,
    forward_staging: wgpu::Buffer,
    pending: Option<PendingMaps>,
}

struct PendingMaps {
    counts_slot: MapSlot,
    opaque_slot: MapSlot,
    transparent_slot: MapSlot,
    forward_slot: MapSlot,
    counts_ready: bool,
    opaque_ready: bool,
    transparent_ready: bool,
    forward_ready: bool,
}

impl PendingMaps {
    fn all_ready(&self) -> bool {
        self.counts_ready && self.opaque_ready && self.transparent_ready && self.forward_ready
    }
}

impl Generation {
    fn new(device: &wgpu::Device, range_capacity: u32) -> Self {
        let range_bytes = (range_capacity.max(1) as u64) * super::RANGE_BYTES;
        let make = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            // [group_count, bucket_opaque, bucket_transparent, bucket_forward, shadow_static, shadow_movable, instance_count, shadow_transmissive] -- 8 u32s.
            counts_staging: make("ObjBatch Counts Staging", 10 * 4),
            opaque_staging: make("ObjBatch Opaque Staging", range_bytes),
            transparent_staging: make("ObjBatch Transparent Staging", range_bytes),
            forward_staging: make("ObjBatch Forward Staging", range_bytes),
            pending: None,
        }
    }

    fn range_capacity_bytes(&self) -> u64 {
        self.opaque_staging.size()
    }

    fn range_capacity(&self) -> u32 {
        (self.range_capacity_bytes() / super::RANGE_BYTES) as u32
    }
}

const GENERATIONS: usize = 2;
/// Initial per-table staging size in ranges. Real scenes have a few distinct
/// pipelines; staging grows to the next power of two when exceeded.
const MIN_RANGE_CAPACITY: u32 = 64;

pub struct RangeReadback {
    gens: Vec<Generation>,
    next: usize,
    /// Ranges per table each staging generation can hold.
    range_capacity: u32,

    opaque: Vec<RangeTuple>,
    transparent: Vec<RangeTuple>,
    forward: Vec<RangeTuple>,
    draw_count: u32,
    shadow_static: u32,
    shadow_movable: u32,
    shadow_transmissive: u32,
    shadow_membership_fingerprint: (u32, u32),
    instance_count: u32,
    /// Bumped whenever a freshly-harvested generation's static caster
    /// membership fingerprint changes. Counts alone are insufficient: a
    /// same-frame static/movable swap can preserve counts while changing the
    /// cached shadow atlas contents.
    shadow_static_generation: u64,
}

impl RangeReadback {
    pub fn new() -> Self {
        Self {
            gens: Vec::new(),
            next: 0,
            range_capacity: MIN_RANGE_CAPACITY,
            opaque: Vec::new(),
            transparent: Vec::new(),
            forward: Vec::new(),
            draw_count: 0,
            shadow_static: 0,
            shadow_movable: 0,
            shadow_transmissive: 0,
            shadow_membership_fingerprint: (0, 0),
            instance_count: 0,
            shadow_static_generation: 0,
        }
    }

    pub fn opaque(&self) -> &[RangeTuple] {
        &self.opaque
    }
    pub fn transparent(&self) -> &[RangeTuple] {
        &self.transparent
    }
    pub fn forward(&self) -> &[RangeTuple] {
        &self.forward
    }
    pub fn counts(&self) -> (u32, u32, u32) {
        (self.draw_count, self.shadow_static, self.shadow_movable)
    }
    /// Static transparent-only shadow casters (the transmittance layer).
    pub fn shadow_transmissive(&self) -> u32 {
        self.shadow_transmissive
    }
    /// Live instance count -- same value as `instances_buffer()`'s valid
    /// prefix length.
    pub fn instance_count(&self) -> u32 {
        self.instance_count
    }
    /// See [`Self::shadow_static_generation`]'s field doc.
    pub fn shadow_static_generation(&self) -> u64 {
        self.shadow_static_generation
    }

    /// See this module's doc for the full ordering rationale. Never blocks.
    /// Returns whether a copy was kicked off this call.
    pub fn poll_and_kick_off(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scratch: &ScratchBuffers,
        kick_off_copy: bool,
        poll_device: bool,
    ) -> bool {
        // A shared device is maintained by every `Queue::submit` (see
        // `PrepareContext::owns_device`), which delivers the map callbacks.
        if poll_device {
            device.poll(wgpu::PollType::Poll).ok();
        }

        if self.gens.len() < GENERATIONS
            || self
                .gens
                .iter()
                .any(|gen| gen.range_capacity() != self.range_capacity)
        {
            // (Re)build generations sized to the current range capacity.
            // Any maps in flight against the old (about to be dropped)
            // buffers simply never get read -- harmless: the public arrays
            // just keep whatever they last held until a new generation's
            // map resolves.
            self.gens = (0..GENERATIONS)
                .map(|_| Generation::new(device, self.range_capacity))
                .collect();
            self.next = 0;
        }

        // 1. Advance every generation's pending maps; harvest the first one
        // that's fully resolved this call.
        for gen in self.gens.iter_mut() {
            let Some(pending) = gen.pending.as_mut() else {
                continue;
            };
            if !pending.counts_ready {
                if let Some(r) = try_recv(&pending.counts_slot) {
                    pending.counts_ready = r.is_ok();
                    if r.is_err() {
                        gen.pending = None;
                        continue;
                    }
                }
            }
            let Some(pending) = gen.pending.as_mut() else {
                continue;
            };
            if !pending.opaque_ready {
                if let Some(r) = try_recv(&pending.opaque_slot) {
                    pending.opaque_ready = r.is_ok();
                }
            }
            if !pending.transparent_ready {
                if let Some(r) = try_recv(&pending.transparent_slot) {
                    pending.transparent_ready = r.is_ok();
                }
            }
            if !pending.forward_ready {
                if let Some(r) = try_recv(&pending.forward_slot) {
                    pending.forward_ready = r.is_ok();
                }
            }
            if pending.all_ready() {
                let needed = required_range_capacity(gen);
                if needed > gen.range_capacity() {
                    // Truncated tables: keep the previous ones and grow the
                    // staging before the next copy.
                    self.range_capacity = self
                        .range_capacity
                        .max(needed.checked_next_power_of_two().unwrap_or(needed));
                    gen.counts_staging.unmap();
                    gen.opaque_staging.unmap();
                    gen.transparent_staging.unmap();
                    gen.forward_staging.unmap();
                    gen.pending = None;
                    continue;
                }
                let prev_shadow_membership = self.shadow_membership_fingerprint;
                read_generation_into(
                    gen,
                    &mut self.opaque,
                    &mut self.transparent,
                    &mut self.forward,
                    &mut self.draw_count,
                    &mut self.shadow_static,
                    &mut self.shadow_movable,
                    &mut self.shadow_transmissive,
                    &mut self.shadow_membership_fingerprint,
                    &mut self.instance_count,
                );
                if self.shadow_membership_fingerprint != prev_shadow_membership {
                    self.shadow_static_generation = self.shadow_static_generation.wrapping_add(1);
                }
                gen.counts_staging.unmap();
                gen.opaque_staging.unmap();
                gen.transparent_staging.unmap();
                gen.forward_staging.unmap();
                gen.pending = None;
            }
        }

        // 2. Queue a fresh copy+map into the next round-robin slot, unless
        // it's still waiting on its own previous map (the GPU/CPU would
        // have to be more than `GENERATIONS` frames behind -- this frame
        // just skips kicking off a new copy rather than stomping on a
        // buffer still being read).
        if !kick_off_copy {
            return false;
        }

        let slot = self.next;
        self.next = (self.next + 1) % self.gens.len().max(1);
        if self.gens[slot].pending.is_some() {
            return false;
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ObjBatch Readback Copy"),
        });
        {
            let gen = &self.gens[slot];
            encoder.copy_buffer_to_buffer(&scratch.group_count, 0, &gen.counts_staging, 0, 4);
            encoder.copy_buffer_to_buffer(&scratch.range_count, 0, &gen.counts_staging, 4, 12);
            encoder.copy_buffer_to_buffer(&scratch.shadow_counts, 0, &gen.counts_staging, 16, 8);
            // `frame_uniform`'s first u32 is `count` -- the gather's live
            // instance count (see `FrameUniformGpu`'s doc in `src/lib.rs`).
            encoder.copy_buffer_to_buffer(&scratch.frame_uniform, 0, &gen.counts_staging, 24, 4);
            encoder.copy_buffer_to_buffer(&scratch.shadow_counts, 8, &gen.counts_staging, 28, 4);
            encoder.copy_buffer_to_buffer(&scratch.shadow_counts, 12, &gen.counts_staging, 32, 8);
            let range_bytes = gen.range_capacity_bytes();
            encoder.copy_buffer_to_buffer(
                &scratch.opaque_ranges,
                0,
                &gen.opaque_staging,
                0,
                range_bytes.min(scratch.opaque_ranges.size()),
            );
            encoder.copy_buffer_to_buffer(
                &scratch.transparent_ranges,
                0,
                &gen.transparent_staging,
                0,
                range_bytes.min(scratch.transparent_ranges.size()),
            );
            encoder.copy_buffer_to_buffer(
                &scratch.forward_ranges,
                0,
                &gen.forward_staging,
                0,
                range_bytes.min(scratch.forward_ranges.size()),
            );
        }
        queue.submit([encoder.finish()]);

        let gen = &mut self.gens[slot];
        let counts_slot = new_map_slot();
        {
            let slot = Arc::clone(&counts_slot);
            gen.counts_staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    *slot.lock().expect("map slot mutex poisoned") = Some(r);
                });
        }
        let opaque_slot = new_map_slot();
        {
            let slot = Arc::clone(&opaque_slot);
            gen.opaque_staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    *slot.lock().expect("map slot mutex poisoned") = Some(r);
                });
        }
        let transparent_slot = new_map_slot();
        {
            let slot = Arc::clone(&transparent_slot);
            gen.transparent_staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    *slot.lock().expect("map slot mutex poisoned") = Some(r);
                });
        }
        let forward_slot = new_map_slot();
        {
            let slot = Arc::clone(&forward_slot);
            gen.forward_staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    *slot.lock().expect("map slot mutex poisoned") = Some(r);
                });
        }

        gen.pending = Some(PendingMaps {
            counts_slot,
            opaque_slot,
            transparent_slot,
            forward_slot,
            counts_ready: false,
            opaque_ready: false,
            transparent_ready: false,
            forward_ready: false,
        });
        true
    }
}

/// Largest of the three range counts in a resolved generation's counts.
fn required_range_capacity(gen: &Generation) -> u32 {
    let view = gen
        .counts_staging
        .slice(..)
        .get_mapped_range()
        .expect("counts staging buffer was mapped (map_async already resolved Ok)");
    let counts: &[u32] = bytemuck::cast_slice(&view);
    counts[1].max(counts[2]).max(counts[3])
}

fn read_generation_into(
    gen: &Generation,
    opaque: &mut Vec<RangeTuple>,
    transparent: &mut Vec<RangeTuple>,
    forward: &mut Vec<RangeTuple>,
    draw_count: &mut u32,
    shadow_static: &mut u32,
    shadow_movable: &mut u32,
    shadow_transmissive: &mut u32,
    shadow_membership_fingerprint: &mut (u32, u32),
    instance_count: &mut u32,
) {
    let counts_view = gen
        .counts_staging
        .slice(..)
        .get_mapped_range()
        .expect("counts staging buffer was mapped (map_async already resolved Ok)");
    let counts: &[u32] = bytemuck::cast_slice(&counts_view);
    let group_count = counts[0];
    let n_opaque = counts[1];
    let n_transparent = counts[2];
    let n_forward = counts[3];
    let n_shadow_static = counts[4];
    let n_shadow_movable = counts[5];
    let live_instance_count = counts[6];
    let n_shadow_transmissive = counts[7];
    *shadow_membership_fingerprint = (counts[8], counts[9]);
    drop(counts_view);

    *draw_count = group_count;
    *shadow_static = n_shadow_static;
    *shadow_movable = n_shadow_movable;
    *shadow_transmissive = n_shadow_transmissive;
    *instance_count = live_instance_count;

    *opaque = read_ranges(&gen.opaque_staging, n_opaque);
    *transparent = read_ranges(&gen.transparent_staging, n_transparent);
    *forward = read_ranges(&gen.forward_staging, n_forward);
}

fn read_ranges(staging: &wgpu::Buffer, count: u32) -> Vec<RangeTuple> {
    let view = staging
        .slice(..)
        .get_mapped_range()
        .expect("range staging buffer was mapped (map_async already resolved Ok)");
    let words: &[u32] = bytemuck::cast_slice(&view);
    // GpuRangeOut: {material_class, graph_hash_lo, graph_hash_hi, start, count} -- 5 u32s.
    let n = (count as usize).min(words.len() / 5);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let base = i * 5;
        let class = words[base];
        let hash_lo = words[base + 1] as u64;
        let hash_hi = words[base + 2] as u64;
        let start = words[base + 3];
        let cnt = words[base + 4];
        out.push((class, (hash_hi << 32) | hash_lo, start, cnt));
    }
    drop(view);
    out
}
