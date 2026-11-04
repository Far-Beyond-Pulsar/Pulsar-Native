//! CPU half of the asset store: asset slots, packed-array ranges, global page
//! ids, the residency-table shadow, the CPU page cache (SceneDB `Residency`)
//! and the scheduler. Everything the GPU half needs is returned as data
//! (ranges, table writes, uploads), so this is testable without a device.

use crate::feedback::Feedback;
use crate::ranges::{Range, RangeAlloc};
use crate::scheduler::{Fetch, PageScheduler, SchedulerConfig, SchedulerStats, Upload};
use crate::source::VgeoSource;
use helio_vg_format::*;
use pulsar_scenedb::{Prefetch, Residency};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Residency-table value for a page that is not in the pool.
pub const PAGE_NOT_RESIDENT: u32 = 0xffff_ffff;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VgStreamError {
    /// The source returned an error or inconsistent data.
    Source(String),
    /// Structurally invalid asset.
    Invalid(String),
    /// Pinned root pages would not fit the page budget.
    Budget { needed_slots: u32, limit_slots: u32 },
}

impl std::fmt::Display for VgStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(s) => write!(f, "vg source error: {s}"),
            Self::Invalid(s) => write!(f, "invalid vg asset: {s}"),
            Self::Budget { needed_slots, limit_slots } => write!(
                f,
                "vg page budget exceeded: root pages need {needed_slots} slots, budget allows {limit_slots}"
            ),
        }
    }
}
impl std::error::Error for VgStreamError {}

#[derive(Clone, Debug)]
pub struct VgStreamConfig {
    /// Page pool budget in MiB (the pool grows lazily toward it).
    pub budget_mb: u32,
    /// Non-root pages uploaded per `update`.
    pub uploads_per_frame: u32,
    /// Root pages uploaded per `update` (eager, so larger than the above).
    pub root_uploads_per_frame: u32,
    /// Capacity of the GPU request list (distinct pages per readback).
    pub request_capacity: u32,
    /// Slots allocated up front.
    pub initial_slots: u32,
    /// Slots added each time the pool grows.
    pub grow_chunk_slots: u32,
    /// Load page bytes on SceneDB loader threads (`Residency::prefetch`)
    /// instead of reading synchronously inside `update`.
    pub async_loads: bool,
    pub scheduler: SchedulerConfig,
}

impl Default for VgStreamConfig {
    fn default() -> Self {
        Self {
            budget_mb: 512,
            uploads_per_frame: 16,
            root_uploads_per_frame: 64,
            request_capacity: 4096,
            initial_slots: 64,
            grow_chunk_slots: 256,
            async_loads: true,
            scheduler: SchedulerConfig::default(),
        }
    }
}

/// CPU copy of one page, as cached by SceneDB's `Residency`.
pub struct PageBytes(pub Vec<u8>);

struct CacheEntry {
    residency: Residency<PageBytes>,
    prefetch: Option<Prefetch<PageBytes>>,
}

pub struct AssetEntry {
    pub content: [u64; 2],
    pub refs: u32,
    pub header: GpuAssetHeader,
    pub nodes: Range,
    pub group_refs: Range,
    pub pages: Range,
    pub root_pages: Vec<u32>,
    pub quant: GpuAssetQuant,
    pub ready: bool,
    pub source: Arc<dyn VgeoSource>,
}

#[derive(Debug, Clone)]
pub struct Registered {
    pub slot: u32,
    pub is_new: bool,
    pub nodes: Range,
    pub group_refs: Range,
    pub page_base: u32,
    pub quant: GpuAssetQuant,
    /// Hierarchy and group refs to upload (new assets only).
    pub data: Option<(Vec<GpuHierarchyNode>, Vec<GpuGroupRef>)>,
}

/// Work for the GPU half after a release.
#[derive(Debug, Default)]
pub struct Released {
    pub freed: bool,
    pub slot: u32,
    pub table_writes: Vec<(u32, u32)>,
}

