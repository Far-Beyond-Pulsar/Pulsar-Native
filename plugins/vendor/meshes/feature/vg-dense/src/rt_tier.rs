//! The CPU half of the ray-traced Tier 3: candidates in, proxies and a TLAS
//! out, with no stall of the render thread.
//!
//! Per frame ([`RtTier::prepare`], before any VG pass records):
//!
//! 1. Collect the newest finished candidate readback ([`crate::collect`]):
//!    the dense instances of a frame a few frames ago, with transform and
//!    asset. They are remembered for [`RtTierConfig::retain_frames`] so an
//!    instance that flickers in and out of dense tiles does not churn the TLAS.
//! 2. Ask [`helio_vg_stream::ProxyCache`] to build proxy LODs for their assets
//!    on worker threads, register finished builds (one per frame) with the
//!    [`DenseRtPass`], and drop evicted ones.
//! 3. Select a proxy level per remembered instance
//!    ([`crate::select_proxy_lod`]), request the pool pages of the selected
//!    levels from the stream store, and, only when the instance set, a level
//!    or the page residency changed, rebuild the TLAS and tables
//!    ([`DenseRtPass::prepare_frame_resolved`]).
//! 4. Decide readiness: the RT frame is complete when every remembered
//!    instance has a resident proxy and the latest readback *taken after the
//!    last TLAS change* reported no dense instance outside the TLAS.
//!
//! The pass wrapper then asks [`crate::Tier3Selector`] (mode, hardware,
//! readiness, dense-tile count, hysteresis) which backend runs.
//!
//! Latency and its consequence: candidates arrive 1-3 frames late, so the
//! first frame a new instance enters a dense tile its pixels are drawn by the
//! previous backend's result for it (NonRT while RT is not yet ready); once RT
//! is active, an instance that newly enters a dense tile can be a hole for the
//! 1-3 frames before the readback reports it, then the selector falls back to
//! NonRT until the TLAS covers it. Documented limitation of the readback
//! design; the alternative (a CPU stall) was rejected.

use crate::collect::{CollectResult, RtCollect, CAND_CAPACITY};
use crate::common::DenseFrame;
use crate::config::DenseConfig;
use crate::rt::{DenseRtPass, ProxyAsset, ProxyLod, RtError, RtFrameInfo, RtInstanceRequest};
use helio_core::acceleration::{BlasManager, TlasManager};
use helio_vg_stream::{proxy_key, ProxyBuild, ProxyCache, ProxyOptions, ProxyState, VgRuntime};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Tunables of the RT tier.
#[derive(Clone, Copy, Debug)]
pub struct RtTierConfig {
    /// Budget for CPU-built + GPU-resident proxy data, in bytes.
    pub proxy_budget_bytes: usize,
    /// Candidate readback capacity (instances per frame).
    pub candidate_capacity: u32,
    /// Frames an instance stays in the TLAS set after it was last seen dense.
    pub retain_frames: u64,
    /// Upper bound on TLAS instances.
    pub max_tlas_instances: u32,
    /// Proxy uploads per frame.
    pub registrations_per_frame: usize,
    pub proxy: ProxyOptionsCfg,
}

/// Plain-data copy of [`ProxyOptions`] (so the config stays `Copy`).
#[derive(Clone, Copy, Debug)]
pub struct ProxyOptionsCfg {
    pub levels: usize,
    pub ratio: f32,
    pub max_triangles_per_lod: u32,
}

impl Default for RtTierConfig {
    fn default() -> Self {
        let p = ProxyOptions::default();
        Self {
            proxy_budget_bytes: 384 << 20,
            candidate_capacity: CAND_CAPACITY,
            retain_frames: 90,
            max_tlas_instances: 16_384,
            registrations_per_frame: 1,
            proxy: ProxyOptionsCfg {
                levels: p.levels,
                ratio: p.ratio,
                max_triangles_per_lod: p.max_triangles_per_lod,
            },
        }
    }
}

/// What the tier needs from the frame.
pub struct RtEnv<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub runtime: &'a VgRuntime,
    pub frame: &'a DenseFrame,
    pub tau_px: f32,
    pub px_per_unit: f32,
}

