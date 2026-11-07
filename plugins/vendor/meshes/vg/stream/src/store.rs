//! GPU half of the asset store: the page pool, the packed per-asset arrays,
//! the residency table and the feedback readback. See the crate docs.

/// Bytes of one asset header row (the cull reads `array<VgAssetHeader>`).
const HEADER_BYTES: u64 = std::mem::size_of::<helio_vg_format::GpuAssetHeader>() as u64;

use crate::core::*;
use crate::feedback::{self, FeedbackLayout};
use crate::source::VgeoSource;
use helio_vg_format::*;
use std::sync::mpsc;
use std::sync::Arc;

const USAGE: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE
    .union(wgpu::BufferUsages::COPY_DST)
    .union(wgpu::BufferUsages::COPY_SRC);

/// Which buffer a version counter or accessor refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VgBuffer {
    PagePool,
    Residency,
    Hierarchy,
    GroupRefs,
    AssetHeaders,
    AssetBases,
    AssetQuant,
    Feedback,
}

/// Bind group layout flavour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VgBindKind {
    /// All eight buffers; feedback is read-write. Compute stages only.
    Compute,
    /// The seven read-only buffers (no feedback) for any stage.
    ReadOnly,
}

struct GrowBuf {
    buffer: wgpu::Buffer,
    size: u64,
    version: u64,
    label: &'static str,
}

impl GrowBuf {
    fn new(device: &wgpu::Device, label: &'static str, size: u64) -> Self {
        let size = size.max(16).next_multiple_of(4);
        Self {
            buffer: Self::make(device, label, size),
            size,
            version: 1,
            label,
        }
    }
    fn make(device: &wgpu::Device, label: &'static str, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: USAGE,
            mapped_at_creation: false,
        })
    }
    /// Grow to at least `needed` bytes (geometric), copying the old contents
    /// when `keep`. Returns whether the buffer was replaced.
    fn ensure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        needed: u64,
        keep: bool,
    ) -> bool {
        if needed <= self.size {
            return false;
        }
        let new_size = needed.max(self.size * 2).next_multiple_of(4);
        let new = Self::make(device, self.label, new_size);
        if keep {
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vg-stream grow"),
            });
            enc.copy_buffer_to_buffer(&self.buffer, 0, &new, 0, self.size);
            queue.submit(Some(enc.finish()));
        }
        self.buffer = new;
        self.size = new_size;
        self.version += 1;
        true
    }
    /// Replace with a fresh buffer of exactly `size` bytes.
    fn recreate(&mut self, device: &wgpu::Device, size: u64) {
        let size = size.max(16).next_multiple_of(4);
        self.buffer = Self::make(device, self.label, size);
        self.size = size;
        self.version += 1;
    }
}

/// Opaque id of a registered asset. `slot()` is the `asset_slot` instances use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VgAssetHandle(u32);