/// Work for the GPU half after an update.
pub struct UpdateOps {
    pub uploads: Vec<Upload<Arc<PageBytes>>>,
    /// `(global page, slot or PAGE_NOT_RESIDENT)`.
    pub table_writes: Vec<(u32, u32)>,
    /// `(asset slot, ready)` for assets whose readiness flipped.
    pub ready_changed: Vec<(u32, bool)>,
}

pub struct StoreCore {
    pub cfg: VgStreamConfig,
    pub frame: u64,
    assets: Vec<Option<AssetEntry>>,
    free_slots: Vec<u32>,
    by_content: HashMap<[u64; 2], u32>,
    by_page_base: BTreeMap<u32, u32>,
    pub node_alloc: RangeAlloc,
    pub ref_alloc: RangeAlloc,
    pub page_alloc: RangeAlloc,
    table: Vec<u32>,
    sched: PageScheduler,
    caches: HashMap<u32, CacheEntry>,
    pub asset_slot_capacity: u32,
    table_version: u64,
}

fn validate_page(bytes: &[u8], local: u32) -> Result<(), String> {
    if bytes.len() < std::mem::size_of::<PageHeader>() || bytes.len() > PAGE_BYTES as usize {
        return Err(format!("page {local}: bad length {}", bytes.len()));
    }
    let h: PageHeader = bytemuck::pod_read_unaligned(&bytes[..std::mem::size_of::<PageHeader>()]);
    if h.magic != PAGE_MAGIC || h.page_id != local {
        return Err(format!("page {local}: bad page header"));
    }
    Ok(())
}

impl StoreCore {
    pub fn new(cfg: VgStreamConfig, slot_limit: u32) -> Self {
        let sched = PageScheduler::new(
            cfg.scheduler,
            cfg.initial_slots,
            cfg.grow_chunk_slots,
            slot_limit,
        );
        Self {
            cfg,
            frame: 0,
            assets: Vec::new(),
            free_slots: Vec::new(),
            by_content: HashMap::new(),
            by_page_base: BTreeMap::new(),
            node_alloc: RangeAlloc::new(256),
            ref_alloc: RangeAlloc::new(256),
            page_alloc: RangeAlloc::new(1024),
            table: vec![PAGE_NOT_RESIDENT; 1024],
            sched,
            caches: HashMap::new(),
            asset_slot_capacity: 16,
            table_version: 1,
        }
    }

    pub fn asset(&self, slot: u32) -> Option<&AssetEntry> {
        self.assets.get(slot as usize).and_then(|a| a.as_ref())
    }
    pub fn assets(&self) -> impl Iterator<Item = (u32, &AssetEntry)> {
        self.assets
            .iter()
            .enumerate()
            .filter_map(|(i, a)| a.as_ref().map(|a| (i as u32, a)))
    }
    pub fn slot_of_content(&self, content: [u64; 2]) -> Option<u32> {
        self.by_content.get(&content).copied()
    }

    /// Rows of the `vg_asset_ready` buffer for the scene derivation
    /// (`helio-component`), from the caller's mapping `content id ->
    /// (interned vertex_offset, vertex_count)`. Sorted ascending by
    /// `vertex_offset`, padded with [`VgAssetReadyRow::PADDING`] to at least
    /// `min_rows` (and one) rows. Unregistered contents are skipped; an asset
    /// is flagged ready once its root pages are resident.
    pub fn asset_ready_rows(
        &self,
        mapping: &[([u64; 2], u32, u32)],
        min_rows: usize,
    ) -> Vec<VgAssetReadyRow> {
        let mut rows: Vec<VgAssetReadyRow> = mapping
            .iter()
            .filter_map(|&(content, vertex_offset, vertex_count)| {
                let slot = self.slot_of_content(content)?;
                Some(VgAssetReadyRow {
                    vertex_offset,
                    vertex_count,
                    asset_slot: slot,
                    flags: if self.is_ready(slot) {
                        VG_ASSET_READY_FLAG_READY
                    } else {
                        0
                    },
                })
            })
            .collect();
        rows.sort_by_key(|r| r.vertex_offset);
        rows.resize(rows.len().max(min_rows).max(1), VgAssetReadyRow::PADDING);
        rows
    }

