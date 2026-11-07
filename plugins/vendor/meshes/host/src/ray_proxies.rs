//! Virtual-geometry instances in the scene's TLAS ([`super::ray_tracing`]),
//! through one proxy per asset.
//!
//! A VG instance is an ordinary static object whose mesh's interned vertex
//! range is a VG asset's identity (`VgRuntime::request_asset`); its classic
//! mesh is a fallback or a zero-area placeholder, never the shape rays
//! should see, and the asset's full geometry is far too big for a BLAS. So
//! each asset is traced through ONE coarse cut of its cluster DAG
//! ([`ProxyOptions::trace`]: at most 1 % of its bounding radius off the
//! surface, at most 32 Ki triangles), built on worker threads from the
//! asset's bytes ([`ProxyCache`]) the first time an object uses it, shared
//! by all its instances. Ray hits on it are shaded as the object's material
//! (`RayInstance` rows flagged [`helio_mesh_api::RAY_INSTANCE_PROXY`], the
//! face's winding as its normal); its vertices keep their uv0, so an
//! alpha-tested material is tested on it as on a classic mesh.
//!
//! The proxies live in one pool of packed vertices (position, then uv0) and
//! indices (the proxy pool a frame publishes, `helio_mesh_api::RAY_PROXY_VERTICES`), used directly as BLAS input
//! and by shaders that read hit triangles. It only grows by appending; when full it
//! is reallocated at twice the live size and the live proxies are copied in,
//! packed (their BLASes rebuild: their input moved). An asset no object uses
//! any more is dropped at the next build.
//!
//! Until an asset's proxy is built its instances are left out (rays pass
//! them), as any object the projection cannot represent yet.

use helio_vg_stream::{proxy_key, ProxyBuild, ProxyCache, ProxyOptions, ProxyState, VgRuntime};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Bytes of a pool vertex: a `Float32x3` position, then a `Float32x2` uv0.
const VERTEX_BYTES: u64 = helio_mesh_api::RAY_PROXY_VERTEX_BYTES;
/// The cache's accounting budget for proxies not yet uploaded. Every proxy
/// an object uses stays (live ones are pinned); this only bounds a single
/// build ([`ProxyOptions::TRACE_MAX_TRIANGLES`] keeps it far below).
const CACHE_BUDGET: usize = 512 << 20;

/// Where one asset's proxy is in the pool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProxyRange {
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub first_index: u32,
    pub index_count: u32,
}

/// What a mesh is to the projection.
pub(crate) enum Lookup {
    /// Not a virtual-geometry asset: traced as its classic mesh.
    Classic,
    /// A VG asset whose proxy is not built (or failed): left out.
    Pending,
    /// Traced through its proxy, by key.
    Proxy(u64, ProxyRange),
}

/// Counters of the projection's virtual geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VgRayStats {
    /// Assets with a proxy in the pool, and their triangles.
    pub proxies: u32,
    pub triangles: u64,
    /// Triangles of those assets' original geometry.
    pub source_triangles: u64,
    /// Proxy builds running or queued, and those that failed.
    pub pending: u32,
    pub failed: u32,
    /// Live bytes of vertices and indices, and the pool's allocated bytes.
    pub live_bytes: u64,
    pub pool_bytes: u64,
}

struct Pool {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    vertex_capacity: u32,
    index_capacity: u32,
    vertex_end: u32,
    index_end: u32,
}

impl Pool {
    fn new(device: &wgpu::Device, vertex_capacity: u32, index_capacity: u32) -> Self {
        let buffer = |label, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.max(16),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::BLAS_INPUT
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        Self {
            vertices: buffer(
                "VG ray proxy vertices",
                u64::from(vertex_capacity) * VERTEX_BYTES,
            ),
            indices: buffer("VG ray proxy indices", u64::from(index_capacity) * 4),
            vertex_capacity,
            index_capacity,
            vertex_end: 0,
            index_end: 0,
        }
    }

    fn bytes(&self) -> u64 {
        self.vertices.size() + self.indices.size()
    }
}

struct Proxy {
    range: ProxyRange,
    source_triangles: u32,
}

/// The scene projection's VG proxies (module docs).
pub struct VgRayProxies {
    runtime: VgRuntime,
    cache: ProxyCache,
    /// Runtime assets by interned vertex range: their proxy key and bytes.
    assets: HashMap<(u32, u32), (u64, Arc<dyn helio_vg_stream::VgeoSource>)>,
    assets_version: Option<u64>,
    proxies: HashMap<u64, Proxy>,
    /// Keys the last build used: kept pinned in the cache.
    live: Vec<u64>,
    pool: Option<Pool>,
    compact_pool: bool,
    frame: u64,
}