/// The tier's verdict for the frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct RtPrep {
    /// All remembered dense instances are traceable and coverage was verified
    /// by a readback newer than the last TLAS change.
    pub rt_ready: bool,
    /// Dense tiles of the newest readback (lagged by a few frames).
    pub dense_tiles: u32,
}

struct Seen {
    asset_slot: u32,
    lod_error_scale: f32,
    transform: [f32; 12],
    last_seen: u64,
}

pub struct RtTier {
    cfg: RtTierConfig,
    pub(crate) rt: DenseRtPass,
    blas: BlasManager,
    tlas: TlasManager,
    pub(crate) collect: RtCollect,
    cache: ProxyCache,
    seen: HashMap<u32, Seen>,
    /// Store asset slot -> proxy key (content hash) and back.
    slot_key: HashMap<u32, u64>,
    key_slot: HashMap<u64, u32>,
    bounds: HashMap<u64, [f32; 4]>,
    registered: HashSet<u64>,
    pending: Vec<(u64, Arc<ProxyBuild>)>,
    frame: u64,
    collect_frame_at_change: u64,
    coverage_verified: bool,
    have_result: bool,
    last_dense_tiles: u32,
    /// Dense-tile counts of the latest readbacks: the raster re-routes clusters
    /// by last frame's mask, so the count can alternate frame to frame; the
    /// selector sees the window maximum.
    recent_dense: std::collections::VecDeque<u32>,
    /// RT stays off until this frame: an alpha-masked cluster overlapped a dense
    /// tile recently, and the opaque proxies cannot represent its cut-outs.
    masked_until: u64,
    last_info: Option<RtFrameInfo>,
    traceable_ready: bool,
}

impl RtTier {
    pub fn new(
        device: &wgpu::Device,
        dense: DenseConfig,
        cfg: RtTierConfig,
    ) -> Result<Self, RtError> {
        let rt = DenseRtPass::new(device, dense)?;
        let dev = Arc::new(device.clone());
        let proxy = ProxyOptions {
            levels: cfg.proxy.levels,
            ratio: cfg.proxy.ratio,
            max_triangles_per_lod: cfg
                .proxy
                .max_triangles_per_lod
                .min(device.limits().max_blas_primitive_count),
            ..ProxyOptions::default()
        };
        Ok(Self {
            blas: BlasManager::new(dev.clone()),
            tlas: TlasManager::new(dev, 16),
            collect: RtCollect::new(device, &dense, cfg.candidate_capacity),
            cache: ProxyCache::new(cfg.proxy_budget_bytes, proxy),
            rt,
            cfg,
            seen: HashMap::new(),
            slot_key: HashMap::new(),
            key_slot: HashMap::new(),
            bounds: HashMap::new(),
            registered: HashSet::new(),
            pending: Vec::new(),
            frame: 0,
            collect_frame_at_change: 0,
            coverage_verified: false,
            have_result: false,
            last_dense_tiles: 0,
            recent_dense: std::collections::VecDeque::with_capacity(8),
            masked_until: 0,
            last_info: None,
            traceable_ready: false,
        })
    }

    pub fn rt(&self) -> &DenseRtPass {
        &self.rt
    }

    pub fn rt_mut(&mut self) -> &mut DenseRtPass {
        &mut self.rt
    }

    pub fn tlas(&self) -> &TlasManager {
        &self.tlas
    }

    pub fn last_info(&self) -> Option<&RtFrameInfo> {
        self.last_info.as_ref()
    }