    pub fn is_ready(&self, slot: u32) -> bool {
        self.asset(slot).is_some_and(|a| a.ready)
    }
    pub fn table(&self) -> &[u32] {
        &self.table
    }
    pub fn slot_of_page(&self, global: u32) -> Option<u32> {
        self.sched.slot_of(global)
    }
    /// Increments whenever any page residency changes (uploads, evictions,
    /// releases): caches of resolved pool slots are valid while it is unchanged.
    pub fn table_version(&self) -> u64 {
        self.table_version
    }

    /// Pool slot of asset-local page `local`, if resident.
    pub fn local_page_slot(&self, asset_slot: u32, local: u32) -> Option<u32> {
        let a = self.asset(asset_slot)?;
        self.sched.slot_of(a.pages.start + local)
    }

    /// Ask for (or keep resident) asset-local pages: each call is a request for
    /// non-resident pages and a recency touch for resident ones, so calling it
    /// every frame while the pages are wanted keeps them from being evicted.
    pub fn request_local_pages(&mut self, asset_slot: u32, locals: &[u32]) {
        let frame = self.frame;
        let Some(base) = self.asset(asset_slot).map(|a| (a.pages.start, a.pages.len)) else {
            return;
        };
        for &l in locals {
            if l < base.1 {
                self.sched.request(base.0 + l, 1, frame);
            }
        }
    }

    /// The asset's source, content id and lattice (for proxy builds).
    pub fn asset_source(
        &self,
        asset_slot: u32,
    ) -> Option<(Arc<dyn VgeoSource>, [u64; 2], GpuAssetQuant)> {
        self.asset(asset_slot)
            .map(|a| (a.source.clone(), a.content, a.quant))
    }

    pub fn scheduler_stats(&self) -> SchedulerStats {
        self.sched.stats()
    }
    pub fn slot_capacity(&self) -> u32 {
        self.sched.slot_capacity()
    }
    pub fn cached_pages(&self) -> usize {
        self.caches.len()
    }