impl VgRayProxies {
    pub fn new(runtime: VgRuntime) -> Self {
        Self {
            runtime,
            cache: ProxyCache::new(CACHE_BUDGET, ProxyOptions::trace()),
            assets: HashMap::new(),
            assets_version: None,
            proxies: HashMap::new(),
            live: Vec::new(),
            pool: None,
            compact_pool: false,
            frame: 0,
        }
    }

    /// Per frame, before the projection builds: follows the runtime's
    /// assets, collects finished proxy builds and uploads them. True when
    /// the projection must rebuild (an asset or a proxy came or went).
    pub(crate) fn poll(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        self.frame += 1;
        let mut changed = false;
        let version = self.runtime.assets_version();
        if self.assets_version != Some(version) {
            self.assets_version = Some(version);
            self.assets = self
                .runtime
                .assets()
                .into_iter()
                .map(|a| {
                    (
                        (a.vertex_offset, a.vertex_count),
                        (proxy_key(a.content), a.source),
                    )
                })
                .collect();
            changed = true;
        }
        for &key in &self.live {
            self.cache.touch(key, self.frame, 2);
        }
        for key in self.cache.poll(self.frame) {
            changed |= self.proxies.remove(&key).is_some();
        }
        let ready = self.cache.take_ready();
        if !ready.is_empty() {
            self.upload(device, queue, ready);
            changed = true;
        }
        if self.compact_pool && self.pool_is_sparse() {
            self.grow(device, queue, 0, 0);
            changed = true;
        }
        self.compact_pool = false;
        changed
    }

    /// What the mesh with interned vertex range `range` is to the
    /// projection; a VG asset met for the first time starts its build.
    pub(crate) fn lookup(&mut self, range: (u32, u32)) -> Lookup {
        let Some((key, source)) = self.assets.get(&range) else {
            return Lookup::Classic;
        };
        if let Some(proxy) = self.proxies.get(key) {
            return Lookup::Proxy(*key, proxy.range);
        }
        if self.cache.state(*key).is_none() {
            self.cache.request(*key, source.clone(), self.frame);
        }
        Lookup::Pending
    }

    /// After a build: the proxies its objects use. The others are dropped.
    pub(crate) fn retain(&mut self, used: &HashSet<u64>) {
        self.live = used.iter().copied().collect();
        let unused: Vec<u64> = self
            .proxies
            .keys()
            .filter(|k| !used.contains(k))
            .copied()
            .collect();
        let had_unused = !unused.is_empty();
        for key in unused {
            self.proxies.remove(&key);
            self.cache.forget(key);
        }
        self.compact_pool |= had_unused;
    }

    fn pool_is_sparse(&self) -> bool {
        let Some(pool) = &self.pool else {
            return false;
        };
        let live_vertices: u32 = self.proxies.values().map(|p| p.range.vertex_count).sum();
        let live_indices: u32 = self.proxies.values().map(|p| p.range.index_count).sum();
        let target_vertices = live_vertices
            .max(1024)
            .next_power_of_two()
            .saturating_mul(2);
        let target_indices = live_indices.max(4096).next_power_of_two().saturating_mul(2);
        pool.vertex_capacity > target_vertices.saturating_mul(4)
            || pool.index_capacity > target_indices.saturating_mul(4)
    }

    /// The pool's vertices and indices (after the first upload).
    pub fn buffers(&self) -> Option<(&wgpu::Buffer, &wgpu::Buffer)> {
        self.pool.as_ref().map(|p| (&p.vertices, &p.indices))
    }

    pub fn stats(&self) -> VgRayStats {
        let mut stats = VgRayStats {
            proxies: self.proxies.len() as u32,
            ..Default::default()
        };
        for proxy in self.proxies.values() {
            stats.triangles += u64::from(proxy.range.index_count / 3);
            stats.source_triangles += u64::from(proxy.source_triangles);
            stats.live_bytes += u64::from(proxy.range.vertex_count) * VERTEX_BYTES
                + u64::from(proxy.range.index_count) * 4;
        }
        for (key, _) in self.assets.values() {
            match self.cache.state(*key) {
                Some(ProxyState::Building) => stats.pending += 1,
                Some(ProxyState::Failed(_)) => stats.failed += 1,
                _ => {}
            }
        }
        stats.pool_bytes = self.pool.as_ref().map_or(0, Pool::bytes);
        stats
    }