impl VgAssetHandle {
    pub fn slot(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VgPoolStats {
    pub resident_pages: u32,
    pub root_pages_resident: u32,
    pub slot_capacity: u32,
    pub slot_limit: u32,
    pub resident_bytes: u64,
    /// Bytes of the allocated page pool buffer.
    pub pool_buffer_bytes: u64,
    pub uploads: u64,
    pub evictions: u64,
    pub failures: u64,
    /// Pages whose CPU bytes are tracked by SceneDB `Residency`.
    pub cpu_tracked_pages: u32,
    pub feedback_overflows: u64,
}

struct Pending {
    rx: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    tx: Option<mpsc::Sender<Result<(), wgpu::BufferAsyncError>>>,
    layout: FeedbackLayout,
}

pub struct VgAssetStore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    core: StoreCore,
    pool: GrowBuf,
    residency: GrowBuf,
    nodes: GrowBuf,
    group_refs: GrowBuf,
    headers: GrowBuf,
    bases: GrowBuf,
    quant: GrowBuf,
    feedback: GrowBuf,
    fb_layout: FeedbackLayout,
    staging: wgpu::Buffer,
    pending: Option<Pending>,
    content_version: u64,
    feedback_overflows: u64,
}

fn make_staging(device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vg-stream feedback readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl VgAssetStore {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, cfg: VgStreamConfig) -> Self {
        let limits = device.limits();
        let device_bytes =
            u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        let device_slots = (device_bytes / u64::from(PAGE_BYTES)).min(u64::from(u32::MAX)) as u32;
        let budget_slots =
            (u64::from(cfg.budget_mb) * 1024 * 1024 / u64::from(PAGE_BYTES)).max(1) as u32;
        let slot_limit = budget_slots.min(device_slots).max(1);
        let core = StoreCore::new(cfg.clone(), slot_limit);

        let pool = GrowBuf::new(
            device,
            "vg-stream page pool",
            u64::from(core.slot_capacity()) * u64::from(PAGE_BYTES),
        );
        let page_cap = core.page_alloc.capacity();
        let residency = GrowBuf::new(device, "vg-stream residency table", u64::from(page_cap) * 4);
        queue.write_buffer(&residency.buffer, 0, bytemuck::cast_slice(core.table()));
        let nodes = GrowBuf::new(
            device,
            "vg-stream hierarchy",
            u64::from(core.node_alloc.capacity()) * 96,
        );
        let group_refs = GrowBuf::new(
            device,
            "vg-stream group refs",
            u64::from(core.ref_alloc.capacity()) * 8,
        );
        let headers = GrowBuf::new(
            device,
            "vg-stream asset headers",
            u64::from(core.asset_slot_capacity) * HEADER_BYTES,
        );
        let bases = GrowBuf::new(
            device,
            "vg-stream asset bases",
            u64::from(core.asset_slot_capacity) * 16,
        );
        let quant = GrowBuf::new(
            device,
            "vg-stream asset quant",
            u64::from(core.asset_slot_capacity) * 16,
        );
        queue.write_buffer(
            &quant.buffer,
            0,
            &vec![0u8; (quant.size as usize).min(1 << 20)],
        );
        let fb_layout = FeedbackLayout::new(cfg.request_capacity.max(1), page_cap);
        let feedback = GrowBuf::new(device, "vg-stream feedback", fb_layout.total_bytes());
        queue.write_buffer(
            &feedback.buffer,
            0,
            bytemuck::cast_slice(&fb_layout.initial_words()),
        );
        let staging = make_staging(device, fb_layout.total_bytes());
        Self {
            device: device.clone(),
            queue: queue.clone(),
            core,
            pool,
            residency,
            nodes,
            group_refs,
            headers,
            bases,
            quant,
            feedback,
            fb_layout,
            staging,
            pending: None,
            content_version: 1,
            feedback_overflows: 0,
        }
    }

    // ── registration ────────────────────────────────────────────────────────

    /// Register an asset (or add a reference to one with the same content id).
    /// Hierarchy, group refs and header upload now; root pages stream in over
    /// the following `update` calls. Poll [`Self::is_ready`].
    pub fn register(
        &mut self,
        source: Arc<dyn VgeoSource>,
    ) -> Result<VgAssetHandle, VgStreamError> {
        let reg = self.core.register(source)?;
        self.sync_capacities();
        if let Some((nodes, refs)) = &reg.data {
            let a = self.core.asset(reg.slot).expect("registered asset");
            self.queue.write_buffer(
                &self.nodes.buffer,
                u64::from(reg.nodes.start) * 96,
                bytemuck::cast_slice(nodes),
            );
            self.queue.write_buffer(
                &self.group_refs.buffer,
                u64::from(reg.group_refs.start) * 8,
                bytemuck::cast_slice(refs),
            );
            // GPU view of the header: root leaves (+1) in the words no shader reads,
            // see `helio_vg_format::gpu_header_with_root_leaves`.
            let gpu_header = helio_vg_format::gpu_header_with_root_leaves(a.header, nodes);
            self.queue.write_buffer(
                &self.headers.buffer,
                u64::from(reg.slot) * HEADER_BYTES,
                bytemuck::bytes_of(&gpu_header),
            );
            let bases = [reg.nodes.start, reg.group_refs.start, reg.page_base, 0u32];
            self.queue.write_buffer(
                &self.bases.buffer,
                u64::from(reg.slot) * 16,
                bytemuck::cast_slice(&bases),
            );
            self.queue.write_buffer(
                &self.quant.buffer,
                u64::from(reg.slot) * 16,
                bytemuck::bytes_of(&reg.quant),
            );
            self.content_version += 1;
        }
        Ok(VgAssetHandle(reg.slot))
    }

    /// Drop one reference. At zero the asset's slot, ranges and pool pages
    /// are freed and its header row is zeroed (`page_count == 0`).
    pub fn release(&mut self, handle: VgAssetHandle) {
        let rel = self.core.release(handle.0);
        if !rel.freed {
            return;
        }
        for (page, value) in rel.table_writes {
            self.queue.write_buffer(
                &self.residency.buffer,
                u64::from(page) * 4,
                bytemuck::bytes_of(&value),
            );
        }
        self.queue.write_buffer(
            &self.headers.buffer,
            u64::from(rel.slot) * HEADER_BYTES,
            &[0u8; HEADER_BYTES as usize],
        );
        self.queue
            .write_buffer(&self.bases.buffer, u64::from(rel.slot) * 16, &[0u8; 16]);
        self.queue
            .write_buffer(&self.quant.buffer, u64::from(rel.slot) * 16, &[0u8; 16]);
        self.content_version += 1;
    }

    /// True once every root page of the asset is resident in the pool.
    pub fn is_ready(&self, handle: VgAssetHandle) -> bool {
        self.core.is_ready(handle.0)
    }

    /// See [`StoreCore::asset_ready_rows`]; upload the result as the
    /// `vg_asset_ready` scene buffer whenever [`Self::content_version`] changes.
    pub fn asset_ready_rows(
        &self,
        mapping: &[([u64; 2], u32, u32)],
        min_rows: usize,
    ) -> Vec<VgAssetReadyRow> {
        self.core.asset_ready_rows(mapping, min_rows)
    }

    pub fn header(&self, handle: VgAssetHandle) -> Option<GpuAssetHeader> {
        self.core.asset(handle.0).map(|a| a.header)
    }

    // ── per-frame ───────────────────────────────────────────────────────────

    /// Append the feedback copy + clear to `encoder`. Call after the passes
    /// that append requests, once per frame; does nothing while a previous
    /// readback is still in flight (requests then keep accumulating, deduped).
    pub fn record_feedback_readback(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if self.pending.is_some() {
            return;
        }
        let l = self.fb_layout;
        encoder.copy_buffer_to_buffer(&self.feedback.buffer, 0, &self.staging, 0, l.total_bytes());
        encoder.clear_buffer(&self.feedback.buffer, 0, Some(4));
        encoder.clear_buffer(
            &self.feedback.buffer,
            u64::from(l.request_bits_offset_words()) * 4,
            Some(u64::from(l.bits_words) * 8),
        );
        let (tx, rx) = mpsc::channel();
        self.pending = Some(Pending {
            rx,
            tx: Some(tx),
            layout: l,
        });
    }

    /// Call after the submit that contained [`Self::record_feedback_readback`]:
    /// starts mapping the staging buffer. Never blocks.
    pub fn after_submit(&mut self) {
        let Some(p) = &mut self.pending else { return };
        let Some(tx) = p.tx.take() else { return };
        self.staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
    }

    /// Non-blocking: collect a finished readback, if any, into the scheduler.
    /// Returns whether one was decoded.
    pub fn poll_feedback(&mut self) -> bool {
        let Some(p) = &self.pending else { return false };
        if p.tx.is_some() {
            return false; // recorded but not yet submitted/mapped
        }
        let _ = self.device.poll(wgpu::PollType::Poll);
        let Some(p) = &self.pending else { return false };
        match p.rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) | Ok(Err(_)) => {
                log::warn!("vg-stream: feedback readback failed; dropping it");
                self.pending = None;
                false
            }
            Ok(Ok(())) => {
                let layout = p.layout;
                let words: Vec<u32> = {
                    let Ok(view) = self.staging.slice(..).get_mapped_range() else {
                        self.pending = None;
                        return false;
                    };
                    bytemuck::pod_collect_to_vec(&view)
                };
                self.staging.unmap();
                self.pending = None;
                let fb = feedback::decode(&words, layout, self.core.page_alloc.capacity());
                if fb.overflowed {
                    self.feedback_overflows += 1;
                }
                self.core.ingest(&fb);
                true
            }
        }
    }

    /// One streaming step: collect feedback, schedule, upload (bounded by the
    /// configured per-frame budget), update the residency table.
    pub fn update(&mut self) {
        self.poll_feedback();
        let ops = self.core.update();
        self.sync_capacities();
        for u in &ops.uploads {
            let off = u64::from(u.slot) * u64::from(PAGE_BYTES);
            self.queue
                .write_buffer(&self.pool.buffer, off, &u.payload.0);
        }
        for &(page, value) in &ops.table_writes {
            self.queue.write_buffer(
                &self.residency.buffer,
                u64::from(page) * 4,
                bytemuck::bytes_of(&value),
            );
        }
        for &(slot, ready) in &ops.ready_changed {
            let w = u32::from(ready);
            self.queue.write_buffer(
                &self.bases.buffer,
                u64::from(slot) * 16 + 12,
                bytemuck::bytes_of(&w),
            );
            self.content_version += 1;
        }
    }

    /// Grow GPU buffers to match the CPU-side capacities.
    fn sync_capacities(&mut self) {
        let (d, q) = (&self.device, &self.queue);
        self.pool.ensure(
            d,
            q,
            u64::from(self.core.slot_capacity()) * u64::from(PAGE_BYTES),
            true,
        );
        self.nodes
            .ensure(d, q, u64::from(self.core.node_alloc.capacity()) * 96, true);
        self.group_refs
            .ensure(d, q, u64::from(self.core.ref_alloc.capacity()) * 8, true);
        self.headers.ensure(
            d,
            q,
            u64::from(self.core.asset_slot_capacity) * HEADER_BYTES,
            true,
        );
        self.bases
            .ensure(d, q, u64::from(self.core.asset_slot_capacity) * 16, true);
        self.quant
            .ensure(d, q, u64::from(self.core.asset_slot_capacity) * 16, true);
        let page_cap = self.core.page_alloc.capacity();
        if self.residency.ensure(d, q, u64::from(page_cap) * 4, false) {
            q.write_buffer(
                &self.residency.buffer,
                0,
                bytemuck::cast_slice(self.core.table()),
            );
        }
        let want = FeedbackLayout::new(self.fb_layout.list_capacity, page_cap);
        if want.bits_words > self.fb_layout.bits_words {
            self.fb_layout = want;
            self.feedback.recreate(d, want.total_bytes());
            q.write_buffer(
                &self.feedback.buffer,
                0,
                bytemuck::cast_slice(&want.initial_words()),
            );
            self.staging = make_staging(d, want.total_bytes());
            self.pending = None; // layout changed under it
        }
    }

    // ── handles for passes ──────────────────────────────────────────────────

    pub fn buffer(&self, which: VgBuffer) -> &wgpu::Buffer {
        &self.gb(which).buffer
    }

    /// Increments each time that buffer is replaced (bind groups using it go stale).
    pub fn buffer_version(&self, which: VgBuffer) -> u64 {
        self.gb(which).version
    }

    /// Sum of all buffer versions: rebuild bind groups when it changes.
    pub fn bind_version(&self) -> u64 {
        use VgBuffer::*;
        [
            PagePool,
            Residency,
            Hierarchy,
            GroupRefs,
            AssetHeaders,
            AssetBases,
            AssetQuant,
            Feedback,
        ]
        .iter()
        .map(|&b| self.buffer_version(b))
        .sum()
    }

    /// Increments when an asset is registered/released or becomes ready:
    /// derivations that join instance rows to assets re-evaluate when it changes.
    pub fn content_version(&self) -> u64 {
        self.content_version
    }

    fn gb(&self, which: VgBuffer) -> &GrowBuf {
        match which {
            VgBuffer::PagePool => &self.pool,
            VgBuffer::Residency => &self.residency,
            VgBuffer::Hierarchy => &self.nodes,
            VgBuffer::GroupRefs => &self.group_refs,
            VgBuffer::AssetHeaders => &self.headers,
            VgBuffer::AssetBases => &self.bases,
            VgBuffer::AssetQuant => &self.quant,
            VgBuffer::Feedback => &self.feedback,
        }
    }

    /// Layout matching [`crate::bindings_wgsl`] for `(group, first_binding)`.
    pub fn bind_group_layout(
        device: &wgpu::Device,
        kind: VgBindKind,
        first_binding: u32,
    ) -> wgpu::BindGroupLayout {
        let vis = match kind {
            VgBindKind::Compute => wgpu::ShaderStages::COMPUTE,
            VgBindKind::ReadOnly => {
                wgpu::ShaderStages::VERTEX
                    | wgpu::ShaderStages::FRAGMENT
                    | wgpu::ShaderStages::COMPUTE
            }
        };
        let count = if kind == VgBindKind::Compute { 8 } else { 7 };
        let entries: Vec<wgpu::BindGroupLayoutEntry> = (0..count)
            .map(|i| wgpu::BindGroupLayoutEntry {
                binding: first_binding + i,
                visibility: vis,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: i < 7 },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vg-stream"),
            entries: &entries,
        })
    }

    /// Bind group over the current buffers. Rebuild when [`Self::bind_version`] changes.
    pub fn create_bind_group(
        &self,
        layout: &wgpu::BindGroupLayout,
        kind: VgBindKind,
        first_binding: u32,
    ) -> wgpu::BindGroup {
        use VgBuffer::*;
        let order = [
            PagePool,
            Residency,
            Hierarchy,
            GroupRefs,
            AssetHeaders,
            AssetBases,
            AssetQuant,
            Feedback,
        ];
        let count = if kind == VgBindKind::Compute { 8 } else { 7 };
        let entries: Vec<wgpu::BindGroupEntry> = order[..count]
            .iter()
            .enumerate()
            .map(|(i, &b)| wgpu::BindGroupEntry {
                binding: first_binding + i as u32,
                resource: self.buffer(b).as_entire_binding(),
            })
            .collect();
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg-stream"),
            layout,
            entries: &entries,
        })
    }

    pub fn stats(&self) -> VgPoolStats {
        let s = self.core.scheduler_stats();
        VgPoolStats {
            resident_pages: s.resident_pages,
            root_pages_resident: s.root_pages_resident,
            slot_capacity: s.slot_capacity,
            slot_limit: s.slot_limit,
            resident_bytes: u64::from(s.resident_pages) * u64::from(PAGE_BYTES),
            pool_buffer_bytes: self.pool.size,
            uploads: s.uploads,
            evictions: s.evictions,
            failures: s.failures,
            cpu_tracked_pages: self.core.cached_pages() as u32,
            feedback_overflows: self.feedback_overflows,
        }
    }

    /// Pool slot of a global page, if resident (tests/tools).
    pub fn page_slot(&self, global_page: u32) -> Option<u32> {
        self.core.slot_of_page(global_page)
    }

    /// Global id of local page `local` of an asset.
    pub fn global_page(&self, handle: VgAssetHandle, local: u32) -> Option<u32> {
        self.core.asset(handle.0).map(|a| a.pages.start + local)
    }

    pub fn core(&self) -> &StoreCore {
        &self.core
    }

    /// Mutable access for the proxy tier (page requests); the streaming step
    /// itself stays [`Self::update`].
    pub fn core_mut(&mut self) -> &mut StoreCore {
        &mut self.core
    }
}

