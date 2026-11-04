//! Proxy triangle meshes for the ray-traced Tier 3 (plan section 5.4 "3-RT").
//!
//! Hardware ray tracing intersects a fixed BLAS and cannot evaluate a
//! per-pixel DAG cut, so each virtual-geometry asset is also offered as a
//! small ladder of *proxy LODs*: uniform cuts of its cluster DAG expanded to
//! plain triangles (`Float32x3` positions + `u32` indices, ready for a BLAS).
//!
//! * [`build_proxy`] is pure CPU. It reads the asset through [`VgeoSource`]
//!   (the always-resident hierarchy for the error ladder, then every page once)
//!   and selects, per error level `E`, the clusters with
//!   `self_error <= E < parent_error`. Builder invariants (`parent >= self`,
//!   strict order along a path) make this a consistent DAG cut: every
//!   leaf-to-root path contributes exactly one cluster, so there are no
//!   cracks or double coverage between clusters of different depth.
//!   Positions are expanded with the same lattice decode the GPU uses
//!   (`origin + q * step`, [`helio_vg_format::testing::dequantize`]), so a hit
//!   triangle's barycentrics apply to exactly the vertices `vis_resolve` later
//!   decodes from the page pool.
//! * [`ProxyCache`] runs builds on background threads (never the render
//!   thread), keeps finished builds under a byte budget, and evicts the least
//!   recently used entries that are not in active use.
//!
//! Each proxy triangle remembers the real cluster triangle it came from
//! (`prim_map`: `proxy cluster ordinal << VIS_TRI_BITS | triangle`), and each
//! proxy cluster its page (`cluster_pages`) and local index, because the pool
//! slot a page lives in is only known at frame time.
//!
//! Limitations (documented, not hidden): a proxy is a *uniform* cut, so error
//! is bounded by the level's `error` in object space, not per pixel; one level
//! per asset per frame (large instances spanning a wide distance range use the
//! level for their nearest point); no per-triangle material data. With
//! [`ProxyOptions::uvs`] each vertex also carries its uv0 (the attribute
//! stream's, decoded from f16), so alpha-tested materials can be tested at
//! a hit; vertices then weld on lattice coordinate and uv, keeping UV seams.
//!
//! Two ladders use this: the dense tier's ([`ProxyOptions::default`]: the
//! leaves, then cuts up to the root, one picked per instance and frame from
//! its projected error) and the scene TLAS's ([`ProxyOptions::trace`]: ONE
//! coarse cut per asset, shared by all its instances, for rays that need the
//! object's shape, not its pixels: GI probes, ray-traced shadows).

use crate::source::VgeoSource;
use helio_vg_format::testing::dequantize;
use helio_vg_format::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

/// Controls of [`build_proxy`].
#[derive(Clone, Copy, Debug)]
pub struct ProxyOptions {
    /// Number of error levels including the finest (leaf) one.
    pub levels: usize,
    /// Error ratio between neighbouring levels (levels are
    /// `0, Emax/ratio^(n-2), ..., Emax/ratio, Emax`).
    pub ratio: f32,
    /// Levels with more triangles than this are skipped (the finest *kept*
    /// level then exceeds tau closer in, which the RT path reports).
    pub max_triangles_per_lod: u32,
    /// Error of the finest level as a fraction of the asset's bounding
    /// radius. Zero: the finest level is the leaves (the original geometry)
    /// and the others divide the root's error by `ratio`; above zero, the
    /// levels are `finest, finest * ratio, ...` up to the root's error.
    pub finest_relative_error: f32,
    /// Keep at most this many levels: the finest that fit the triangle cap.
    pub max_lods: usize,
    /// Decode each vertex's uv0 into [`ProxyLodCpu::uvs`].
    pub uvs: bool,
}

impl Default for ProxyOptions {
    fn default() -> Self {
        Self {
            levels: 5,
            ratio: 4.0,
            max_triangles_per_lod: 8 << 20,
            finest_relative_error: 0.0,
            max_lods: usize::MAX,
            uvs: false,
        }
    }
}

