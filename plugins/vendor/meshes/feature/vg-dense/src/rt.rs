//! Tier 3-RT: primary rays over dense tiles against a TLAS of VG instance
//! proxies.
//!
//! See the crate docs for the design and its limits. In short: hardware ray
//! tracing cannot evaluate a per-pixel DAG cut, so every instance is traced
//! through ONE proxy LOD (a uniform cut of its cluster DAG, expanded to plain
//! triangles and built into a BLAS through `helio_core::acceleration`). The
//! LOD is chosen from the instance's nearest distance so its error is at most
//! tau pixels; if even the finest proxy exceeds tau the result is still
//! produced and flagged ([`ProxySelection::within_tau`]).

use crate::common::*;
use crate::config::{self, DenseConfig};
use crate::mode::rt_available;
use bytemuck::{Pod, Zeroable};
use helio_core::acceleration::{
    AccelerationError, BlasGeometry, BlasManager, TlasInstanceInput, TlasManager,
};
use std::collections::HashMap;

/// Vis-id cluster indices at or above this address the RT visible table
/// (`DenseRtPass::rt_visible`), not T5's list. T5 must therefore cap its
/// visible-cluster index below `1 << 24` when 3-RT is enabled.
pub const DENSE_RT_VISIBLE_BASE: u32 = 1 << 24;

#[derive(Debug)]
pub enum RtError {
    /// `rt_available(device)` is false.
    Unsupported,
    InvalidConfig(String),
    Acceleration(AccelerationError),
}
impl std::fmt::Display for RtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "ray queries are not available on this device"),
            Self::InvalidConfig(m) => write!(f, "invalid dense config: {m}"),
            Self::Acceleration(e) => write!(f, "acceleration structure: {e}"),
        }
    }
}
impl std::error::Error for RtError {}
impl From<AccelerationError> for RtError {
    fn from(e: AccelerationError) -> Self {
        Self::Acceleration(e)
    }
}

/// One proxy LOD of an asset: a uniform cut of its cluster DAG, expanded to
/// triangles in asset space.
///
/// `prim_map[p]` for triangle `p` of `indices` is
/// `(proxy_cluster_ordinal << VIS_TRI_BITS) | triangle_in_cluster`, i.e. which
/// cluster of the real DAG the proxy triangle came from and which of that
/// cluster's triangles it is, so a hit resolves to a genuine `(cluster, tri)`
/// vis id and the hardware barycentrics apply to that cluster triangle's
/// vertices. `clusters[ordinal]` is the `cluster_slot` (`pool_slot << 11 | local`)
/// of that proxy cluster in the page pool.
#[derive(Clone, Debug, Default)]
pub struct ProxyLod {
    /// Object-space simplification error of this cut (same units as
    /// `GpuCluster::self_error`), non-decreasing from LOD 0 (finest) up.
    pub error: f32,
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub prim_map: Vec<u32>,
    /// Per proxy cluster: `cluster_slot`, or, when `cluster_pages` is non-empty,
    /// the cluster's local index inside its page.
    pub clusters: Vec<u32>,
    /// Per proxy cluster: asset-local page id. Non-empty means the slots are
    /// resolved every frame through the pool residency (a page that is not
    /// resident makes the instance incomplete); empty means `clusters` already
    /// holds final `cluster_slot`s (tests, fixed pools).
    pub cluster_pages: Vec<u32>,
}