/// The eight buffers of the stream binding group, in binding order
/// (see [`crate::bindings_wgsl`]). Plain references so passes and tests can
/// bind hand-built data without a store.
#[derive(Clone, Copy)]
pub struct VgStreamBuffers<'a> {
    pub page_pool: &'a wgpu::Buffer,
    pub residency: &'a wgpu::Buffer,
    pub nodes: &'a wgpu::Buffer,
    pub group_refs: &'a wgpu::Buffer,
    pub asset_headers: &'a wgpu::Buffer,
    pub asset_bases: &'a wgpu::Buffer,
    pub asset_quant: &'a wgpu::Buffer,
    /// Read-write request/touch feedback (compute only).
    pub feedback: &'a wgpu::Buffer,
}

impl<'a> VgStreamBuffers<'a> {
    pub fn as_array(&self) -> [&'a wgpu::Buffer; 8] {
        [
            self.page_pool,
            self.residency,
            self.nodes,
            self.group_refs,
            self.asset_headers,
            self.asset_bases,
            self.asset_quant,
            self.feedback,
        ]
    }
}

impl VgAssetStore {
    pub fn stream_buffers(&self) -> VgStreamBuffers<'_> {
        VgStreamBuffers {
            page_pool: self.buffer(VgBuffer::PagePool),
            residency: self.buffer(VgBuffer::Residency),
            nodes: self.buffer(VgBuffer::Hierarchy),
            group_refs: self.buffer(VgBuffer::GroupRefs),
            asset_headers: self.buffer(VgBuffer::AssetHeaders),
            asset_bases: self.buffer(VgBuffer::AssetBases),
            asset_quant: self.buffer(VgBuffer::AssetQuant),
            feedback: self.buffer(VgBuffer::Feedback),
        }
    }
}