impl ProxyOptions {
    /// Error bound of the scene TLAS's proxies, as a fraction of the asset's
    /// bounding radius: 1 cm on a statue 2 m across, 10 cm on a building
    /// 20 m across. GI probes are 0.5 m apart at the finest and integrate
    /// whole hemispheres; a ray-traced shadow of a surface displaced this
    /// little stays on it. A coarser bound shows as light leaking through
    /// thin VG walls and creases; a finer one only costs memory.
    pub const TRACE_RELATIVE_ERROR: f32 = 0.01;
    /// Triangle budget of a scene TLAS proxy. Its BLAS costs roughly 60 to
    /// 100 bytes a triangle on current hardware (2 to 3 MiB at the budget),
    /// plus about 18 bytes a triangle of positions and indices. An asset
    /// whose cut at [`Self::TRACE_RELATIVE_ERROR`] needs more takes the
    /// first coarser cut (each twice the error) that fits.
    pub const TRACE_MAX_TRIANGLES: u32 = 32 * 1024;

    /// One proxy per asset for the scene's TLAS: the finest cut whose error
    /// is at most [`Self::TRACE_RELATIVE_ERROR`] of the asset's bounding
    /// radius and that fits [`Self::TRACE_MAX_TRIANGLES`], else the first
    /// coarser cut (each twice the error, up to the root) that fits. With
    /// UVs: rays alpha-test cut-out materials on it.
    pub fn trace() -> Self {
        Self {
            levels: 12,
            ratio: 2.0,
            max_triangles_per_lod: Self::TRACE_MAX_TRIANGLES,
            finest_relative_error: Self::TRACE_RELATIVE_ERROR,
            max_lods: 1,
            uvs: true,
        }
    }
}

/// One proxy LOD in CPU memory.
#[derive(Clone, Debug)]
pub struct ProxyLodCpu {
    /// Object-space error bound of this cut (units of `GpuCluster::self_error`).
    pub error: f32,
    pub positions: Vec<[f32; 3]>,
    /// Per vertex, uv0 ([`ProxyOptions::uvs`]; else empty).
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Per triangle: `proxy cluster ordinal << VIS_TRI_BITS | triangle in cluster`.
    pub prim_map: Vec<u32>,
    /// Per proxy cluster: local cluster index inside its page.
    pub clusters: Vec<u32>,
    /// Per proxy cluster: asset-local page id.
    pub cluster_pages: Vec<u32>,
    /// Distinct asset-local pages the clusters live in (sorted).
    pub pages: Vec<u32>,
}