    fn absorb(&mut self, env: &RtEnv<'_>, res: CollectResult) {
        if std::env::var_os("VG_DEBUG").is_some() {
            eprintln!(
                "VgDense RT: readback frame {} total {} uncovered {} dense_tiles {} (collect frame {}, seen {})",
                res.recorded_frame, res.total, res.uncovered, res.dense_tiles, self.collect.frame(), self.seen.len()
            );
        }
        self.have_result = true;
        if self.recent_dense.len() == 8 {
            self.recent_dense.pop_front();
        }
        self.recent_dense.push_back(res.dense_tiles);
        self.last_dense_tiles = self.recent_dense.iter().copied().max().unwrap_or(0);
        // Alpha-masked clusters no longer gate the ray-traced backend: they are
        // not candidates and the masked peel merges them into the ray result.
        // Only a readback recorded after the last TLAS change says anything
        // about the current coverage.
        if res.recorded_frame > self.collect_frame_at_change {
            self.coverage_verified = res.uncovered == 0 && !res.overflowed();
            env.runtime.update_tier3_stats(|s| {
                if res.uncovered > 0 {
                    s.uncovered_readbacks += 1;
                }
            });
        }
        env.runtime.update_tier3_stats(|s| {
            s.candidates = res.total;
            if res.overflowed() {
                s.candidate_overflows += 1;
            }
        });
        let mut new_slots: Vec<u32> = Vec::new();
        for c in &res.records {
            if !self.slot_key.contains_key(&c.asset_slot) && !new_slots.contains(&c.asset_slot) {
                new_slots.push(c.asset_slot);
            }
            self.seen.insert(
                c.instance,
                Seen {
                    asset_slot: c.asset_slot,
                    lod_error_scale: c.lod_error_scale,
                    transform: c.transform,
                    last_seen: self.frame,
                },
            );
        }
        // Resolve the proxy key of assets met for the first time and start
        // their background builds.
        for slot in new_slots {
            let found = env
                .runtime
                .with_store(|s| s.core().asset_source(slot))
                .flatten();
            let Some((source, content, _quant)) = found else {
                continue;
            };
            let key = proxy_key(content);
            self.slot_key.insert(slot, key);
            self.key_slot.insert(key, slot);
            if self.cache.state(key).is_none() {
                self.cache.request(key, source, self.frame);
            }
        }
    }