impl ProxyLod {
    fn validate(&self) -> Result<(), String> {
        if self.positions.is_empty() || self.indices.is_empty() || self.indices.len() % 3 != 0 {
            return Err("proxy LOD needs vertices and a multiple of 3 indices".into());
        }
        if self.prim_map.len() != self.indices.len() / 3 {
            return Err("prim_map must have one entry per triangle".into());
        }
        if self
            .indices
            .iter()
            .any(|&i| i as usize >= self.positions.len())
        {
            return Err("index out of range".into());
        }
        let max_ord = self
            .prim_map
            .iter()
            .map(|m| m >> helio_vg_format::VIS_TRI_BITS)
            .max()
            .unwrap_or(0);
        if max_ord as usize >= self.clusters.len() {
            return Err("prim_map references a missing proxy cluster".into());
        }
        if !self.cluster_pages.is_empty() && self.cluster_pages.len() != self.clusters.len() {
            return Err("cluster_pages must have one entry per proxy cluster".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ProxyAsset {
    pub asset_key: u64,
    /// Finest first; `error` non-decreasing.
    pub lods: Vec<ProxyLod>,
}

/// Pixels per world unit at distance 1 from a column-major projection matrix
/// (`proj[5]` is `1 / tan(fov_y / 2)` for a perspective projection).
pub fn pixels_per_unit_from_proj(proj: &[f32; 16], viewport_height: u32) -> f32 {
    viewport_height as f32 * 0.5 * proj[5].abs()
}

/// Pixels per world unit at distance 1 for a vertical field of view.
pub fn pixels_per_unit(viewport_height: u32, fov_y_radians: f32) -> f32 {
    viewport_height as f32 / (2.0 * (fov_y_radians * 0.5).tan())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProxySelection {
    pub lod: usize,
    pub projected_error_px: f32,
    /// False when even the finest proxy exceeds tau at this distance. The
    /// finest is still used; this is the documented accuracy risk of 3-RT.
    pub within_tau: bool,
}

/// Choose the coarsest proxy LOD whose projected error at `near_distance` is
/// at most `tau_px`. `errors` is ascending (LOD 0 finest).
pub fn select_proxy_lod(
    errors: &[f32],
    error_scale: f32,
    near_distance: f32,
    tau_px: f32,
    px_per_unit: f32,
) -> ProxySelection {
    let d = near_distance.max(1.0e-4);
    let proj = |e: f32| e * error_scale * px_per_unit / d;
    for l in (0..errors.len()).rev() {
        let p = proj(errors[l]);
        if p <= tau_px {
            return ProxySelection {
                lod: l,
                projected_error_px: p,
                within_tau: true,
            };
        }
    }
    ProxySelection {
        lod: 0,
        projected_error_px: errors.first().map_or(0.0, |&e| proj(e)),
        within_tau: false,
    }
}

/// BLAS cache key for `(asset, lod)`.
pub fn proxy_mesh_id(asset_key: u64, lod: usize) -> u64 {
    // splitmix-style mix so asset keys that differ in low bits cannot collide
    // with other LODs of a neighbouring key.
    let mut z = asset_key ^ ((lod as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One dense-capable VG instance offered to the RT path for this frame.
#[derive(Clone, Copy, Debug)]
pub struct RtInstanceRequest {
    /// Row of `vg_instances` (the `GpuVisibleCluster.instance` space).
    pub instance_id: u32,
    pub asset_key: u64,
    /// Row-major 3x4 object-to-world (the `TlasInstanceInput` convention).
    pub transform: [f32; 12],
    /// World bounding sphere, for the nearest-distance estimate.
    pub world_center: [f32; 3],
    pub world_radius: f32,
    /// `GpuVgInstance::lod_error_scale` times the instance's uniform scale.
    pub error_scale: f32,
}

/// `RtInstance` in `rt.wgsl` (16 B).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct RtInstanceGpu {
    pub visible_base: u32,
    pub prim_map_offset: u32,
    pub instance_id: u32,
    pub flags: u32,
}

/// RT visible rows are the standard visible-cluster record (`VgVisibleCluster`
/// in WGSL): `{instance, cluster_slot}`, 8 bytes.
pub type DenseVisibleGpu = helio_vg_format::GpuVisibleCluster;

/// What `prepare_frame` produced.
#[derive(Clone, Debug, Default)]
pub struct RtFrameInfo {
    pub tlas_instances: u32,
    /// Rows written to the RT visible table (ids start at
    /// `DENSE_RT_VISIBLE_BASE`).
    pub rt_visible_count: u32,
    /// Requests whose asset has no registered proxy. Their pixels are holes
    /// in dense tiles; callers should fall back to 3-NonRT while nonzero.
    pub missing_proxy_instances: u32,
    /// Instances whose chosen proxy exceeds tau even at the finest LOD.
    pub over_tau_instances: u32,
    pub selections: Vec<(u32, ProxySelection)>,
    pub blas_builds: u32,
    /// Instances left out because a page of their chosen proxy level is not
    /// resident in the pool (cluster slots cannot be resolved).
    pub unresolved_page_instances: u32,
    /// Instances left out because the RT visible table would exceed
    /// `max_visible_rows`.
    pub dropped_instances: u32,
    /// Distinct `(asset key, lod)` selected this frame, including instances
    /// that were left out for non-resident pages.
    pub wanted_lods: Vec<(u64, usize)>,
    /// The TLAS was (re)built this frame (false: instance set, selected
    /// levels and BLASes were identical to the previous frame).
    pub tlas_rebuilt: bool,
    /// The instance / visible tables were rewritten this frame.
    pub tables_rebuilt: bool,
}

impl RtFrameInfo {
    /// Every requested instance is covered.
    pub fn complete(&self) -> bool {
        self.missing_proxy_instances == 0
            && self.unresolved_page_instances == 0
            && self.dropped_instances == 0
    }
}

struct GpuLod {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    vertex_count: u32,
    index_count: u32,
    prim_map_offset: u32,
    prim_map: Vec<u32>,
    /// Distinct asset-local pages of the proxy clusters (empty when slots are final).
    pages: Vec<u32>,
    error: f32,
    last_used: u64,
}

struct GpuAsset {
    lods: Vec<GpuLod>,
    /// Per LOD: `cluster_slot`s, or local indices when `cluster_pages` is set.
    clusters: Vec<Vec<u32>>,
    cluster_pages: Vec<Vec<u32>>,
    gpu_bytes: u64,
}

/// Growable storage buffer.
struct Grow {
    buf: wgpu::Buffer,
    cap: u64,
}
impl Grow {
    fn new(device: &wgpu::Device, label: &str) -> Self {
        Self {
            buf: Self::alloc(device, label, 64),
            cap: 64,
        }
    }
    fn alloc(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }
    /// Replace the contents with `bytes`. Returns true when the buffer was
    /// reallocated (bind groups must be rebuilt; they are, every frame).
    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, bytes: &[u8]) {
        let need = (bytes.len() as u64).max(16).next_multiple_of(4);
        if need > self.cap {
            self.cap = need.next_power_of_two();
            self.buf = Self::alloc(device, label, self.cap);
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buf, 0, bytes);
        }
    }
}

pub struct DenseRtPass {
    cfg: DenseConfig,
    compact: wgpu::ComputePipeline,
    args: wgpu::ComputePipeline,
    trace: wgpu::ComputePipeline,
    compact_bgl: wgpu::BindGroupLayout,
    args_bgl: wgpu::BindGroupLayout,
    trace_bgl: wgpu::BindGroupLayout,
    buffers: Option<DenseBuffers>,
    dense_bary: Option<wgpu::Buffer>,
    assets: HashMap<u64, GpuAsset>,
    prim_map_pool: Vec<u32>,
    prim_map_dirty: bool,
    prim_map_buf: Grow,
    instances_buf: Grow,
    rt_visible_buf: Grow,
    frame_counter: u64,
    rt_instance_count: u32,
    rt_visible_rows: u32,
    max_visible_rows: u32,
    prim_map_garbage: usize,
    /// Bumped whenever the proxy set or the prim_map pool layout changes.
    pool_epoch: u64,
    last_tlas: Option<(Vec<TlasInstanceInput>, u64)>,
    last_table_sig: Option<u64>,
}

impl DenseRtPass {
    pub fn new(device: &wgpu::Device, cfg: DenseConfig) -> Result<Self, RtError> {
        if !rt_available(device) {
            return Err(RtError::Unsupported);
        }
        cfg.validate().map_err(RtError::InvalidConfig)?;
        let compact_bgl = bgl(
            device,
            "vg_dense_rt compact",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(1, storage(true)),
                bgl_entry(2, storage(false)),
                bgl_entry(3, storage(false)),
            ],
        );
        let args_bgl = bgl(
            device,
            "vg_dense_rt args",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(1, storage(false)),
                bgl_entry(3, storage(false)),
            ],
        );
        let trace_bgl = bgl(
            device,
            "vg_dense_rt trace",
            &[
                bgl_entry(0, uniform()),
                bgl_entry(
                    1,
                    wgpu::BindingType::AccelerationStructure {
                        vertex_return: false,
                    },
                ),
                bgl_entry(2, storage(true)),
                bgl_entry(3, storage(true)),
                bgl_entry(8, storage(false)),
                bgl_entry(9, storage(false)),
                bgl_entry(10, storage(false)),
            ],
        );
        let tiles = config::tiles_source(&cfg);
        let args = config::args_source(&cfg);
        let rt = config::rt_source(&cfg);
        Ok(Self {
            compact: pipeline(
                device,
                "vg_dense_rt compact",
                &tiles,
                "compact_tiles_only",
                &compact_bgl,
            ),
            args: pipeline(device, "vg_dense_rt args", &args, "make_args_rt", &args_bgl),
            trace: pipeline(device, "vg_dense_rt trace", &rt, "trace", &trace_bgl),
            compact_bgl,
            args_bgl,
            trace_bgl,
            cfg,
            buffers: None,
            dense_bary: None,
            assets: HashMap::new(),
            prim_map_pool: Vec::new(),
            prim_map_dirty: false,
            prim_map_buf: Grow::new(device, "vg_dense_rt prim_map"),
            instances_buf: Grow::new(device, "vg_dense_rt instances"),
            rt_visible_buf: Grow::new(device, "vg_dense_rt visible"),
            frame_counter: 0,
            rt_instance_count: 0,
            rt_visible_rows: 0,
            max_visible_rows: 1 << 22,
            prim_map_garbage: 0,
            pool_epoch: 0,
            last_tlas: None,
            last_table_sig: None,
        })
    }

    pub fn config(&self) -> &DenseConfig {
        &self.cfg
    }

    /// Cap on rows of the RT visible table (ids are `1 << 24 + row`, so it must
    /// stay below `1 << 24`).
    pub fn set_max_visible_rows(&mut self, rows: u32) {
        self.max_visible_rows = rows.min(DENSE_RT_VISIBLE_BASE - 1);
    }

    /// Output buffers (`dense_key`, stats) once a frame has been recorded.
    pub fn buffers(&self) -> Option<&DenseBuffers> {
        self.buffers.as_ref()
    }

    /// Hardware barycentrics per pixel, packed like `bary_pack`.
    pub fn dense_bary(&self) -> Option<&wgpu::Buffer> {
        self.dense_bary.as_ref()
    }

    /// The RT visible table (`DenseVisibleGpu` rows). Vis-id cluster index
    /// `DENSE_RT_VISIBLE_BASE + i` addresses row `i`.
    pub fn rt_visible(&self) -> &wgpu::Buffer {
        &self.rt_visible_buf.buf
    }

    /// Whether proxies are registered for `asset_key` (the `rt_ready` input of
    /// the `Auto` policy is "all dense-capable assets have proxies").
    pub fn has_proxy(&self, asset_key: u64) -> bool {
        self.assets.contains_key(&asset_key)
    }

    /// Upload an asset's proxy LODs (built by `helio_vg_stream::proxy`).
    /// Replaces any previous registration of `asset_key`. Returns the GPU bytes
    /// the asset holds (vertex + index + prim_map).
    pub fn register_proxy_asset(
        &mut self,
        device: &wgpu::Device,
        asset: &ProxyAsset,
    ) -> Result<u64, String> {
        for lod in &asset.lods {
            lod.validate()?;
        }
        let mut last_err = 0.0f32;
        for lod in &asset.lods {
            if lod.error < last_err {
                return Err("proxy LOD errors must be non-decreasing".into());
            }
            last_err = lod.error;
        }
        self.drop_asset(asset.asset_key);
        let mut lods = Vec::new();
        let mut clusters = Vec::new();
        let mut cluster_pages = Vec::new();
        let mut gpu_bytes = 0u64;
        for lod in &asset.lods {
            let usage = wgpu::BufferUsages::BLAS_INPUT
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::STORAGE;
            let vertices = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vg_dense_rt proxy vertices"),
                size: (lod.positions.len() * 12) as u64,
                usage,
                mapped_at_creation: true,
            });
            vertices
                .slice(..)
                .get_mapped_range_mut()
                .unwrap()
                .copy_from_slice(bytemuck::cast_slice(&lod.positions));
            vertices.unmap();
            let indices = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vg_dense_rt proxy indices"),
                size: (lod.indices.len() * 4) as u64,
                usage,
                mapped_at_creation: true,
            });
            indices
                .slice(..)
                .get_mapped_range_mut()
                .unwrap()
                .copy_from_slice(bytemuck::cast_slice(&lod.indices));
            indices.unmap();
            let prim_map_offset = self.prim_map_pool.len() as u32;
            self.prim_map_pool.extend_from_slice(&lod.prim_map);
            let mut pages = lod.cluster_pages.clone();
            pages.sort_unstable();
            pages.dedup();
            gpu_bytes +=
                (lod.positions.len() * 12 + lod.indices.len() * 4 + lod.prim_map.len() * 4) as u64;
            lods.push(GpuLod {
                vertices,
                indices,
                vertex_count: lod.positions.len() as u32,
                index_count: lod.indices.len() as u32,
                prim_map_offset,
                prim_map: lod.prim_map.clone(),
                pages,
                error: lod.error,
                last_used: 0,
            });
            clusters.push(lod.clusters.clone());
            cluster_pages.push(lod.cluster_pages.clone());
        }
        self.prim_map_dirty = true;
        self.pool_epoch += 1;
        self.assets.insert(
            asset.asset_key,
            GpuAsset {
                lods,
                clusters,
                cluster_pages,
                gpu_bytes,
            },
        );
        Ok(gpu_bytes)
    }

    fn drop_asset(&mut self, asset_key: u64) -> Option<GpuAsset> {
        let removed = self.assets.remove(&asset_key)?;
        self.prim_map_garbage += removed.lods.iter().map(|l| l.prim_map.len()).sum::<usize>();
        // Compact the shared prim_map pool once half of it is dead.
        if self.prim_map_garbage * 2 > self.prim_map_pool.len() {
            let mut pool = Vec::with_capacity(self.prim_map_pool.len() - self.prim_map_garbage);
            for asset in self.assets.values_mut() {
                for lod in &mut asset.lods {
                    lod.prim_map_offset = pool.len() as u32;
                    pool.extend_from_slice(&lod.prim_map);
                }
            }
            self.prim_map_pool = pool;
            self.prim_map_garbage = 0;
            self.prim_map_dirty = true;
        }
        self.pool_epoch += 1;
        Some(removed)
    }

    /// Forget an asset's proxies and drop its BLASes. Returns the GPU bytes
    /// released.
    pub fn unregister_proxy_asset(&mut self, blas: &mut BlasManager, asset_key: u64) -> u64 {
        let Some(removed) = self.drop_asset(asset_key) else {
            return 0;
        };
        for l in 0..removed.lods.len() {
            blas.remove_blas(proxy_mesh_id(asset_key, l));
        }
        removed.gpu_bytes
    }

    /// Registered assets and the GPU bytes they hold.
    pub fn proxy_stats(&self) -> (u32, u64) {
        (
            self.assets.len() as u32,
            self.assets.values().map(|a| a.gpu_bytes).sum(),
        )
    }

    /// Select proxy LODs, build any missing BLASes, and (re)build the TLAS and
    /// the per-instance tables for this frame. Record before
    /// [`DenseRtPass::record`] in the same (or an earlier) submission.
    ///
    /// Proxies whose clusters carry a page list ([`ProxyLod::cluster_pages`])
    /// need [`DenseRtPass::prepare_frame_resolved`]; with this entry point they
    /// count as unresolved.
    pub fn prepare_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        blas: &mut BlasManager,
        tlas: &mut TlasManager,
        frame: &DenseFrame,
        requests: &[RtInstanceRequest],
        tau_px: f32,
        px_per_unit: f32,
    ) -> Result<RtFrameInfo, RtError> {
        self.prepare_frame_resolved(
            device,
            queue,
            encoder,
            blas,
            tlas,
            frame,
            requests,
            tau_px,
            px_per_unit,
            0,
            &|_, _| None,
        )
    }

    /// As [`Self::prepare_frame`], resolving page-addressed proxy clusters to
    /// `cluster_slot`s: `resolve_page(asset_key, page)` is the page's pool slot
    /// (`None` = not resident, the instance is left out and counted in
    /// [`RtFrameInfo::unresolved_page_instances`]). `residency_version` must
    /// change whenever any answer of `resolve_page` may have (the store's
    /// `table_version`); while it, the selected levels and the instance set are
    /// unchanged the tables and TLAS are not rebuilt.
    ///
    /// CPU cost: O(requests) per frame; O(sum of proxy clusters of the
    /// requested instances) only when the set, a level or the residency changed.
    pub fn prepare_frame_resolved(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        blas: &mut BlasManager,
        tlas: &mut TlasManager,
        frame: &DenseFrame,
        requests: &[RtInstanceRequest],
        tau_px: f32,
        px_per_unit: f32,
        residency_version: u64,
        resolve_page: &dyn Fn(u64, u32) -> Option<u32>,
    ) -> Result<RtFrameInfo, RtError> {
        self.frame_counter += 1;
        let frame_no = self.frame_counter;
        let mut info = RtFrameInfo::default();
        // (request index, selected lod, within tau) of every traceable instance.
        let mut chosen: Vec<(usize, usize, bool)> = Vec::with_capacity(requests.len());
        let mut resolvable: HashMap<(u64, usize), bool> = HashMap::new();
        let mut errors: Vec<f32> = Vec::new();
        let mut rows_used = 0u32;
        for (ri, r) in requests.iter().enumerate() {
            let Some(asset) = self.assets.get_mut(&r.asset_key) else {
                info.missing_proxy_instances += 1;
                continue;
            };
            let cam = frame.camera_pos;
            let d = ((r.world_center[0] - cam[0]).powi(2)
                + (r.world_center[1] - cam[1]).powi(2)
                + (r.world_center[2] - cam[2]).powi(2))
            .sqrt();
            // Nearest possible distance of any point of the instance: never
            // farther than the tile's nearest distance, so at least as fine.
            let near = (d - r.world_radius).max(frame.ray_t[0]);
            errors.clear();
            errors.extend(asset.lods.iter().map(|l| l.error));
            let sel = select_proxy_lod(&errors, r.error_scale, near, tau_px, px_per_unit);
            asset.lods[sel.lod].last_used = frame_no;
            if !info.wanted_lods.contains(&(r.asset_key, sel.lod)) {
                info.wanted_lods.push((r.asset_key, sel.lod));
            }
            if !asset.cluster_pages[sel.lod].is_empty() {
                let lod = &asset.lods[sel.lod];
                let ok = *resolvable.entry((r.asset_key, sel.lod)).or_insert_with(|| {
                    lod.pages
                        .iter()
                        .all(|&p| resolve_page(r.asset_key, p).is_some())
                });
                if !ok {
                    info.unresolved_page_instances += 1;
                    continue;
                }
            }
            let cluster_rows = asset.clusters[sel.lod].len() as u32;
            if rows_used + cluster_rows > self.max_visible_rows {
                info.dropped_instances += 1;
                continue;
            }
            rows_used += cluster_rows;
            if !sel.within_tau {
                info.over_tau_instances += 1;
            }
            let lod = &asset.lods[sel.lod];
            if blas.build_from_buffers(
                proxy_mesh_id(r.asset_key, sel.lod),
                encoder,
                BlasGeometry {
                    revision: 0,
                    vertices: &lod.vertices,
                    first_vertex: 0,
                    vertex_count: lod.vertex_count,
                    vertex_stride: 12,
                    indices: Some(&lod.indices),
                    first_index: 0,
                    index_count: lod.index_count,
                },
            )? {
                info.blas_builds += 1;
            }
            info.selections.push((r.instance_id, sel));
            chosen.push((ri, sel.lod, sel.within_tau));
        }

        let tlas_inputs: Vec<TlasInstanceInput> = chosen
            .iter()
            .map(|&(ri, l, _)| TlasInstanceInput {
                mesh_id: proxy_mesh_id(requests[ri].asset_key, l),
                transform: requests[ri].transform,
            })
            .collect();
        info.tlas_instances = tlas_inputs.len() as u32;
        if tlas_inputs.is_empty() {
            self.rt_instance_count = 0;
            self.last_tlas = None;
            return Ok(info);
        }
        let tlas_same = self
            .last_tlas
            .as_ref()
            .is_some_and(|(inputs, rev)| *rev == blas.revision() && *inputs == tlas_inputs)
            && tlas.tlas().is_some();
        tlas.build(encoder, &tlas_inputs, blas)?;
        info.tlas_rebuilt = !tlas_same;
        self.last_tlas = Some((tlas_inputs, blas.revision()));

        // Table signature: which instances, which level, which asset, under
        // which residency/pool layout.
        let mut sig: u64 =
            0xcbf2_9ce4_8422_2325 ^ residency_version.rotate_left(17) ^ self.pool_epoch;
        let mut mix = |v: u64| {
            sig = (sig ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        };
        for &(ri, l, within) in &chosen {
            let r = &requests[ri];
            mix(r.instance_id as u64);
            mix(r.asset_key);
            mix(l as u64 | ((within as u64) << 32));
        }
        if self.last_table_sig != Some(sig) {
            let mut rows = Vec::with_capacity(chosen.len());
            let mut visible: Vec<DenseVisibleGpu> = Vec::new();
            let mut slot_memo: HashMap<(u64, u32), u32> = HashMap::new();
            for &(ri, l, within) in &chosen {
                let r = &requests[ri];
                let asset = &self.assets[&r.asset_key];
                let lod = &asset.lods[l];
                let visible_base = DENSE_RT_VISIBLE_BASE + visible.len() as u32;
                let locals = &asset.clusters[l];
                let pages = &asset.cluster_pages[l];
                for (c, &cl) in locals.iter().enumerate() {
                    let cluster_slot = if pages.is_empty() {
                        cl
                    } else {
                        let page = pages[c];
                        let pool = *slot_memo
                            .entry((r.asset_key, page))
                            .or_insert_with(|| resolve_page(r.asset_key, page).unwrap_or(0));
                        (pool << helio_vg_format::PAGE_CLUSTER_SHIFT) | cl
                    };
                    visible.push(DenseVisibleGpu {
                        instance: r.instance_id,
                        cluster_slot,
                    });
                }
                rows.push(RtInstanceGpu {
                    visible_base,
                    prim_map_offset: lod.prim_map_offset,
                    instance_id: r.instance_id,
                    flags: within as u32,
                });
            }
            info.rt_visible_count = visible.len() as u32;
            self.rt_visible_rows = visible.len() as u32;
            if self.prim_map_dirty {
                let bytes = bytemuck::cast_slice(&self.prim_map_pool).to_vec();
                self.prim_map_buf
                    .write(device, queue, "vg_dense_rt prim_map", &bytes);
                self.prim_map_dirty = false;
            }
            self.instances_buf.write(
                device,
                queue,
                "vg_dense_rt instances",
                bytemuck::cast_slice(&rows),
            );
            self.rt_visible_buf.write(
                device,
                queue,
                "vg_dense_rt visible",
                bytemuck::cast_slice(&visible),
            );
            self.rt_instance_count = rows.len() as u32;
            self.last_table_sig = Some(sig);
            info.tables_rebuilt = true;
        } else {
            info.rt_visible_count = self.rt_visible_rows;
        }
        Ok(info)
    }

    /// Drop BLASes of proxy LODs unused for `max_age` frames.
    pub fn evict_unused(&mut self, blas: &mut BlasManager, max_age: u64) {
        let now = self.frame_counter;
        let assets = &self.assets;
        blas.retain(|id| {
            // Ids that are not ours are kept: only proxy ids that are stale go.
            for (key, asset) in assets {
                for (l, lod) in asset.lods.iter().enumerate() {
                    if proxy_mesh_id(*key, l) == id {
                        return now.saturating_sub(lod.last_used) <= max_age;
                    }
                }
            }
            true
        });
    }

    /// Record compaction, args, and the ray trace over dense tiles.
    /// `tile_mask` is T7's dense mask (one `u32` per tile, nonzero = dense).
    pub fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &DenseFrame,
        tile_mask: &wgpu::Buffer,
        tlas: &TlasManager,
    ) -> Result<(), RtError> {
        let mut rec = helio_core::CommandRecorder::from_encoder(encoder);
        self.record_rec(device, queue, &mut rec, frame, tile_mask, tlas)
    }

    /// As [`Self::record`], into a render-graph command stream.
    pub fn record_rec(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        tile_mask: &wgpu::Buffer,
        tlas: &TlasManager,
    ) -> Result<(), RtError> {
        self.record_scoped(
            device,
            queue,
            encoder,
            frame,
            tile_mask,
            tlas,
            &mut GpuScopes::none(),
        )
    }

    /// [`Self::record_rec`] with per-kernel GPU timestamp scopes.
    #[allow(clippy::too_many_arguments)]
    pub fn record_scoped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        tile_mask: &wgpu::Buffer,
        tlas: &TlasManager,
        scopes: &mut GpuScopes<'_>,
    ) -> Result<(), RtError> {
        let Some(tlas_res) = tlas.as_binding() else {
            return Err(RtError::Acceleration(AccelerationError::Unsupported));
        };
        if !self
            .buffers
            .as_ref()
            .is_some_and(|b| b.matches(&self.cfg, frame.viewport))
        {
            let b = DenseBuffers::new(device, &self.cfg, frame.viewport, false);
            self.dense_bary = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vg_dense_rt bary"),
                size: (frame.viewport[0] as u64 * frame.viewport[1] as u64 * 4).max(16),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.buffers = Some(b);
        }
        let b = self.buffers.as_ref().unwrap();
        b.write_params(queue, frame);
        encoder.clear_buffer(&b.state, 0, Some(STATE_HEADER_WORDS as u64 * 4));
        // See `DenseNonRtPass`: stale keys outside dense tiles would ghost.
        encoder.clear_buffer(&b.dense_key, 0, None);
        if let Some(bary) = &self.dense_bary {
            encoder.clear_buffer(bary, 0, None);
        }

        let compact_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense_rt compact"),
            layout: &self.compact_bgl,
            entries: &[
                bg_entry(0, &b.params),
                bg_entry(1, tile_mask),
                bg_entry(2, &b.tile_slot),
                bg_entry(3, &b.state),
            ],
        });
        let args_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense_rt args"),
            layout: &self.args_bgl,
            entries: &[
                bg_entry(0, &b.params),
                bg_entry(1, &b.state),
                bg_entry(3, &b.args),
            ],
        });
        let trace_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vg_dense_rt trace"),
            layout: &self.trace_bgl,
            entries: &[
                bg_entry(0, &b.params),
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: tlas_res,
                },
                bg_entry(2, &self.instances_buf.buf),
                bg_entry(3, &self.prim_map_buf.buf),
                bg_entry(8, &b.state),
                bg_entry(9, &b.dense_key),
                bg_entry(10, self.dense_bary.as_ref().unwrap()),
            ],
        });
        let mut run = |label: &'static str, f: &dyn Fn(&mut helio_core::ComputeCmds<'_>)| {
            scopes.begin(encoder, label);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(label),
                    timestamp_writes: None,
                });
                f(&mut pass);
            }
            scopes.end(encoder, label);
        };
        run("vg_dense_rt compact", &|p| {
            p.set_pipeline(&self.compact);
            p.set_bind_group(0, &compact_bg, &[]);
            p.dispatch_workgroups(b.tile_count().div_ceil(64), 1, 1);
        });
        run("vg_dense_rt args", &|p| {
            p.set_pipeline(&self.args);
            p.set_bind_group(0, &args_bg, &[]);
            p.dispatch_workgroups(1, 1, 1);
        });
        run("vg_dense_rt trace", &|p| {
            p.set_pipeline(&self.trace);
            p.set_bind_group(0, &trace_bg, &[]);
            p.dispatch_workgroups_indirect(&b.args, 0);
        });
        Ok(())
    }

    /// Distinct `(asset key, lod)` the last [`Self::prepare_frame_resolved`] selected
    /// (including instances left out for non-resident pages), so the caller can
    /// request exactly those pages.
    pub fn lod_pages(&self, asset_key: u64, lod: usize) -> &[u32] {
        self.assets
            .get(&asset_key)
            .and_then(|a| a.lods.get(lod))
            .map_or(&[], |l| l.pages.as_slice())
    }

    pub fn tlas_instance_count(&self) -> u32 {
        self.rt_instance_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_selection_picks_coarsest_within_tau() {
        let errors = [0.0, 0.01, 0.04, 0.16, 0.64];
        let ppu = pixels_per_unit(1080, 1.0);
        // Far away: a coarse proxy is within 1 px.
        let far = select_proxy_lod(&errors, 1.0, 1000.0, 1.0, ppu);
        assert!(far.within_tau && far.lod == 4, "{far:?}");
        // Close: only finer ones qualify, and the choice is monotone in distance.
        let mut last = usize::MAX;
        for d in [400.0, 100.0, 25.0, 6.0, 1.5] {
            let s = select_proxy_lod(&errors, 1.0, d, 1.0, ppu);
            assert!(s.lod <= last);
            if s.within_tau {
                assert!(s.projected_error_px <= 1.0);
                if s.lod + 1 < errors.len() {
                    let coarser = errors[s.lod + 1] * ppu / d;
                    assert!(
                        coarser > 1.0,
                        "a coarser LOD would still have been within tau"
                    );
                }
            }
            last = s.lod;
        }
    }

    #[test]
    fn proxy_selection_flags_unsatisfiable_tau() {
        let errors = [0.5, 1.0];
        let s = select_proxy_lod(&errors, 1.0, 0.5, 1.0, pixels_per_unit(1080, 1.0));
        assert_eq!(s.lod, 0);
        assert!(!s.within_tau && s.projected_error_px > 1.0);
    }

    #[test]
    fn error_scale_and_tau_shift_the_choice() {
        let errors = [0.0, 0.1, 0.4];
        let ppu = pixels_per_unit(1080, 1.0);
        let base = select_proxy_lod(&errors, 1.0, 100.0, 1.0, ppu).lod;
        assert!(select_proxy_lod(&errors, 4.0, 100.0, 1.0, ppu).lod <= base);
        assert!(select_proxy_lod(&errors, 1.0, 100.0, 8.0, ppu).lod >= base);
    }

    #[test]
    fn mesh_ids_do_not_collide_across_lods() {
        let mut seen = std::collections::HashSet::new();
        for k in 0..2000u64 {
            for l in 0..6 {
                assert!(seen.insert(proxy_mesh_id(k, l)));
            }
        }
    }

    #[test]
    fn proxy_lod_validation() {
        let ok = ProxyLod {
            error: 0.0,
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            prim_map: vec![0],
            clusters: vec![0],
            cluster_pages: vec![],
        };
        assert!(ok.validate().is_ok());
        let mut bad = ok.clone();
        bad.prim_map = vec![1 << helio_vg_format::VIS_TRI_BITS];
        assert!(bad.validate().is_err());
        let mut bad = ok;
        bad.indices = vec![0, 1, 9];
        assert!(bad.validate().is_err());
    }

    #[test]
    fn gpu_row_layouts() {
        assert_eq!(std::mem::size_of::<RtInstanceGpu>(), 16);
        assert_eq!(std::mem::size_of::<DenseVisibleGpu>(), 8);
        assert_eq!(std::mem::size_of::<DenseParamsGpu>(), 192);
    }
}