impl ProxyLodCpu {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Bytes of GPU + CPU-side tables this level costs (vertex + uv + index
    /// + prim_map + per-cluster rows).
    pub fn bytes(&self) -> usize {
        self.positions.len() * 12
            + self.uvs.len() * 8
            + self.indices.len() * 4
            + self.prim_map.len() * 4
            + self.clusters.len() * 8
    }
}

/// All proxy LODs of an asset, finest first, `error` strictly increasing.
#[derive(Clone, Debug)]
pub struct ProxyBuild {
    pub lods: Vec<ProxyLodCpu>,
    /// Asset-space bounding sphere (xyz, radius) of the finest kept cut.
    pub bounds: [f32; 4],
    /// Triangles of the original asset (finest cut in the file).
    pub source_triangles: u32,
    /// Levels dropped because they exceeded `max_triangles_per_lod`.
    pub skipped_levels: u32,
}

impl ProxyBuild {
    pub fn bytes(&self) -> usize {
        self.lods.iter().map(ProxyLodCpu::bytes).sum()
    }
}

fn largest_finite_parent_error(nodes: &[GpuHierarchyNode]) -> f32 {
    let mut m = 0.0f32;
    for n in nodes {
        for i in 0..NODE_FANOUT {
            if n.children[i] != NODE_INVALID && n.max_parent_error[i].is_finite() {
                m = m.max(n.max_parent_error[i]);
            }
        }
    }
    m
}

/// Bounding radius of an asset: of its root cut's vertices (the root cut
/// covers the whole asset and lives in the small, always-resident root
/// pages; the hierarchy's spheres nest loosely, several times too big).
fn root_cut_radius(source: &dyn VgeoSource, quant: &GpuAssetQuant) -> Result<f32, String> {
    let mut points = Vec::new();
    for page_id in source.root_pages() {
        let bytes = source.read_page(page_id)?;
        let ph: PageHeader = bytes
            .get(..std::mem::size_of::<PageHeader>())
            .map(bytemuck::pod_read_unaligned)
            .ok_or_else(|| format!("proxy: page {page_id} truncated"))?;
        for local in 0..ph.cluster_count {
            let co =
                ph.clusters_offset as usize + local as usize * std::mem::size_of::<GpuCluster>();
            let c: GpuCluster = bytes
                .get(co..co + std::mem::size_of::<GpuCluster>())
                .map(bytemuck::pod_read_unaligned)
                .ok_or_else(|| format!("proxy: page {page_id} cluster {local} out of range"))?;
            if c.flags & GpuCluster::FLAG_ROOT != 0 {
                points.extend(decode_cluster(&bytes, &c, quant, false)?.positions);
            }
        }
    }
    Ok(bounding_sphere(&points)[3])
}

/// The error thresholds `build_proxy` cuts at, finest first, for an asset
/// whose root error is `root_error` and bounding radius `radius`.
pub fn error_ladder(root_error: f32, radius: f32, opts: &ProxyOptions) -> Vec<f32> {
    let n = opts.levels.max(1);
    let ratio = opts.ratio.max(1.5);
    if !(root_error > 0.0) {
        // One level: every cluster is both a leaf and a root.
        return vec![0.0];
    }
    // Just above the root's error: the root cut (roots' parents are infinite).
    let top = root_error * 1.0001;
    let mut out = Vec::with_capacity(n);
    if opts.finest_relative_error > 0.0 {
        let mut e = opts.finest_relative_error * radius;
        while out.len() < n {
            out.push(e.min(top));
            if e >= top {
                break;
            }
            e *= ratio;
        }
        return out;
    }
    // Leaf clusters have self_error 0; a tiny positive threshold also takes
    // leaves whose measured error is a rounding residue.
    out.push(0.0);
    for k in 1..n {
        out.push(top / ratio.powi((n - 1 - k) as i32));
    }
    out
}

struct LodBuild {
    error: f32,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    prim_map: Vec<u32>,
    clusters: Vec<u32>,
    cluster_pages: Vec<u32>,
    /// Lattice coordinate and uv0 bits (zero without UVs) to vertex.
    weld: HashMap<([u32; 3], u32), u32>,
    over_cap: bool,
}

/// Build the proxy ladder of one asset. Reads every page once; run it on a
/// worker thread. `cancel` (when set) aborts early with an error.
pub fn build_proxy(
    source: &dyn VgeoSource,
    opts: &ProxyOptions,
    cancel: Option<&AtomicBool>,
) -> Result<ProxyBuild, String> {
    let header = source.header();
    let quant = source.quant();
    let nodes = source.hierarchy()?;
    let root_error = largest_finite_parent_error(&nodes);
    if !(root_error > 0.0) && opts.finest_relative_error <= 0.0 {
        return Err("proxy: asset has no finite hierarchy error (single level)".into());
    }
    // The radius only matters to a relative finest error (reads the root pages).
    let radius = if opts.finest_relative_error > 0.0 {
        root_cut_radius(source, &quant)?
    } else {
        0.0
    };
    let ladder = error_ladder(root_error, radius, opts);
    let mut lods: Vec<LodBuild> = ladder
        .iter()
        .map(|&error| LodBuild {
            error,
            positions: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            prim_map: Vec::new(),
            clusters: Vec::new(),
            cluster_pages: Vec::new(),
            weld: HashMap::new(),
            over_cap: false,
        })
        .collect();
    let mut source_triangles = 0u32;

    for page_id in 0..header.asset.page_count {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err("proxy: cancelled".into());
        }
        let bytes = source.read_page(page_id)?;
        let ph_size = std::mem::size_of::<PageHeader>();
        if bytes.len() < ph_size {
            return Err(format!("proxy: page {page_id} truncated"));
        }
        let ph: PageHeader = bytemuck::pod_read_unaligned(&bytes[..ph_size]);
        if ph.magic != PAGE_MAGIC {
            return Err(format!("proxy: page {page_id} bad magic"));
        }
        for local in 0..ph.cluster_count {
            let co =
                ph.clusters_offset as usize + local as usize * std::mem::size_of::<GpuCluster>();
            let c: GpuCluster = bytes
                .get(co..co + std::mem::size_of::<GpuCluster>())
                .map(bytemuck::pod_read_unaligned)
                .ok_or_else(|| format!("proxy: page {page_id} cluster {local} out of range"))?;
            if c.flags & GpuCluster::FLAG_LEAF != 0 {
                source_triangles += c.tri_count();
            }
            // Which levels select this cluster: self <= E < parent.
            let mut decoded: Option<Decoded> = None;
            for lod in lods.iter_mut() {
                if lod.over_cap || !(c.self_error <= lod.error && lod.error < c.parent_error) {
                    continue;
                }
                if decoded.is_none() {
                    decoded = Some(decode_cluster(&bytes, &c, &quant, opts.uvs)?);
                }
                let d = decoded.as_ref().unwrap();
                let ordinal = lod.clusters.len() as u32;
                lod.clusters.push(local);
                lod.cluster_pages.push(page_id);
                // Weld on the lattice coordinate: shared vertices decode
                // bit-identically across clusters (asset-global lattice).
                // With UVs, on the uv too: a seam's two sides stay apart.
                let mut map = [0u32; 128];
                for (v, (p, q)) in d.positions.iter().zip(&d.lattice).enumerate() {
                    let uv_bits = d.uv_bits.get(v).copied().unwrap_or(0);
                    let next = lod.positions.len() as u32;
                    let id = *lod.weld.entry((*q, uv_bits)).or_insert(next);
                    if id == next {
                        lod.positions.push(*p);
                        if opts.uvs {
                            lod.uvs.push(uv0(uv_bits));
                        }
                    }
                    map[v] = id;
                }
                let tris = &d.tris;
                for (t, tri) in tris.iter().enumerate() {
                    lod.indices.extend_from_slice(&[
                        map[tri[0] as usize],
                        map[tri[1] as usize],
                        map[tri[2] as usize],
                    ]);
                    lod.prim_map.push((ordinal << VIS_TRI_BITS) | t as u32);
                }
                if lod.prim_map.len() as u32 > opts.max_triangles_per_lod {
                    // Too big to be useful: drop its data now so it does not
                    // dominate memory while the other levels finish.
                    lod.over_cap = true;
                    lod.positions = Vec::new();
                    lod.uvs = Vec::new();
                    lod.indices = Vec::new();
                    lod.prim_map = Vec::new();
                    lod.clusters = Vec::new();
                    lod.cluster_pages = Vec::new();
                    lod.weld = HashMap::new();
                }
            }
        }
    }