/// What to record to read the feedback buffer back: a copy of the whole
/// buffer into `staging`, then clearing the counter word and the request
/// bits (`clears` are `(byte offset, byte size)`). Returned by
/// [`VgAssetStore::begin_feedback_readback`] for callers that record into
/// something other than a bare `wgpu::CommandEncoder` (the render graph).
pub struct FeedbackReadbackPlan {
    pub src: wgpu::Buffer,
    pub staging: wgpu::Buffer,
    pub bytes: u64,
    pub clears: [(u64, u64); 2],
}

impl VgAssetStore {
    /// As [`Self::record_feedback_readback`], but returns the commands to
    /// record instead of recording them. The caller must record them and
    /// submit them before calling [`Self::after_submit`]. `None` while a
    /// previous readback is still in flight.
    pub fn begin_feedback_readback(&mut self) -> Option<FeedbackReadbackPlan> {
        if self.pending.is_some() {
            return None;
        }
        let l = self.fb_layout;
        let plan = FeedbackReadbackPlan {
            src: self.feedback.buffer.clone(),
            staging: self.staging.clone(),
            bytes: l.total_bytes(),
            clears: [
                (0, 4),
                (
                    u64::from(l.request_bits_offset_words()) * 4,
                    u64::from(l.bits_words) * 8,
                ),
            ],
        };
        let (tx, rx) = mpsc::channel();
        self.pending = Some(Pending {
            rx,
            tx: Some(tx),
            layout: l,
        });
        Some(plan)
    }
}