    /// Register (or add a reference to) the asset with this source's content id.
    pub fn register(&mut self, source: Arc<dyn VgeoSource>) -> Result<Registered, VgStreamError> {
        let hdr = source.header();
        if hdr.magic != VGEO_MAGIC || hdr.version != FORMAT_VERSION {
            return Err(VgStreamError::Invalid("bad vgeo header".into()));
        }
        if let Some(&slot) = self.by_content.get(&hdr.content_id) {
            let a = self.assets[slot as usize].as_mut().expect("indexed asset");
            a.refs += 1;
            return Ok(Registered {
                slot,
                is_new: false,
                nodes: a.nodes,
                group_refs: a.group_refs,
                page_base: a.pages.start,
                quant: a.quant,
                data: None,
            });
        }
        let a = hdr.asset;
        let nodes = source.hierarchy().map_err(VgStreamError::Source)?;
        let refs = source.group_refs().map_err(VgStreamError::Source)?;
        if nodes.len() != a.node_count as usize || refs.len() != a.group_count as usize {
            return Err(VgStreamError::Invalid(
                "section lengths disagree with the header".into(),
            ));
        }
        if a.node_count == 0 || a.root_node >= a.node_count {
            return Err(VgStreamError::Invalid(
                "empty hierarchy or bad root node".into(),
            ));
        }
        if refs.iter().any(|r| r.page_id >= a.page_count) {
            return Err(VgStreamError::Invalid(
                "group ref names a page past page_count".into(),
            ));
        }
        let root_pages = source.root_pages();
        if root_pages.iter().any(|&p| p >= a.page_count)
            || root_pages.len() != a.root_page_count as usize
        {
            return Err(VgStreamError::Invalid(
                "root pages inconsistent with the header".into(),
            ));
        }
        let quant = source.quant();
        let page_deps = source.page_deps().map_err(VgStreamError::Source)?;
        if page_deps.len() != a.page_count as usize
            || page_deps.iter().flatten().any(|&d| d >= a.page_count)
        {
            return Err(VgStreamError::Invalid(
                "page dependency table inconsistent with the header".into(),
            ));
        }
        let needed = self.sched.pinned_roots() + root_pages.len() as u32;
        if needed > self.sched.slot_limit() {
            return Err(VgStreamError::Budget {
                needed_slots: needed,
                limit_slots: self.sched.slot_limit(),
            });
        }

        let node_r = self.node_alloc.alloc(a.node_count);
        let ref_r = self.ref_alloc.alloc(a.group_count);
        let page_r = self.page_alloc.alloc(a.page_count);
        if self.table.len() < self.page_alloc.capacity() as usize {
            self.table
                .resize(self.page_alloc.capacity() as usize, PAGE_NOT_RESIDENT);
        }
        let slot = self.free_slots.pop().unwrap_or_else(|| {
            self.assets.push(None);
            (self.assets.len() - 1) as u32
        });
        if self.assets.len() as u32 > self.asset_slot_capacity {
            self.asset_slot_capacity = (self.assets.len() as u32).next_power_of_two();
        }
        for (p, d) in page_deps.iter().enumerate() {
            self.sched.set_deps(
                page_r.start + p as u32,
                d.iter().map(|&x| page_r.start + x).collect(),
            );
        }
        for &p in &root_pages {
            self.sched.pin_root(page_r.start + p);
        }
        self.by_content.insert(hdr.content_id, slot);
        self.by_page_base.insert(page_r.start, slot);
        self.assets[slot as usize] = Some(AssetEntry {
            content: hdr.content_id,
            refs: 1,
            header: a,
            nodes: node_r,
            group_refs: ref_r,
            pages: page_r,
            root_pages,
            quant,
            ready: false,
            source,
        });
        Ok(Registered {
            slot,
            is_new: true,
            nodes: node_r,
            group_refs: ref_r,
            page_base: page_r.start,
            quant,
            data: Some((nodes, refs)),
        })
    }

    /// Drop one reference; frees everything at zero.
    pub fn release(&mut self, slot: u32) -> Released {
        let Some(a) = self.assets.get_mut(slot as usize).and_then(|a| a.as_mut()) else {
            return Released::default();
        };
        a.refs -= 1;
        if a.refs > 0 {
            return Released::default();
        }
        let a = self.assets[slot as usize].take().expect("asset");
        self.by_content.remove(&a.content);
        self.by_page_base.remove(&a.pages.start);
        let mut table_writes = Vec::new();
        for page in a.pages.start..a.pages.end() {
            self.caches.remove(&page);
            if self.sched.remove_page(page).is_some() {
                self.table[page as usize] = PAGE_NOT_RESIDENT;
                table_writes.push((page, PAGE_NOT_RESIDENT));
            }
        }
        self.node_alloc.free(a.nodes);
        self.ref_alloc.free(a.group_refs);
        self.page_alloc.free(a.pages);
        self.free_slots.push(slot);
        self.table_version += 1;
        Released {
            freed: true,
            slot,
            table_writes,
        }
    }

    fn locate(&self, global: u32) -> Option<(u32, u32)> {
        let (&base, &slot) = self.by_page_base.range(..=global).next_back()?;
        let a = self.assets[slot as usize].as_ref()?;
        (global < a.pages.end()).then_some((slot, global - base))
    }