    /// Appends finished builds to the pool, growing (and packing) it when
    /// they do not fit.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ready: Vec<(u64, Arc<ProxyBuild>)>,
    ) {
        let builds: Vec<(u64, Arc<ProxyBuild>)> = ready
            .into_iter()
            .filter(|(_, b)| !b.lods.is_empty())
            .collect();
        let (new_vertices, new_indices) = builds.iter().fold((0u32, 0u32), |(v, i), (_, b)| {
            (
                v + b.lods[0].positions.len() as u32,
                i + b.lods[0].indices.len() as u32,
            )
        });
        let fits = self.pool.as_ref().is_some_and(|p| {
            p.vertex_end + new_vertices <= p.vertex_capacity
                && p.index_end + new_indices <= p.index_capacity
        });
        if !fits {
            self.grow(device, queue, new_vertices, new_indices);
        }
        let pool = self.pool.as_mut().expect("pool allocated above");
        for (key, build) in builds {
            let lod = &build.lods[0];
            let range = ProxyRange {
                first_vertex: pool.vertex_end,
                vertex_count: lod.positions.len() as u32,
                first_index: pool.index_end,
                index_count: lod.indices.len() as u32,
            };
            let vertices: Vec<[f32; 5]> = lod
                .positions
                .iter()
                .enumerate()
                .map(|(v, p)| {
                    let uv = lod.uvs.get(v).copied().unwrap_or([0.0; 2]);
                    [p[0], p[1], p[2], uv[0], uv[1]]
                })
                .collect();
            queue.write_buffer(
                &pool.vertices,
                u64::from(range.first_vertex) * VERTEX_BYTES,
                bytemuck::cast_slice(&vertices),
            );
            queue.write_buffer(
                &pool.indices,
                u64::from(range.first_index) * 4,
                bytemuck::cast_slice(&lod.indices),
            );
            pool.vertex_end += range.vertex_count;
            pool.index_end += range.index_count;
            log::info!(
                "VG ray proxy {key:#018x}: {} triangles of {} (error {:.4}, bounding radius {:.3}), {} KiB of vertices and indices",
                lod.triangle_count(),
                build.source_triangles,
                lod.error,
                build.bounds[3],
                (u64::from(range.vertex_count) * VERTEX_BYTES + u64::from(range.index_count) * 4) / 1024,
            );
            self.proxies.insert(
                key,
                Proxy {
                    range,
                    source_triangles: build.source_triangles,
                },
            );
        }
    }

    /// Reallocates the pool at twice what the live proxies and `extra` need,
    /// copying the live proxies in, packed.
    fn grow(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        extra_vertices: u32,
        extra_indices: u32,
    ) {
        let live_vertices: u32 = self.proxies.values().map(|p| p.range.vertex_count).sum();
        let live_indices: u32 = self.proxies.values().map(|p| p.range.index_count).sum();
        let vertices = (live_vertices + extra_vertices)
            .max(1024)
            .next_power_of_two()
            * 2;
        let indices = (live_indices + extra_indices).max(4096).next_power_of_two() * 2;
        let mut pool = Pool::new(device, vertices, indices);
        if let Some(old) = self.pool.take() {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("VG ray proxy pool growth"),
            });
            let mut keys: Vec<u64> = self.proxies.keys().copied().collect();
            keys.sort_unstable_by_key(|k| self.proxies[k].range.first_vertex);
            for key in keys {
                let range = &mut self.proxies.get_mut(&key).expect("listed above").range;
                encoder.copy_buffer_to_buffer(
                    &old.vertices,
                    u64::from(range.first_vertex) * VERTEX_BYTES,
                    &pool.vertices,
                    u64::from(pool.vertex_end) * VERTEX_BYTES,
                    u64::from(range.vertex_count) * VERTEX_BYTES,
                );
                encoder.copy_buffer_to_buffer(
                    &old.indices,
                    u64::from(range.first_index) * 4,
                    &pool.indices,
                    u64::from(pool.index_end) * 4,
                    u64::from(range.index_count) * 4,
                );
                range.first_vertex = pool.vertex_end;
                range.first_index = pool.index_end;
                pool.vertex_end += range.vertex_count;
                pool.index_end += range.index_count;
            }
            queue.submit([encoder.finish()]);
        }
        self.pool = Some(pool);
    }
}