    let mut out = Vec::new();
    let mut skipped = 0;
    let mut last_tris = usize::MAX;
    for lod in lods {
        if lod.over_cap {
            skipped += 1;
            continue;
        }
        if out.len() >= opts.max_lods {
            break;
        }
        if lod.indices.is_empty() {
            continue;
        }
        let tris = lod.prim_map.len();
        // A level that does not reduce anything is not a different proxy.
        if tris >= last_tris {
            continue;
        }
        last_tris = tris;
        let mut pages = lod.cluster_pages.clone();
        pages.sort_unstable();
        pages.dedup();
        out.push(ProxyLodCpu {
            error: lod.error,
            positions: lod.positions,
            uvs: lod.uvs,
            indices: lod.indices,
            prim_map: lod.prim_map,
            clusters: lod.clusters,
            cluster_pages: lod.cluster_pages,
            pages,
        });
    }
    if out.is_empty() {
        return Err("proxy: no usable level (all empty or over the triangle cap)".into());
    }
    // Strictly increasing error (the ladder is, but dedup above can only drop).
    let bounds = bounding_sphere(&out[0].positions);
    Ok(ProxyBuild {
        lods: out,
        bounds,
        source_triangles,
        skipped_levels: skipped,
    })
}

fn bounding_sphere(points: &[[f32; 3]]) -> [f32; 4] {
    if points.is_empty() {
        return [0.0; 4];
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in points {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    let c = [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ];
    let r2 = points
        .iter()
        .map(|p| (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2))
        .fold(0.0, f32::max);
    [c[0], c[1], c[2], r2.sqrt()]
}

/// One cluster, decoded.
struct Decoded {
    positions: Vec<[f32; 3]>,
    lattice: Vec<[u32; 3]>,
    /// uv0 as stored (f16x2 bits), when asked for.
    uv_bits: Vec<u32>,
    tris: Vec<[u8; 3]>,
}

/// uv0 of f16x2 bits.
fn uv0(bits: u32) -> [f32; 2] {
    [f16_to_f32(bits & 0xffff), f16_to_f32(bits >> 16)]
}

fn f16_to_f32(h: u32) -> f32 {
    let (e, m) = ((h >> 10) & 31, h & 1023);
    let v = match e {
        0 => m as f32 * 2f32.powi(-24),
        31 if m == 0 => f32::INFINITY,
        31 => f32::NAN,
        _ => f32::from_bits(((e + 112) << 23) | (m << 13)),
    };
    if h & 0x8000 != 0 {
        -v
    } else {
        v
    }
}

/// CPU decode of one cluster's positions (lattice + dequantized), its uv0s
/// (`uvs`; word 2 of each [`ATTR_STRIDE`] attribute record, whose stream
/// starts at the position block's word 4) and indices; the mirror of
/// `vg_decode.wgsl` / `vg_alpha_uv.wgsl` / `VgeoReader::decode_cluster`.
fn decode_cluster(
    page: &[u8],
    c: &GpuCluster,
    quant: &GpuAssetQuant,
    uvs: bool,
) -> Result<Decoded, String> {
    let (nv, nt) = (c.vert_count() as usize, c.tri_count() as usize);
    let po = c.vertex_offset as usize;
    let word = |k: usize| -> Result<u32, String> {
        page.get(po + k * 4..po + k * 4 + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| "proxy: position block out of range".to_string())
    };
    let anchor = [word(0)?, word(1)?, word(2)?];
    let packed = word(3)?;
    let bits = [packed & 0xff, (packed >> 8) & 0xff, (packed >> 16) & 0xff];
    let per_vert = (bits[0] + bits[1] + bits[2]) as usize;
    let total_words = (per_vert * nv).div_ceil(32);
    let mut stream = Vec::with_capacity(total_words + 1);
    for i in 0..total_words {
        stream.push(word(POS_BLOCK_HEADER_WORDS + i)?);
    }
    stream.push(0);
    let get = |bitpos: usize, n: u32| -> u32 {
        if n == 0 {
            return 0;
        }
        let (wi, o) = (bitpos / 32, (bitpos % 32) as u32);
        let mut v = stream[wi] >> o;
        if o + n > 32 {
            v |= stream[wi + 1] << (32 - o);
        }
        if n < 32 {
            v &= (1 << n) - 1;
        }
        v
    };
    let mut positions = Vec::with_capacity(nv);
    let mut lattice = Vec::with_capacity(nv);
    let mut bitpos = 0usize;
    for _ in 0..nv {
        let mut q = [0u32; 3];
        for a in 0..3 {
            q[a] = anchor[a] + get(bitpos, bits[a]);
            bitpos += bits[a] as usize;
        }
        lattice.push(q);
        positions.push(dequantize(q, quant));
    }
    let io = c.index_offset as usize;
    let raw = page
        .get(io..io + nt * 3)
        .ok_or_else(|| "proxy: index stream out of range".to_string())?;
    let tris: Vec<[u8; 3]> = raw.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    if tris.iter().flatten().any(|&i| i as usize >= nv) {
        return Err("proxy: triangle index out of range".into());
    }
    let mut uv_bits = Vec::new();
    if uvs {
        let attributes = word(4)? as usize;
        for v in 0..nv {
            let at = attributes + v * ATTR_STRIDE as usize + 8;
            let bytes = page
                .get(at..at + 4)
                .ok_or_else(|| "proxy: attribute stream out of range".to_string())?;
            uv_bits.push(u32::from_le_bytes(bytes.try_into().unwrap()));
        }
    }
    Ok(Decoded {
        positions,
        lattice,
        uv_bits,
        tris,
    })
}

// ── Cache ────────────────────────────────────────────────────────────────────

/// State of one asset in a [`ProxyCache`].
#[derive(Clone)]
pub enum ProxyState {
    /// A worker is building it.
    Building,
    /// Finished; CPU data available until the owner takes it with
    /// [`ProxyCache::take_ready`] (after upload the cache only tracks bytes).
    Ready,
    /// Build failed or the result did not fit the budget.
    Failed(String),
}

struct Entry {
    state: ProxyState,
    bytes: usize,
    last_used: u64,
    pinned_until: u64,
}

/// Counters of a [`ProxyCache`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProxyCacheStats {
    pub jobs_started: u64,
    pub jobs_finished: u64,
    pub jobs_failed: u64,
    pub in_flight: u32,
    pub entries: u32,
    pub bytes: u64,
    pub budget_bytes: u64,
    pub evictions: u64,
    pub rejected_over_budget: u64,
}

/// Background proxy builder with a memory budget. All methods are cheap and
/// non-blocking; the heavy work runs on spawned threads.
pub struct ProxyCache {
    budget_bytes: usize,
    max_jobs: usize,
    options: ProxyOptions,
    entries: HashMap<u64, Entry>,
    queued: Vec<(u64, Arc<dyn VgeoSource>)>,
    tx: mpsc::Sender<(u64, Result<ProxyBuild, String>)>,
    rx: std::sync::Mutex<mpsc::Receiver<(u64, Result<ProxyBuild, String>)>>,
    cancel: Arc<AtomicBool>,
    in_flight: usize,
    ready: Vec<(u64, Arc<ProxyBuild>)>,
    stats: ProxyCacheStats,
}

impl ProxyCache {
    pub fn new(budget_bytes: usize, options: ProxyOptions) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            budget_bytes,
            max_jobs: 2,
            options,
            entries: HashMap::new(),
            queued: Vec::new(),
            tx,
            rx: std::sync::Mutex::new(rx),
            cancel: Arc::new(AtomicBool::new(false)),
            in_flight: 0,
            ready: Vec::new(),
            stats: ProxyCacheStats {
                budget_bytes: budget_bytes as u64,
                ..Default::default()
            },
        }
    }

    pub fn set_budget(&mut self, budget_bytes: usize) {
        self.budget_bytes = budget_bytes;
        self.stats.budget_bytes = budget_bytes as u64;
    }

    pub fn state(&self, key: u64) -> Option<&ProxyState> {
        self.entries.get(&key).map(|e| &e.state)
    }

    /// Ask for `key`'s proxies. A no-op when it is known (building, ready or
    /// failed). Failed entries are not retried until [`Self::forget`].
    pub fn request(&mut self, key: u64, source: Arc<dyn VgeoSource>, frame: u64) {
        if let Some(e) = self.entries.get_mut(&key) {
            e.last_used = frame;
            return;
        }
        self.entries.insert(
            key,
            Entry {
                state: ProxyState::Building,
                bytes: 0,
                last_used: frame,
                pinned_until: 0,
            },
        );
        self.queued.push((key, source));
    }

    /// Mark `key` as used this frame (LRU) and protect it from eviction for
    /// `hold` more frames.
    pub fn touch(&mut self, key: u64, frame: u64, hold: u64) {
        if let Some(e) = self.entries.get_mut(&key) {
            e.last_used = frame;
            e.pinned_until = e.pinned_until.max(frame + hold);
        }
    }

    pub fn forget(&mut self, key: u64) {
        self.entries.remove(&key);
        self.queued.retain(|(k, _)| *k != key);
        self.ready.retain(|(k, _)| *k != key);
    }

    /// Start queued jobs (up to the concurrency limit), collect finished ones
    /// and enforce the budget. Returns the keys evicted this call (the owner
    /// must drop its GPU copies).
    pub fn poll(&mut self, frame: u64) -> Vec<u64> {
        while self.in_flight < self.max_jobs && !self.queued.is_empty() {
            let (key, source) = self.queued.remove(0);
            let tx = self.tx.clone();
            let opts = self.options;
            let cancel = self.cancel.clone();
            self.in_flight += 1;
            self.stats.jobs_started += 1;
            let spawned = std::thread::Builder::new()
                .name("vg-proxy-build".into())
                .spawn(move || {
                    let r = build_proxy(source.as_ref(), &opts, Some(&cancel));
                    let _ = tx.send((key, r));
                });
            if spawned.is_err() {
                self.in_flight -= 1;
                if let Some(e) = self.entries.get_mut(&key) {
                    e.state = ProxyState::Failed("could not spawn a proxy build thread".into());
                }
            }
        }
        loop {
            let received = self.rx.lock().unwrap_or_else(|p| p.into_inner()).try_recv();
            let Ok((key, result)) = received else { break };
            self.in_flight = self.in_flight.saturating_sub(1);
            self.stats.jobs_finished += 1;
            let Some(e) = self.entries.get_mut(&key) else {
                continue;
            };
            match result {
                Ok(build) => {
                    let bytes = build.bytes();
                    if bytes > self.budget_bytes {
                        self.stats.rejected_over_budget += 1;
                        self.stats.jobs_failed += 1;
                        e.state = ProxyState::Failed(format!(
                            "proxy needs {bytes} bytes, over the {} byte budget",
                            self.budget_bytes
                        ));
                    } else {
                        e.bytes = bytes;
                        e.state = ProxyState::Ready;
                        e.last_used = e.last_used.max(frame);
                        self.ready.push((key, Arc::new(build)));
                    }
                }
                Err(message) => {
                    self.stats.jobs_failed += 1;
                    e.state = ProxyState::Failed(message);
                }
            }
        }
        // Evict least recently used, unprotected, finished entries until the
        // accounted bytes fit. Entries still protected stay (the budget is
        // soft for the working set; new builds over budget are rejected above).
        let mut evicted = Vec::new();
        let mut total: usize = self.entries.values().map(|e| e.bytes).sum();
        while total > self.budget_bytes {
            let victim = self
                .entries
                .iter()
                .filter(|(_, e)| {
                    matches!(e.state, ProxyState::Ready) && e.pinned_until < frame && e.bytes > 0
                })
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| *k);
            let Some(k) = victim else { break };
            let e = self.entries.remove(&k).unwrap();
            total -= e.bytes;
            self.ready.retain(|(key, _)| *key != k);
            self.stats.evictions += 1;
            evicted.push(k);
        }
        self.stats.in_flight = self.in_flight as u32;
        self.stats.entries = self.entries.len() as u32;
        self.stats.bytes = total as u64;
        evicted
    }

    /// Finished builds not yet taken (each is returned once).
    pub fn take_ready(&mut self) -> Vec<(u64, Arc<ProxyBuild>)> {
        std::mem::take(&mut self.ready)
    }

    pub fn stats(&self) -> ProxyCacheStats {
        self.stats
    }
}

impl Drop for ProxyCache {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Cache key of an asset: its content id words mixed to one `u64`.
pub fn proxy_key(content: [u64; 2]) -> u64 {
    let mut z = content[0] ^ content[1].rotate_left(29) ^ 0x9E37_79B9_7F4A_7C15;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
