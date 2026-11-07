//! The shared, host-facing handle to virtual-geometry streaming: the
//! [`VgAssetStore`] plus the settings the passes read each frame and the
//! assets the host asked for.
//!
//! One [`VgRuntime`] is created by the host (editor / game) before the
//! renderer. The host
//!
//! * pushes the global settings ([`VgRuntime::set_settings`]),
//! * registers each static mesh whose `.vgeo` is built, together with the
//!   interned classic vertex range that identifies the mesh on the GPU
//!   ([`VgRuntime::request_asset`]),
//!
//! and the renderer side (the `vg_asset_ready` scene derivation and the graph
//! passes) service it: the store is created lazily on the first
//! [`VgRuntime::service`] call with the renderer's device, pending assets are
//! registered, and [`VgRuntime::update`] runs the streaming step once per
//! frame before the passes record.
//!
//! Everything is behind one mutex; every call is short and none blocks on the
//! GPU.

use crate::core::{VgStreamConfig, VgStreamError};
use crate::source::VgeoSource;
use crate::store::{VgAssetHandle, VgAssetStore};
use helio_vg_format::{VgAssetReadyRow, VgMode};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Settings the passes read every frame (from `VirtualGeometrySettings`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VgRuntimeSettings {
    pub mode: VgMode,
    /// Screen-space error threshold in pixels.
    pub tau_px: f32,
    /// Desired VG selection density, as selected triangles per viewport pixel.
    pub selection_triangles_per_pixel: f32,
    /// Strength of the previous-frame density feedback (0 disables it).
    pub selection_sensitivity: f32,
    /// Page pool budget in MiB (applied when the store is created).
    pub page_budget_mb: u32,
    /// Ceiling on visible clusters per frame (a ceiling, not an allocation).
    pub max_visible_clusters: u32,
    pub two_phase_occlusion: bool,
    /// Tier 3 (dense tiles: ray tracing where ready, depth peel on the no-int64
    /// Tier 2 fallback). Off by default: enabled but idle it costs under 0.02 ms,
    /// but on an RTX 4090 it measured no gain over Tier 2 (VG_STRESS.md).
    pub tier3: bool,
}

impl Default for VgRuntimeSettings {
    fn default() -> Self {
        let selection_triangles_per_pixel = std::env::var("HELIO_VG_SELECTION_TRIANGLES_PER_PIXEL")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(2.0)
            .clamp(0.25, 32.0);
        let selection_sensitivity = std::env::var("HELIO_VG_SELECTION_SENSITIVITY")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        Self {
            mode: VgMode::Auto,
            tau_px: 1.0,
            selection_triangles_per_pixel,
            selection_sensitivity,
            page_budget_mb: 512,
            max_visible_clusters: 4_194_304,
            two_phase_occlusion: true,
            tier3: false,
        }
    }
}

struct AssetEntry {
    content: [u64; 2],
    vertex_offset: u32,
    vertex_count: u32,
    source: Option<Arc<dyn VgeoSource>>,
    handle: Option<VgAssetHandle>,
    failed: bool,
}

/// One asset of [`VgRuntime::assets`].
#[derive(Clone)]
pub struct VgAssetInfo {
    pub content: [u64; 2],
    /// The mesh's interned classic vertex range (its identity on the GPU).
    pub vertex_offset: u32,
    pub vertex_count: u32,
    pub source: Arc<dyn VgeoSource>,
}

#[derive(Default)]
struct State {
    store: Option<VgAssetStore>,
    assets: Vec<AssetEntry>,
}

struct Inner {
    settings: Mutex<VgRuntimeSettings>,
    state: Mutex<State>,
    /// Bumped on every host-side change to the asset list or settings.
    host_version: AtomicU64,
    /// Bumped whenever [`VgRuntime::assets`] may change.
    assets_version: AtomicU64,
    tier3: Mutex<VgTier3Stats>,
}

/// See the module doc. Cloning shares the same runtime.
#[derive(Clone)]
pub struct VgRuntime(Arc<Inner>);

impl Default for VgRuntime {
    fn default() -> Self {
        Self::new(VgRuntimeSettings::default())
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Content id (`u128`) <-> the `[low, high]` words `.vgeo` headers carry.
pub fn content_words(content_id: u128) -> [u64; 2] {
    [content_id as u64, (content_id >> 64) as u64]
}

impl VgRuntime {
    pub fn new(settings: VgRuntimeSettings) -> Self {
        Self(Arc::new(Inner {
            settings: Mutex::new(settings),
            state: Mutex::new(State::default()),
            host_version: AtomicU64::new(1),
            assets_version: AtomicU64::new(1),
            tier3: Mutex::new(VgTier3Stats::default()),
        }))
    }

    pub fn settings(&self) -> VgRuntimeSettings {
        *lock(&self.0.settings)
    }

    pub fn set_settings(&self, settings: VgRuntimeSettings) {
        let mut current = lock(&self.0.settings);
        if *current != settings {
            *current = settings;
            self.0.host_version.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Ask for `content_id`'s `.vgeo` to be streamed. `vertex_offset` /
    /// `vertex_count` are the mesh's interned classic vertex range, the one
    /// identity a static-mesh instance carries on the GPU. Idempotent; a
    /// changed range replaces the old one.
    pub fn request_asset(
        &self,
        content_id: u128,
        vertex_offset: u32,
        vertex_count: u32,
        source: Arc<dyn VgeoSource>,
    ) {
        let content = content_words(content_id);
        let mut state = lock(&self.0.state);
        if let Some(entry) = state.assets.iter_mut().find(|e| e.content == content) {
            if entry.vertex_offset == vertex_offset && entry.vertex_count == vertex_count {
                return;
            }
            entry.vertex_offset = vertex_offset;
            entry.vertex_count = vertex_count;
        } else {
            state.assets.push(AssetEntry {
                content,
                vertex_offset,
                vertex_count,
                source: Some(source),
                handle: None,
                failed: false,
            });
        }
        self.0.host_version.fetch_add(1, Ordering::Relaxed);
        self.0.assets_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether `content_id` was requested (registered or pending).
    pub fn has_asset(&self, content_id: u128) -> bool {
        let content = content_words(content_id);
        lock(&self.0.state)
            .assets
            .iter()
            .any(|e| e.content == content)
    }

    /// Every requested asset that was not rejected, registered or pending:
    /// its content id, interned vertex range and bytes. For consumers that
    /// read assets on the CPU, such as the scene TLAS's proxies.
    pub fn assets(&self) -> Vec<VgAssetInfo> {
        let state = lock(&self.0.state);
        state
            .assets
            .iter()
            .filter(|e| !e.failed)
            .filter_map(|e| {
                let source = match (&e.source, e.handle, state.store.as_ref()) {
                    (Some(source), _, _) => source.clone(),
                    (None, Some(handle), Some(store)) => {
                        store.core().asset_source(handle.slot())?.0
                    }
                    _ => return None,
                };
                Some(VgAssetInfo {
                    content: e.content,
                    vertex_offset: e.vertex_offset,
                    vertex_count: e.vertex_count,
                    source,
                })
            })
            .collect()
    }

    /// Changes whenever [`Self::assets`] may have (assets requested,
    /// re-ranged or released).
    pub fn assets_version(&self) -> u64 {
        self.0.assets_version.load(Ordering::Relaxed)
    }

    /// Stop streaming `content_id` and free its pages.
    pub fn release_asset(&self, content_id: u128) {
        let content = content_words(content_id);
        let mut state = lock(&self.0.state);
        let Some(index) = state.assets.iter().position(|e| e.content == content) else {
            return;
        };
        let entry = state.assets.remove(index);
        if let (Some(handle), Some(store)) = (entry.handle, state.store.as_mut()) {
            store.release(handle);
        }
        self.0.host_version.fetch_add(1, Ordering::Relaxed);
        self.0.assets_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Create the store on first use and register pending assets. Cheap when
    /// idle. Call from the renderer side (derivation or pass `prepare`).
    pub fn service(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let budget = self.settings().page_budget_mb;
        let mut state = lock(&self.0.state);
        if state.store.is_none() {
            let cfg = VgStreamConfig {
                budget_mb: budget,
                ..VgStreamConfig::default()
            };
            state.store = Some(VgAssetStore::new(device, queue, cfg));
        }
        let State { store, assets } = &mut *state;
        let store = store.as_mut().expect("store created above");
        for entry in assets
            .iter_mut()
            .filter(|e| e.handle.is_none() && !e.failed)
        {
            let Some(source) = entry.source.clone() else {
                continue;
            };
            match store.register(source) {
                Ok(handle) => {
                    entry.handle = Some(handle);
                    // The CPU bytes live in the store now.
                    entry.source = None;
                }
                Err(error @ (VgStreamError::Source(_) | VgStreamError::Invalid(_))) => {
                    log::warn!("virtual geometry: asset rejected: {error}");
                    entry.failed = true;
                    self.0.assets_version.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    log::warn!("virtual geometry: asset not registered: {error}");
                    entry.failed = true;
                    self.0.assets_version.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// The per-frame streaming step (`VgAssetStore::update`). No-op before
    /// the store exists.
    pub fn update(&self) {
        if let Some(store) = lock(&self.0.state).store.as_mut() {
            store.update();
        }
    }

    /// Run `f` over the store, if it exists.
    pub fn with_store<R>(&self, f: impl FnOnce(&mut VgAssetStore) -> R) -> Option<R> {
        lock(&self.0.state).store.as_mut().map(f)
    }

    /// Whether the store exists.
    pub fn has_store(&self) -> bool {
        lock(&self.0.state).store.is_some()
    }

    /// `vg_asset_ready` rows for the scene derivation, and a version that
    /// changes whenever they may have (assets added/ready, settings).
    pub fn asset_ready_rows(&self, min_rows: usize) -> (Vec<VgAssetReadyRow>, u64) {
        let state = lock(&self.0.state);
        let host = self.0.host_version.load(Ordering::Relaxed);
        let Some(store) = state.store.as_ref() else {
            return (vec![VgAssetReadyRow::PADDING; min_rows.max(1)], host);
        };
        let mapping: Vec<([u64; 2], u32, u32)> = state
            .assets
            .iter()
            .filter(|e| e.handle.is_some())
            .map(|e| (e.content, e.vertex_offset, e.vertex_count))
            .collect();
        (
            store.asset_ready_rows(&mapping, min_rows),
            host.wrapping_mul(0x9e37_79b9)
                .wrapping_add(store.content_version()),
        )
    }
}

// ── Handle table ────────────────────────────────────────────────────────────
//
// `helio::RendererConfig` is `Copy`, so it carries a runtime as an opaque id
// (`RendererConfig::with_virtual_geometry`) that the graph builder resolves
// here. The table holds weak references: dropping the host's last
// [`VgRuntime`] ends the runtime.

use std::collections::HashMap;
use std::sync::{OnceLock, Weak};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn table() -> &'static Mutex<HashMap<u64, Weak<Inner>>> {
    static TABLE: OnceLock<Mutex<HashMap<u64, Weak<Inner>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl VgRuntime {
    /// An id that [`VgRuntime::from_handle_id`] resolves back to this runtime
    /// while any clone of it is alive. Stable per runtime.
    pub fn handle_id(&self) -> u64 {
        let mut table = lock(table());
        table.retain(|_, weak| weak.strong_count() > 0);
        if let Some((&id, _)) = table.iter().find(|(_, weak)| {
            weak.upgrade()
                .is_some_and(|inner| Arc::ptr_eq(&inner, &self.0))
        }) {
            return id;
        }
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        table.insert(id, Arc::downgrade(&self.0));
        id
    }

    /// The runtime registered under `id` by [`VgRuntime::handle_id`], if it
    /// is still alive.
    pub fn from_handle_id(id: u64) -> Option<Self> {
        lock(table()).get(&id).and_then(Weak::upgrade).map(Self)
    }

    /// Whether any registered asset has its root pages resident.
    pub fn any_ready(&self) -> bool {
        let state = lock(&self.0.state);
        let Some(store) = state.store.as_ref() else {
            return false;
        };
        state
            .assets
            .iter()
            .any(|e| e.handle.is_some_and(|h| store.is_ready(h)))
    }
}

// ── Tier 3 statistics ───────────────────────────────────────────────────────

/// Counters of the Tier-3 (dense overlap) backend selection, written by the
/// `VgDense` pass and read by tools and tests. Cheap to copy.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VgTier3Stats {
    /// Frames Tier 3 ran with the depth-peel backend.
    pub frames_nonrt: u64,
    /// Frames Tier 3 ran with the ray-traced backend.
    pub frames_rt: u64,
    /// Frames the candidate/proxy machinery of the RT tier was active
    /// (zero under `ForceNonRtFallback` or without hardware support).
    pub rt_tier_frames: u64,
    /// The device supports ray queries (`rt_available`).
    pub rt_hardware: bool,
    /// Dense tiles of the most recent frame the CPU learned about (lagged).
    pub dense_tiles: u32,
    /// Distinct dense instances seen by the latest completed readback.
    pub candidates: u32,
    /// Instances in the current TLAS.
    pub tlas_instances: u32,
    /// TLAS builds recorded / skipped because nothing changed.
    pub tlas_rebuilds: u64,
    pub tlas_reuses: u64,
    /// BLAS builds recorded (one per (asset, proxy level) first use).
    pub blas_builds: u64,
    /// Frames the selector wanted RT but proxies/instances were incomplete.
    pub rt_incomplete_frames: u64,
    /// Readbacks that reported dense instances missing from the TLAS.
    pub uncovered_readbacks: u64,
    /// Assets with GPU-resident proxies and the bytes they hold.
    pub proxy_assets: u32,
    pub proxy_bytes: u64,
    pub proxy_jobs_started: u64,
    pub proxy_jobs_failed: u64,
    pub proxy_evictions: u64,
    /// Over-tau instances in the latest RT frame (proxy coarser than tau).
    pub over_tau_instances: u32,
    /// Candidate list overflowed (more dense instances than the readback holds).
    pub candidate_overflows: u64,
    /// Latest Tier-3 counters (read back a few frames late): tiles resolved,
    /// cluster-list entries and dense tiles dropped by the caps, clamped peels.
    pub dense_tiles_processed: u32,
    pub bin_overflow: u32,
    pub slot_overflow: u32,
    pub clamped_tiles: u32,
    pub clamped_tris: u32,
    /// Peel layers summed over the tiles of the latest counters readback.
    pub peel_layers: u32,
    /// Tier 3 decided to take clusters from Tier 2 next frame (the cull diverts
    /// them to the dense bin only while this holds; set by the `VgDense` pass).
    pub engaged: bool,
    /// Tier 2 runs the tiled no-int64 fallback (its per-tile lists can overflow):
    /// the depth peel is then useful on its own. Set when the graph is built.
    pub tier2_tile_fallback: bool,
    pub list_total: u32,
    pub list_max: u32,
    pub tris_total: u32,
    pub tris_max: u32,
    pub masked_tiles: u32,
    pub masked_tris: u32,
    pub masked_tris_max: u32,
    /// CPU microseconds `RtTier::prepare` took on the latest frame.
    pub rt_prepare_cpu_us: u32,
}

impl VgRuntime {
    /// The latest Tier-3 counters.
    pub fn tier3_stats(&self) -> VgTier3Stats {
        *lock(&self.0.tier3)
    }

    /// Update the Tier-3 counters (the `VgDense` pass).
    pub fn update_tier3_stats(&self, f: impl FnOnce(&mut VgTier3Stats)) {
        f(&mut lock(&self.0.tier3));
    }
}