    /// Feed one decoded readback to the scheduler.
    pub fn ingest(&mut self, fb: &Feedback) {
        let frame = self.frame;
        for &p in &fb.used {
            if self.locate(p).is_some() {
                self.sched.touch(p, frame);
            }
        }
        for &p in &fb.requests {
            if self.locate(p).is_some() {
                self.sched.request(p, 1, frame);
            }
        }
    }

    /// One scheduling step: choose uploads, keep the table shadow current,
    /// recompute readiness. Increments the frame counter.
    pub fn update(&mut self) -> UpdateOps {
        self.frame += 1;
        let frame = self.frame;
        let async_loads = self.cfg.async_loads;
        let (max_up, max_root) = (self.cfg.uploads_per_frame, self.cfg.root_uploads_per_frame);

        let Self {
            sched,
            caches,
            by_page_base,
            assets,
            ..
        } = self;
        let uploads = sched.plan(frame, max_up, max_root, |global| {
            let Some((&base, &slot)) = by_page_base.range(..=global).next_back() else {
                return Fetch::Failed;
            };
            let Some(a) = assets[slot as usize].as_ref() else {
                return Fetch::Failed;
            };
            let local = global - base;
            let entry = caches.entry(global).or_insert_with(|| {
                let src = a.source.clone();
                CacheEntry {
                    residency: Residency::lazy(
                        |p: &PageBytes| p.0.len(),
                        move || {
                            let bytes = src.read_page(local)?;
                            validate_page(&bytes, local)?;
                            Ok(PageBytes(bytes))
                        },
                    ),
                    prefetch: None,
                }
            });
            if !async_loads || entry.residency.is_resident() {
                return match entry.residency.get() {
                    Ok(b) => Fetch::Ready(b),
                    Err(e) => {
                        log::warn!("vg-stream: page {local} of asset slot {slot}: {e}");
                        Fetch::Failed
                    }
                };
            }
            let pf = entry
                .prefetch
                .get_or_insert_with(|| entry.residency.prefetch());
            if !pf.is_ready() {
                return Fetch::NotYet;
            }
            match entry.prefetch.take().expect("prefetch").wait() {
                Ok(b) => Fetch::Ready(b),
                Err(e) => {
                    log::warn!("vg-stream: page {local} of asset slot {slot}: {e}");
                    Fetch::Failed
                }
            }
        });

        let mut table_writes = Vec::with_capacity(uploads.len() * 2);
        for u in &uploads {
            if let Some(ev) = u.evicted {
                self.table[ev as usize] = PAGE_NOT_RESIDENT;
                table_writes.push((ev, PAGE_NOT_RESIDENT));
            }
        }
        for u in &uploads {
            self.table[u.page as usize] = u.slot;
            table_writes.push((u.page, u.slot));
        }

        let mut ready_changed = Vec::new();
        for (slot, a) in self.assets.iter_mut().enumerate() {
            let Some(a) = a else { continue };
            if !a.ready
                && a.root_pages
                    .iter()
                    .all(|&p| self.sched.is_resident(a.pages.start + p))
            {
                a.ready = true;
                ready_changed.push((slot as u32, true));
            }
        }
        if !table_writes.is_empty() {
            self.table_version += 1;
        }
        UpdateOps {
            uploads,
            table_writes,
            ready_changed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::Feedback;
    use crate::source::testing::FakeSource;

    fn cfg(budget_slots: u32) -> VgStreamConfig {
        VgStreamConfig {
            async_loads: false,
            initial_slots: budget_slots,
            grow_chunk_slots: budget_slots,
            ..Default::default()
        }
    }

    #[test]
    fn root_pages_gate_readiness_and_are_pinned() {
        let mut c = StoreCore::new(cfg(8), 8);
        let src = Arc::new(FakeSource::new(1, 6, 2));
        let r = c.register(src.clone()).unwrap();
        assert!(r.is_new && !c.is_ready(r.slot));
        // Roots are slower than the per-frame cap: ready only when both land.
        c.cfg.root_uploads_per_frame = 1;
        let ops = c.update();
        assert_eq!(ops.uploads.len(), 1);
        assert!(!c.is_ready(r.slot));
        let ops = c.update();
        assert_eq!(ops.ready_changed, vec![(r.slot, true)]);
        assert!(c.is_ready(r.slot));
        for p in 0..2 {
            assert!(c.table()[(r.page_base + p) as usize] != PAGE_NOT_RESIDENT);
        }
        // Table entries point at the slots the uploads used; non-roots absent.
        assert_eq!(c.table()[(r.page_base + 3) as usize], PAGE_NOT_RESIDENT);
    }

    #[test]
    fn same_content_dedupes_and_release_frees() {
        let mut c = StoreCore::new(cfg(8), 8);
        let a = c.register(Arc::new(FakeSource::new(5, 4, 1))).unwrap();
        let b = c.register(Arc::new(FakeSource::new(5, 4, 1))).unwrap();
        assert_eq!((a.slot, b.is_new), (b.slot, false));
        c.update();
        assert!(c.release(a.slot).table_writes.is_empty()); // still referenced
        let rel = c.release(a.slot);
        assert!(rel.freed && rel.table_writes.len() == 1);
        assert_eq!(c.scheduler_stats().resident_pages, 0);
        // Slot ids and ranges are reused.
        let d = c.register(Arc::new(FakeSource::new(6, 4, 1))).unwrap();
        assert_eq!((d.slot, d.page_base), (a.slot, a.page_base));
    }

    #[test]
    fn requests_stream_pages_and_eviction_spares_roots() {
        // 3 slots: asset has 1 root + 8 other pages; budget is tiny.
        let mut c = StoreCore::new(cfg(3), 3);
        let r = c.register(Arc::new(FakeSource::new(9, 9, 1))).unwrap();
        c.update();
        assert!(c.is_ready(r.slot));
        let root = r.page_base;
        for round in 0..30u32 {
            let want = r.page_base + 1 + (round % 8);
            c.ingest(&Feedback {
                requests: vec![want],
                used: vec![],
                overflowed: false,
            });
            let ops = c.update();
            for _ in 0..6 {
                c.update();
            }
            assert!(
                c.table()[root as usize] != PAGE_NOT_RESIDENT,
                "root evicted"
            );
            assert!(c.scheduler_stats().resident_pages <= 3);
            for u in &ops.uploads {
                assert_eq!(c.table()[u.page as usize], u.slot);
            }
        }
        assert!(c.scheduler_stats().evictions > 0);
    }

    #[test]
    fn feedback_for_foreign_pages_is_ignored_and_bad_pages_fail() {
        let mut c = StoreCore::new(cfg(4), 4);
        let mut bad = FakeSource::new(2, 4, 1);
        bad.fail_page = Some(2);
        let r = c.register(Arc::new(bad)).unwrap();
        c.update();
        c.ingest(&Feedback {
            requests: vec![900, r.page_base + 2],
            used: vec![901],
            overflowed: false,
        });
        let ops = c.update();
        assert!(ops.uploads.is_empty());
        assert_eq!(c.scheduler_stats().failures, 1);
    }

    #[test]
    fn root_budget_is_enforced_at_registration() {
        let mut c = StoreCore::new(cfg(2), 2);
        assert!(matches!(
            c.register(Arc::new(FakeSource::new(3, 8, 3))),
            Err(VgStreamError::Budget { .. })
        ));
    }

    #[test]
    fn async_loads_converge() {
        let mut c = StoreCore::new(
            VgStreamConfig {
                async_loads: true,
                ..cfg(4)
            },
            4,
        );
        let r = c.register(Arc::new(FakeSource::new(11, 4, 2))).unwrap();
        for _ in 0..2000 {
            c.update();
            if c.is_ready(r.slot) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("async root loads never completed");
    }
}