    /// Per-frame CPU work. Never blocks on the GPU.
    pub fn prepare(&mut self, env: &RtEnv<'_>) -> RtPrep {
        self.frame += 1;
        let frame = self.frame;
        let t_start = std::time::Instant::now();
        self.collect.begin_maps();
        if let Some(res) = self.collect.poll(env.device) {
            self.absorb(env, res);
        }
        let t_poll = t_start.elapsed();

        // Proxy builds: poll, drop evicted, upload at most N finished ones.
        for key in self.cache.poll(frame) {
            self.rt.unregister_proxy_asset(&mut self.blas, key);
            self.registered.remove(&key);
            self.bounds.remove(&key);
            self.pending.retain(|(k, _)| *k != key);
        }
        self.pending.extend(self.cache.take_ready());
        for _ in 0..self.cfg.registrations_per_frame {
            let Some((key, build)) = self.pending.pop() else {
                break;
            };
            let build = Arc::try_unwrap(build).unwrap_or_else(|a| (*a).clone());
            let bounds = build.bounds;
            let asset = ProxyAsset {
                asset_key: key,
                lods: build
                    .lods
                    .into_iter()
                    .map(|l| ProxyLod {
                        error: l.error,
                        positions: l.positions,
                        indices: l.indices,
                        prim_map: l.prim_map,
                        clusters: l.clusters,
                        cluster_pages: l.cluster_pages,
                    })
                    .collect(),
            };
            match self.rt.register_proxy_asset(env.device, &asset) {
                Ok(_) => {
                    self.bounds.insert(key, bounds);
                    self.registered.insert(key);
                }
                Err(e) => {
                    log::warn!("virtual geometry RT: proxy of asset {key:#x} rejected: {e}");
                    self.cache.forget(key);
                }
            }
        }

        // Forget instances not seen dense for a while (bounded set).
        let horizon = frame.saturating_sub(self.cfg.retain_frames);
        self.seen.retain(|_, s| s.last_seen >= horizon);

        let t_proxy = t_start.elapsed();
        let mut prep = RtPrep {
            rt_ready: false,
            dense_tiles: self.last_dense_tiles,
        };
        let mut requests: Vec<RtInstanceRequest> = Vec::with_capacity(self.seen.len());
        let mut ids: Vec<u32> = self.seen.keys().copied().collect();
        if ids.len() > self.cfg.max_tlas_instances as usize {
            ids.sort_unstable_by_key(|i| std::cmp::Reverse(self.seen[i].last_seen));
            ids.truncate(self.cfg.max_tlas_instances as usize);
        }
        ids.sort_unstable();
        let mut unknown_asset = false;
        for id in ids {
            let s = &self.seen[&id];
            let Some(&key) = self.slot_key.get(&s.asset_slot) else {
                unknown_asset = true;
                continue;
            };
            self.cache.touch(key, frame, 8);
            let m = s.transform;
            let scale = {
                let col =
                    |c: usize| (m[c] * m[c] + m[4 + c] * m[4 + c] + m[8 + c] * m[8 + c]).sqrt();
                col(0).max(col(1)).max(col(2))
            };
            let b = self
                .bounds
                .get(&key)
                .copied()
                .unwrap_or([0.0, 0.0, 0.0, 0.0]);
            let center = [
                m[0] * b[0] + m[1] * b[1] + m[2] * b[2] + m[3],
                m[4] * b[0] + m[5] * b[1] + m[6] * b[2] + m[7],
                m[8] * b[0] + m[9] * b[1] + m[10] * b[2] + m[11],
            ];
            requests.push(RtInstanceRequest {
                instance_id: id,
                asset_key: key,
                transform: m,
                world_center: center,
                world_radius: b[3] * scale,
                error_scale: s.lod_error_scale * scale,
            });
        }

        let t_requests = t_start.elapsed();
        let mut info = RtFrameInfo::default();
        if !requests.is_empty() {
            let mut encoder = env
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("vg dense RT acceleration"),
                });
            let Self {
                rt,
                blas,
                tlas,
                key_slot,
                ..
            } = self;
            let result = env
                .runtime
                .with_store(|store| {
                    let version = store.core().table_version();
                    let r = {
                        let core = store.core();
                        let resolve = |key: u64, page: u32| {
                            key_slot
                                .get(&key)
                                .and_then(|&slot| core.local_page_slot(slot, page))
                        };
                        rt.prepare_frame_resolved(
                            env.device,
                            env.queue,
                            &mut encoder,
                            blas,
                            tlas,
                            env_frame(env),
                            &requests,
                            env.tau_px,
                            env.px_per_unit,
                            version,
                            &resolve,
                        )
                    };
                    if let Ok(info) = &r {
                        // Pull in (or keep resident) the pages of every level
                        // the selection wants, resident or not.
                        for &(key, lod) in &info.wanted_lods {
                            if let Some(&slot) = key_slot.get(&key) {
                                store
                                    .core_mut()
                                    .request_local_pages(slot, rt.lod_pages(key, lod));
                            }
                        }
                    }
                    r
                })
                .unwrap_or(Err(RtError::Unsupported));
            match result {
                Ok(i) => info = i,
                Err(e) => {
                    log::warn!("virtual geometry RT: acceleration structure update failed: {e}");
                    self.tlas.invalidate();
                    info = RtFrameInfo {
                        missing_proxy_instances: requests.len() as u32,
                        ..Default::default()
                    };
                }
            }
            if info.blas_builds > 0 || info.tlas_rebuilt {
                env.queue.submit([encoder.finish()]);
            }
            // Tell the GPU which instances the TLAS holds now.
            let ids: Vec<u32> = info.selections.iter().map(|(i, _)| *i).collect();
            let before = self.collect.covered_epoch();
            self.collect.set_covered(env.queue, &ids);
            if self.collect.covered_epoch() != before {
                self.collect_frame_at_change = self.collect.frame();
                self.coverage_verified = false;
            }
        }
        self.traceable_ready = !requests.is_empty()
            && !unknown_asset
            && info.tlas_instances > 0
            && info.complete()
            && self.rt.tlas_instance_count() == info.tlas_instances;
        prep.rt_ready = self.traceable_ready
            && self.coverage_verified
            && self.have_result
            && frame >= self.masked_until;
        if frame % 128 == 0 {
            self.rt.evict_unused(&mut self.blas, 1200);
        }

        let (proxy_assets, proxy_bytes) = self.rt.proxy_stats();
        let cache = self.cache.stats();
        let (tlas_rebuilt, blas_builds, over_tau, tlas_n) = (
            info.tlas_rebuilt,
            info.blas_builds,
            info.over_tau_instances,
            info.tlas_instances,
        );
        let had_requests = !requests.is_empty();
        let dense_tiles = self.last_dense_tiles;
        env.runtime.update_tier3_stats(|s| {
            s.rt_hardware = true;
            s.rt_tier_frames += 1;
            s.dense_tiles = dense_tiles;
            s.tlas_instances = tlas_n;
            if had_requests {
                if tlas_rebuilt {
                    s.tlas_rebuilds += 1;
                } else {
                    s.tlas_reuses += 1;
                }
            }
            s.blas_builds += blas_builds as u64;
            s.proxy_assets = proxy_assets;
            s.proxy_bytes = proxy_bytes;
            s.proxy_jobs_started = cache.jobs_started;
            s.proxy_jobs_failed = cache.jobs_failed;
            s.proxy_evictions = cache.evictions;
            s.over_tau_instances = over_tau;
        });
        if frame % 60 == 0 && std::env::var_os("VG_DEBUG").is_some() {
            eprintln!(
                "VgDense RT cpu us: poll {} proxies {} requests {} accel+rest {}",
                t_poll.as_micros(),
                (t_proxy - t_poll).as_micros(),
                (t_requests - t_proxy).as_micros(),
                (t_start.elapsed() - t_requests).as_micros()
            );
            eprintln!(
                "VgDense RT frame {frame}: requests {} tlas {} missing_proxy {} unresolved_pages {} dropped {} traceable {} coverage_verified {} have_result {} dense_tiles {} unknown_asset {}",
                requests.len(), info.tlas_instances, info.missing_proxy_instances, info.unresolved_page_instances, info.dropped_instances,
                self.traceable_ready, self.coverage_verified, self.have_result, self.last_dense_tiles, unknown_asset
            );
        }
        self.last_info = Some(info);
        prep
    }

    /// A pending proxy build of `key` failed: RT can never cover that asset.
    pub fn proxy_failed(&self, asset_slot: u32) -> bool {
        self.slot_key
            .get(&asset_slot)
            .is_some_and(|k| matches!(self.cache.state(*k), Some(ProxyState::Failed(_))))
    }

    pub fn note_incomplete(&self, runtime: &VgRuntime) {
        runtime.update_tier3_stats(|s| s.rt_incomplete_frames += 1);
    }
}

fn env_frame<'a>(env: &'a RtEnv<'_>) -> &'a DenseFrame {
    env.frame
}

impl RtTier {
    /// Record the dense-instance collection (after the NonRT setup stages).
    #[allow(clippy::too_many_arguments)]
    pub fn record_collect(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rec: &mut helio_core::CommandRecorder<'_>,
        dense: &crate::DenseBuffers,
        inputs: &crate::DenseNonRtInputs<'_>,
        instance_rows: u32,
        scopes: &mut crate::common::GpuScopes<'_>,
    ) {
        self.collect
            .record_scoped(device, queue, rec, dense, inputs, instance_rows, scopes);
    }

    /// Record the ray trace over dense tiles against the current TLAS.
    pub fn record_rt(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rec: &mut helio_core::CommandRecorder<'_>,
        frame: &DenseFrame,
        tile_mask: &wgpu::Buffer,
        scopes: &mut crate::common::GpuScopes<'_>,
    ) -> Result<(), RtError> {
        self.rt
            .record_scoped(device, queue, rec, frame, tile_mask, &self.tlas, scopes)
    }

    /// The RT output buffers: `(dense_key, bary, rt_visible)`.
    pub fn outputs(&self) -> Option<(wgpu::Buffer, wgpu::Buffer, wgpu::Buffer)> {
        let b = self.rt.buffers()?;
        Some((
            b.dense_key.clone(),
            self.rt.dense_bary()?.clone(),
            self.rt.rt_visible().clone(),
        ))
    }
}
