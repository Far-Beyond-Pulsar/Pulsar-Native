//! Main HelioRenderer — wgpu + Helio scene renderer with built-in editor state.

use glam::{EulerRot, Mat4, Quat, Vec3};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use engine_fs::virtual_fs;
use helio::{
    Camera, DebugDrawState, EditorState, GizmoMode, GpuMaterial, GroupId, GroupMask,
    MaterialId, MeshId, MeshUpload, Movability, ObjectDescriptor, ObjectId, Renderer,
    RendererConfig, Scene, SceneActor, SceneActorId, ScenePicker, SkyActor,
};
use pulsar_events::script_registry;
use pulsar_reflection::{
    apply_runtime_behavior_for_class, scene_id_to_tag, ComponentRuntimeContext, LiveKeySet,
    RuntimeComponentOwner, Subsystems,
};
use helio_component::{
    subsystems::{
        apply_portal_pair_action, load_mesh_upload, remove_foliage_handles, resolve_asset_path,
        FoliageCache, MeshCache, PortalLinkCache,
    },
    PlanetTerrainFrameInput, PlanetTerrainRuntime, PLANET_TERRAIN_CLASS_NAME,
};
use pulsar_scene::{build_transform_parts, component_instances_from_props};
use helio_scenedb::HelioRenderSubsystem;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore};

use crate::scene::{ObjectDirtyFlags, ObjectUpdate, SceneDbDelta, WorldSceneStore};
use parking_lot::RwLock;
use super::core::{CameraInput, GpuProfilerData, RenderMetrics, RenderSpikeLogConfig};

/// Camera velocity squared below this threshold is considered stopped.
const CAMERA_IDLE_EPSILON: f32 = 0.001;

// ── Legacy types (unused but referenced by UI code) ──────────────────────────

#[derive(Debug, Clone)]
pub enum RendererCommand {
    ToggleFeature(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EditorCameraState {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

use std::sync::atomic::{AtomicBool, Ordering};

/// A left-click or left-release event, queued by the UI thread and drained
/// on the render thread at the top of [`HelioRenderer::render_frame`]
/// (Pulsar-Native drag-release freeze fix).
///
/// Previously `viewport/mod.rs`'s `on_mouse_up` called
/// `HelioRenderer::handle_left_release` directly via `gpu_engine.try_lock()`
/// -- non-blocking, with no retry. If that lost the race against the render
/// thread's own unconditional per-frame `gpu_engine.lock()` (which happens
/// every frame, not just under load), the release event was silently
/// dropped: `EditorState::end_drag()` -- called *only* from
/// `handle_left_release` -- never ran, `is_dragging()` stayed `true`
/// forever, and that permanently gated out `sync_scene`/`sync_scene_delta`
/// (`render_frame`'s `!inner.editor_state.is_dragging()` check) and idle
/// detection. This queue removes `gpu_engine` from the click/release path
/// entirely, so there's no lock left to lose that race on.
///
/// A `Vec`-backed mailbox, not a single-slot `Option` like
/// `pending_gizmo_mode` below -- click and release are order-sensitive and
/// must not collapse into "latest wins."
#[derive(Debug, Clone, Copy)]
pub enum PendingPointerEvent {
    LeftClick { norm_x: f32, norm_y: f32 },
    LeftRelease,
}

/// Cheap, `Clone`-able handle bundle for issuing editor commands
/// (gizmo-mode change, deselect, force-full-resync) without ever taking
/// `gpu_engine`'s blocking `std::sync::Mutex`.
///
/// `panel.rs` previously did `self.gpu_engine.lock()` for several one-shot
/// UI actions (tool switch, undo/redo, escape-to-deselect) -- a blocking
/// call that could stall the UI thread for as long as the render thread
/// holds `gpu_engine` (unconditionally, every frame, for the whole
/// `render_frame` call). Each of `queue_gizmo`/`queue_deselect`/
/// `queue_force_full_resync` below only ever touches its own small
/// `Arc<Mutex<...>>`/`Arc<AtomicBool>` mailbox (or `scene_store`'s already
/// cheap, short-held lock for `queue_gizmo`'s `set_gizmo_type` call) --
/// never `gpu_engine` -- so none of them can block on the render thread's
/// per-frame lock hold at all.
#[derive(Clone)]
pub struct HelioEditorMailbox {
    scene_store: Arc<RwLock<WorldSceneStore>>,
    pending_gizmo_mode: Arc<Mutex<Option<GizmoMode>>>,
    pending_deselect: Arc<AtomicBool>,
    pending_force_full_resync: Arc<AtomicBool>,
}

impl HelioEditorMailbox {
    /// Set the scene-store-level gizmo type immediately (already a cheap,
    /// short `scene_store.write()`, unrelated to `gpu_engine`) and queue the
    /// matching Helio gizmo mode for the render thread to pick up next frame.
    pub fn queue_gizmo(&self, scene_type: crate::scene::GizmoType, mode: GizmoMode) {
        self.scene_store.write().set_gizmo_type(scene_type);
        if let Ok(mut guard) = self.pending_gizmo_mode.lock() {
            *guard = Some(mode);
        }
    }

    /// Request that the editor state deselects the current object next frame.
    pub fn queue_deselect(&self) {
        self.pending_deselect.store(true, Ordering::Relaxed);
    }

    /// Request `force_full_resync()` at the start of the next render frame.
    /// See `HelioRenderer::pending_force_full_resync`'s doc for why this
    /// must never be silently dropped (unlike `queue_deselect`, which is
    /// pure UX and fine to occasionally miss a frame on).
    pub fn queue_force_full_resync(&self) {
        self.pending_force_full_resync.store(true, Ordering::Relaxed);
    }
}

// ── HelioRenderer ─────────────────────────────────────────────────────────────

/// Main renderer coordinating Helio 3D rendering with GPUI.
pub struct HelioRenderer {
    // ── Scene & Input ──
    pub camera_input: Arc<Mutex<CameraInput>>,
    pub scene_store: Arc<RwLock<WorldSceneStore>>,

    // ── Legacy (unused) ──
    pub command_sender: mpsc::Sender<RendererCommand>,
    pub command_receiver: mpsc::Receiver<RendererCommand>,

    // ── Pending editor commands (written by UI thread, read by render thread) ──
    /// Next gizmo mode to apply; consumed at start of render_frame.
    pub pending_gizmo_mode: Arc<Mutex<Option<GizmoMode>>>,
    /// When true, the render thread should call editor_state.deselect() next frame.
    pub pending_deselect: Arc<AtomicBool>,
    /// Left-click/left-release events queued by the UI thread, drained in
    /// order at the top of every `render_frame` -- see [`PendingPointerEvent`].
    pub pending_pointer_events: Arc<Mutex<Vec<PendingPointerEvent>>>,
    /// When true, the render thread should call `force_full_resync()` next
    /// frame. Unlike `pending_deselect` this is correctness-load-bearing,
    /// not just UX (see `force_full_resync`'s own doc) -- undo/redo route
    /// through this instead of a `gpu_engine.lock()` that could silently
    /// drop the request the same way the old click/release path could.
    pub pending_force_full_resync: Arc<AtomicBool>,

    // ── Renderer State ──
    /// Error messages from mesh loading failures, drained by the UI viewport for notifications.
    pub pending_errors: Arc<Mutex<Vec<String>>>,

    inner: Option<HelioInner>,

    // ── Camera State ──
    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    // Smoothed local-space velocity: x=right, y=up, z=forward (units/sec).
    cam_local_velocity: Vec3,
    viewport_size: (u32, u32),

    // ── TAA reset ──
    pub reset_taa_next_frame: bool,

    // ── Metrics ──
    pub metrics: Arc<Mutex<RenderMetrics>>,
    pub gpu_profiler: GpuProfilerData,
    last_frame: Instant,
    frame_count: u64,
    spike_log_config: RenderSpikeLogConfig,
    last_spike_warning: Option<Instant>,
    last_reported_gpu_frame: Option<u64>,
    last_planet_error: Option<String>,

    // ── Idle tracking ──
    /// Set when raw keyboard/mouse input was non-zero this frame, cleared
    /// once the camera decelerates to a stop.  Prevents the renderer from
    /// going idle the instant the user releases a key while velocity is
    /// still smoothing toward zero.
    had_camera_input: bool,
    /// Tracks whether the editor selection or gizmo mode changed since
    /// the last rendered frame.  When false the gizmo geometry is not
    /// rebuilt.
    gizmo_dirty: bool,
    /// Frame counter used to throttle GPU profiler reads to once every
    /// N frames so a fast idle loop doesn't hammer the timing API.
    profiler_frame_counter: u32,
}

struct HelioInner {
    renderer: Renderer,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    editor_state: EditorState,
    scene_picker: ScenePicker,
    /// Persists GPU-uploaded mesh geometry across frames so components
    /// don't re-load + re-upload the same asset every sync pass.
    mesh_cache: MeshCache,
    /// Persists foliage component handles (types/layers/interactors/materials)
    /// so the editor's per-sync component pass updates them in place instead of
    /// re-registering (which re-rolls GPU placement) every scene change.
    foliage_cache: FoliageCache,
    /// Pairs up `PortalComponent` instances that share a `portal_id` into
    /// real `helio::Scene` portals — see that type's doc for why portals
    /// need their own cache (unlike every other single-object cache here,
    /// a portal doesn't exist until *two* components agree on an ID).
    portal_link_cache: PortalLinkCache,
    /// Owns Pulsar's canonical planet state and incrementally publishes it to
    /// the planetary pass in this renderer's graph. Helio's GPU residency is
    /// deliberately not duplicated here.
    planet_terrain: Option<PlanetTerrainRuntime>,
    /// Set when the graph-owned planetary cache was created or recreated. The
    /// controller consumes it on the first frame after Helio's deferred resize
    /// has completed and republishes canonical pages into the new cache.
    planet_graph_rebuilt: bool,
    /// Last SceneDb generation fully applied to Helio. Unchanged scenes do not
    /// need component deserialization, light recreation, or picker rebuilds.
    last_scene_revision: u64,
    /// Set of scene-object IDs that have been synced to Helio.
    /// Used by `sync_scene_delta` to distinguish additions from updates.
    known_ids: HashSet<String>,
}

// component_instances_from_snap delegates to pulsar_scene's shared impl.
// Hoisted out of `sync_scene` (was a nested fn) so `sync_scene_delta`'s own
// per-entity dispatch path (`sync_snapshot_components`) can call it too --
// see that fn's doc for why the delta path needs the same dispatch `sync_scene`
// already does, not just a separate transform/visibility patch.
fn component_instances_from_snap(
    snap: &crate::scene::ObjectSnapshot,
) -> Vec<(usize, String, serde_json::Value)> {
    component_instances_from_props(
        &snap.render_props.props,
        snap.render_props.component_instances.as_ref(),
    )
}

// Hoisted out of `sync_scene` for the same reason as `component_instances_from_snap`
// above -- `sync_snapshot_components` (shared by `sync_scene` and `sync_scene_delta`)
// needs it too, and a struct definition can't live inside an `impl` block as an
// associated item the way a nested fn can live inside a method.
struct HelioRuntimeContext<'a> {
    renderer: &'a mut Renderer,
    subsystems: Subsystems,
    error_queue: &'a Arc<Mutex<Vec<String>>>,
    project_root: &'a Path,
}

impl<'a> ComponentRuntimeContext for HelioRuntimeContext<'a> {
    fn subsystems_mut(&mut self) -> &mut Subsystems {
        &mut self.subsystems
    }

    fn project_root(&self) -> &std::path::Path {
        &self.project_root
    }

    fn report_error(&mut self, message: String) {
        tracing::error!("{}", message);
        if let Ok(mut eq) = self.error_queue.lock() {
            eq.push(message);
        }
    }
}

/// One entity's worth of GPU-native seam data (Pulsar-Native#561 Phase D),
/// queued during `sync_snapshot_components`'s read-locked pass and applied
/// to `World` in a short write-lock afterward -- mirrors the existing
/// dirty-flag-drain's own read-then-write split (`sync_scene`'s Phase 1/
/// Phase 2), for the same reason: resolving this data needs `&mut
/// HelioInner` (mesh cache, renderer) but not `&mut WorldSceneStore`, so
/// there's no reason to hold a write lock for it.
struct PendingStaticMeshSeamUpsert {
    entity: pulsar_scenedb::Entity,
    mesh: MeshId,
    material: GpuMaterial,
    transform: Mat4,
    bounds: [f32; 4],
}

impl HelioRenderer {
    pub fn new(scene_store: Arc<RwLock<WorldSceneStore>>) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        Self {
            camera_input: Arc::new(Mutex::new(CameraInput::new())),
            scene_store,
            command_sender,
            command_receiver,
            pending_gizmo_mode: Arc::new(Mutex::new(None)),
            pending_deselect: Arc::new(AtomicBool::new(false)),
            pending_pointer_events: Arc::new(Mutex::new(Vec::new())),
            pending_force_full_resync: Arc::new(AtomicBool::new(false)),
            reset_taa_next_frame: false,
            inner: None,
            pending_errors: Arc::new(Mutex::new(Vec::new())),
            cam_pos: Vec3::new(8.0, 6.0, 12.0),
            cam_yaw: -0.5,
            cam_pitch: -0.3,
            cam_local_velocity: Vec3::ZERO,
            viewport_size: (0, 0),
            metrics: Arc::new(Mutex::new(RenderMetrics::default())),
            gpu_profiler: GpuProfilerData::default(),
            last_frame: Instant::now(),
            frame_count: 0,
            spike_log_config: RenderSpikeLogConfig::default(),
            last_spike_warning: None,
            last_reported_gpu_frame: None,
            last_planet_error: None,
            had_camera_input: false,
            gizmo_dirty: true,
            profiler_frame_counter: 0,
        }
    }

    pub fn editor_camera_state(&self) -> EditorCameraState {
        EditorCameraState {
            position: self.cam_pos.to_array(),
            yaw: self.cam_yaw,
            pitch: self.cam_pitch,
        }
    }

    pub fn set_editor_camera_state(&mut self, state: EditorCameraState) {
        self.cam_pos = Vec3::from_array(state.position);
        self.cam_yaw = state.yaw;
        self.cam_pitch = state.pitch;
        self.cam_local_velocity = Vec3::ZERO;

        if let Ok(mut input) = self.camera_input.lock() {
            input.forward = 0.0;
            input.right = 0.0;
            input.up = 0.0;
            input.clear_transient_deltas();
        }
    }

    /// Configure cheap frame-spike warning cadence independently from deep
    /// WGPUI capture. Disabling this affects only warning logs.
    pub fn set_spike_log_config(&mut self, config: RenderSpikeLogConfig) {
        self.spike_log_config = config;
        self.last_spike_warning = None;
    }

    pub fn spike_log_config(&self) -> RenderSpikeLogConfig {
        self.spike_log_config
    }

    /// Called each GPUI frame from the viewport.
    pub fn render_frame(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Option<wgpu::SubmissionIndex> {
        profiling::profile_scope!("helio_frame");
        let frame_start = Instant::now();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.frame_count += 1;
        self.profiler_frame_counter += 1;

        // ── Lazy init (first frame only) ────────────────────────────────────────
        if self.inner.is_none() {
            tracing::info!("Initializing Helio renderer...");

            let device_arc = Arc::new(_device.clone());
            let queue_arc = Arc::new(_queue.clone());
            let config = RendererConfig::new(width, height, format);
            let r = helio::RendererBuilder::new(config)
                .with_external_device()
                .with_editor_mode(true)
                .with_clear_color([0.15, 0.18, 0.25, 1.0])
                .with_ambient([0.0, 0.0, 0.0], 0.0)
                .with_graph(Box::new(|d, q, s, c, ds, cb, csb| {
                    helio_default_graphs::build_default_graph_external(
                        d, q, s, c, ds, cb, csb, None,
                    )
                }))
                .build(device_arc.clone(), queue_arc.clone(), width, height, format);

            let mut inner = HelioInner {
                renderer: r,
                device: device_arc.clone(),
                queue: queue_arc.clone(),
                editor_state: EditorState::new(),
                scene_picker: ScenePicker::new(),
                mesh_cache: MeshCache::new(),
                foliage_cache: FoliageCache::new(),
                portal_link_cache: PortalLinkCache::new(),
                planet_terrain: None,
                planet_graph_rebuilt: false,
                last_scene_revision: 0,
                known_ids: HashSet::new(),
            };
            self.populate_initial_scene(&mut inner);
            self.inner = Some(inner);
            self.viewport_size = (width, height);

            // ── SceneDB GPU-native render seam (Pulsar-Native#561 Phase D)
            // ──────────────────────────────────────────────────────────
            // First point `device`/`queue` exist -- `SceneGpuStore` needs a
            // real `wgpu::Device`/`Queue` (CONTRACTS C0: the SceneDB core
            // stays graphics-free without one, so it can't be constructed
            // any earlier than this). Idempotent guard (`has_gpu_mirror`):
            // more than one `HelioRenderer` sharing the same `scene_store`
            // (e.g. multiple viewports) must not clobber an already-wired
            // mirror/subsystem from an earlier renderer's first frame.
            {
                let mut store_guard = self.scene_store.write();
                let scene_db = store_guard.scene_db_mut();
                if !scene_db.world.has_gpu_mirror() {
                    let ctx = EngineGpuContext::new(device_arc.clone(), queue_arc.clone());
                    // Minimal, cell-mirror-region config -- this seam only
                    // uses the World-mirror (growable, auto-registering)
                    // path for StaticMeshComponent/MaterialSlot today, not
                    // SceneGpuStore's fixed-region cell-mirrored buffers, so
                    // these numbers are placeholder-safe (proven values,
                    // copied from `helio-scenedb`'s own `tests/support::
                    // scene_cfg()`), not load-bearing -- revisit once a real
                    // cell-mirrored consumer exists in the live editor.
                    let gpu_cfg = SceneGpuConfig {
                        classes: vec![RegionClassConfig { capacity: 256, max_resident_cells: 4 }],
                        tombstone_headroom: 8,
                        max_cells_metadata: 16,
                    };
                    let gpu_store = Arc::new(SceneGpuStore::new(&ctx, gpu_cfg));
                    let mirror = GpuMirrorHandle::new(gpu_store, queue_arc.clone());
                    scene_db.world.attach_gpu_mirror(mirror);
                    scene_db.register_subsystem(HelioRenderSubsystem::new());
                    tracing::info!(
                        "[HELIO] SceneDB GPU-native render seam wired (HelioRenderSubsystem registered)"
                    );
                }
            }

            tracing::info!(
                "[HELIO] Renderer initialized - camera at {:?}, yaw={}, pitch={}",
                self.cam_pos,
                self.cam_yaw,
                self.cam_pitch
            );
            // First frame must render (lazy init includes nothing visible)
        }

        // ── Pending pointer events (queued by the UI thread, see
        // `PendingPointerEvent`'s doc) ──────────────────────────────────────────
        // Drained unconditionally, before `self.inner` is borrowed below and
        // before the idle/pending-scene checks that follow -- `handle_left_click`/
        // `handle_left_release` already set `self.gizmo_dirty = true`
        // internally, so processing them here needs no extra plumbing to keep
        // this frame from idling out on a drag-release commit.
        let pending_pointer_events = self
            .pending_pointer_events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default();
        for event in pending_pointer_events {
            match event {
                PendingPointerEvent::LeftClick { norm_x, norm_y } => {
                    self.handle_left_click(norm_x, norm_y);
                }
                PendingPointerEvent::LeftRelease => {
                    self.handle_left_release();
                }
            }
        }

        // ── Detect input activity BEFORE consuming ──────────────────────────────
        let (had_input, needs_resize) = {
            let Ok(input) = self.camera_input.lock() else {
                return None;
            };
            (
                input.forward != 0.0
                    || input.right != 0.0
                    || input.up != 0.0
                    || input.mouse_delta_x != 0.0
                    || input.mouse_delta_y != 0.0
                    || input.pan_delta_x != 0.0
                    || input.pan_delta_y != 0.0
                    || input.zoom_delta != 0.0,
                input.needs_resize,
            )
        };

        if had_input {
            self.had_camera_input = true;
        }

        {
            profiling::profile_scope!("helio_camera_input");
            self.apply_camera_input(dt);
        }

        let inner = match self.inner.as_mut() {
            Some(i) => i,
            None => return None,
        };

        // ── Idle detection ───────────────────────────────────────────────────────
        // If the camera is fully stopped, no scene changes are pending, and no
        // editor state changed, we can skip the GPU render entirely.  The render
        // thread keeps pacing itself but returns `None`, which causes the
        // background loop to skip present/publish — the compositor holds the last
        // frame on screen.
        let viewport_resized = needs_resize || self.viewport_size != (width, height);
        let scene_revision = self.scene_store.read().render_revision();
        let has_pending_scene = scene_revision != inner.last_scene_revision;
        let has_pending_editor = self.pending_deselect.load(Ordering::Acquire)
            || self
                .pending_gizmo_mode
                .lock()
                .is_ok_and(|g| g.is_some());
        let camera_stopped =
            self.cam_local_velocity.length_squared() <= CAMERA_IDLE_EPSILON && !self.had_camera_input;
        let is_idle = camera_stopped
            && !has_pending_scene
            && !has_pending_editor
            && !self.gizmo_dirty
            && !viewport_resized
            && !self.reset_taa_next_frame
            && !inner.editor_state.is_dragging();

        // Clear the sticky input flag when camera actually stopped.
        if camera_stopped {
            self.had_camera_input = false;
        }

        // Advance wind every frame (frozen clock yields static lean — correct).
        inner.renderer.scene_mut().advance_wind(dt);

        // ── Resize ──────────────────────────────────────────────────────────────
        if viewport_resized {
            profiling::profile_scope!("helio_resize");
            inner.renderer.set_render_size(width, height);
            if inner.planet_terrain.as_ref().is_some_and(|runtime| {
                runtime.has_active_components() && runtime.renderer_ready(&inner.renderer)
            }) {
                inner.planet_graph_rebuilt = true;
            }
            self.viewport_size = (width, height);
        }

        // ── Pending editor commands ─────────────────────────────────────────────
        if self.pending_deselect.swap(false, Ordering::AcqRel) {
            inner.editor_state.deselect();
            self.gizmo_dirty = true;
        }
        if let Ok(mut pending) = self.pending_gizmo_mode.lock() {
            if let Some(mode) = pending.take() {
                inner.editor_state.set_gizmo_mode(mode);
                self.gizmo_dirty = true;
            }
        }
        // Inlined rather than calling `self.force_full_resync()` -- `inner`
        // above is already a live `&mut` borrow of `self.inner` at this
        // point, and `force_full_resync` needs the same borrow itself.
        if self
            .pending_force_full_resync
            .swap(false, Ordering::AcqRel)
        {
            inner.last_scene_revision = 0;
            inner.known_ids.clear();
        }

        // ── Early out when idle ─────────────────────────────────────────────────
        // No GPU work, no gizmo rebuild, no planet terrain tick, no profiler reads.
        if is_idle {
            if let Ok(mut m) = self.metrics.lock() {
                m.fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                m.frame_time_ms = dt * 1000.0;
                m.frames_rendered = self.frame_count;
            }
            return None;
        }

        // ── Scene sync (delta when possible, full only on first frame) ──────────
        let mut sync_ms = 0.0;
        if has_pending_scene && !inner.editor_state.is_dragging() {
            profiling::profile_scope!("helio_scene_sync");
            let t_sync = Instant::now();
            // Use delta sync for incremental updates; full sync only on first
            // frame when last_scene_revision is 0 and known_ids is empty.
            if inner.last_scene_revision == 0 && inner.known_ids.is_empty() {
                Self::sync_scene(&self.scene_store, inner, &self.pending_errors);
            } else {
                Self::sync_scene_delta(&self.scene_store, inner, &self.pending_errors);
            }
            sync_ms = t_sync.elapsed().as_secs_f64() * 1000.0;
            inner.last_scene_revision = scene_revision;
        }

        // ── Camera / planet / gizmo / render ────────────────────────────────────
        let t_prepare = Instant::now();
        let camera = {
            profiling::profile_scope!("helio_frame_prepare");
            let (sy, cy) = self.cam_yaw.sin_cos();
            let (sp, cp) = self.cam_pitch.sin_cos();
            let fwd = Vec3::new(sy * cp, sp, -cy * cp);
            let aspect = width as f32 / height.max(1) as f32;
            let camera = Camera::perspective_look_at(
                self.cam_pos,
                self.cam_pos + fwd,
                Vec3::Y,
                std::f32::consts::FRAC_PI_4,
                aspect,
                0.1,
                10_000.0,
            );

            // Planet terrain advance (only when camera is actually moving).
            let should_advance_planet = !viewport_resized
                && inner.planet_terrain.as_ref().is_some_and(|runtime| {
                    runtime.has_active_components() && runtime.renderer_ready(&inner.renderer)
                })
                && (!camera_stopped || viewport_resized);
            if should_advance_planet {
                let graph_rebuilt = std::mem::take(&mut inner.planet_graph_rebuilt);
                let planet_terrain = inner
                    .planet_terrain
                    .as_mut()
                    .expect("planet runtime was checked above");
                let horizontal_forward = Vec3::new(sy, 0.0, -cy);
                let right = Vec3::new(cy, 0.0, sy);
                let velocity = right * self.cam_local_velocity.x
                    + Vec3::Y * self.cam_local_velocity.y
                    + horizontal_forward * self.cam_local_velocity.z;
                let input = PlanetTerrainFrameInput {
                    camera_m: self.cam_pos.as_dvec3().to_array(),
                    forward: fwd.as_dvec3().to_array(),
                    up: Vec3::Y.as_dvec3().to_array(),
                    vertical_fov_radians: f64::from(std::f32::consts::FRAC_PI_4),
                    viewport_px: [width.max(1), height.max(1)],
                    near_m: 0.1,
                    far_m: 10_000.0,
                    velocity_mps: velocity.as_dvec3().to_array(),
                    delta_time_s: dt,
                    tick: self.frame_count,
                    frame_index: self.frame_count,
                    graph_rebuilt,
                };
                let planet_error = match planet_terrain.advance(
                    &mut inner.renderer,
                    inner.device.as_ref(),
                    inner.queue.as_ref(),
                    input,
                ) {
                    Ok(report) if report.planning_failures.is_empty() => None,
                    Ok(report) => Some(report.planning_failures.join("; ")),
                    Err(error) => Some(format!("Planet terrain streaming failed: {error}")),
                };
                if planet_error != self.last_planet_error {
                    if let Some(message) = planet_error.as_ref() {
                        tracing::error!("{message}");
                        if let Ok(mut errors) = self.pending_errors.lock() {
                            errors.push(message.clone());
                        }
                    } else if self.last_planet_error.is_some() {
                        tracing::info!("Planet terrain streaming recovered");
                    }
                    self.last_planet_error = planet_error;
                }
            }

            // Gizmo drawing must run every active frame — the camera may have
            // moved, and debug_clear() wipes the previous frame's geometry, so
            // the gizmo would disappear entirely on frame 2 without this call.
            // The `gizmo_dirty` flag is used to *wake* the renderer from idle
            // when only selection/mode changes (no camera motion or scene edit),
            // but once active we always draw.
            inner.renderer.debug_clear();
            inner.renderer.set_gizmo_camera(&camera, height as f32);
            inner.editor_state.draw_gizmos(&mut inner.renderer);
            camera
        };

        if self.reset_taa_next_frame {
            self.reset_taa_next_frame = false;
        }

        let prepare_ms = t_prepare.elapsed().as_secs_f64() * 1000.0;
        let t_render = Instant::now();
        let submission_index = {
            profiling::profile_scope!("helio_render_submit");
            if let Err(e) = inner.renderer.render(&camera, &view) {
                tracing::error!("Helio render error: {:?}", e);
            }
            Some(inner.queue.submit(std::iter::empty::<wgpu::CommandBuffer>()))
        };
        let render_ms = t_render.elapsed().as_secs_f64() * 1000.0;
        let frame_ms = frame_start.elapsed().as_secs_f32() * 1_000.0;

        // ── GPU profiler (throttled to every 30 frames) ─────────────────────────
        if self.profiler_frame_counter >= 30 {
            self.profiler_frame_counter = 0;
            self.gpu_profiler
                .update_from_snapshot(inner.renderer.timing_snapshot());
        }

        let gpu_frame = self.gpu_profiler.gpu_frame_count;
        let new_gpu_result = gpu_frame.is_some() && gpu_frame != self.last_reported_gpu_frame;
        let gpu_spike = new_gpu_result
            && self
                .gpu_profiler
                .total_gpu_ms
                .is_some_and(|time| time > self.spike_log_config.gpu_threshold_ms);
        let cpu_spike = frame_ms > self.spike_log_config.cpu_threshold_ms;
        let warning_due = self.spike_log_config.enabled
            && self
                .last_spike_warning
                .is_none_or(|last| last.elapsed() >= self.spike_log_config.min_interval);

        if warning_due && (cpu_spike || gpu_spike) {
            let (cpu_pass, cpu_pass_ms) = self
                .gpu_profiler
                .slowest_cpu_pass()
                .unwrap_or(("unavailable", 0.0));
            let (gpu_pass, gpu_pass_ms) = self
                .gpu_profiler
                .slowest_gpu_pass()
                .unwrap_or(("pending", 0.0));
            tracing::warn!(
                "[HELIO FRAME SPIKE] frame={:.1}ms (sync {:.1}, prepare {:.1}, submit {:.1}); \
                 slowest CPU pass={} {:.1}ms; GPU frame={:?} total={:?}ms lag={:?} \
                 slowest pass={} {:.1}ms drops={} overflows={}",
                frame_ms,
                sync_ms,
                prepare_ms,
                render_ms,
                cpu_pass,
                cpu_pass_ms,
                gpu_frame,
                self.gpu_profiler.total_gpu_ms,
                self.gpu_profiler.gpu_lag_frames,
                gpu_pass,
                gpu_pass_ms,
                self.gpu_profiler.readback_drops,
                self.gpu_profiler.query_overflows
            );
            self.last_spike_warning = Some(Instant::now());
        }
        if new_gpu_result {
            self.last_reported_gpu_frame = gpu_frame;
        }

        if let Ok(mut m) = self.metrics.lock() {
            m.fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
            m.frame_time_ms = dt * 1000.0;
            m.frames_rendered = self.frame_count;
        }

        submission_index
    }

    fn apply_camera_input(&mut self, dt: f32) {
        const LOOK: f32 = 0.0025;

        let input = match self.camera_input.lock() {
            Ok(mut lock) => {
                let snap = lock.clone();
                lock.clear_transient_deltas();
                snap
            }
            Err(_) => return,
        };

        self.cam_yaw += input.mouse_delta_x * LOOK;
        self.cam_pitch -= input.mouse_delta_y * LOOK;
        self.cam_pitch = self.cam_pitch.clamp(-1.5, 1.5);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let fwd = Vec3::new(sy, 0.0, -cy);
        let right = Vec3::new(cy, 0.0, sy);
        let speed = if input.boost {
            input.move_speed * 3.0
        } else {
            input.move_speed
        };

        // Target local velocity from input (units/sec).
        let target_velocity =
            Vec3::new(input.right * speed, input.up * speed, input.forward * speed);

        // Keyboard velocity applies instantly — matching the crisp, zero-latency
        // behavior of mouse look. The previous exponential ease-in/out
        // (ACCEL_RATE 10 / DECEL_RATE 14) took ~230ms to reach 90% of target
        // speed, which is what made WASD feel "mushy" next to the mouse.
        // Camera position stays continuous (velocity * dt integration), so an
        // instant velocity step produces no visible jump.
        self.cam_local_velocity = target_velocity;

        self.cam_pos += right * self.cam_local_velocity.x * dt;
        self.cam_pos += Vec3::Y * self.cam_local_velocity.y * dt;
        self.cam_pos += fwd * self.cam_local_velocity.z * dt;

        // Middle-mouse (or right-click + Shift) view-plane pan: translate the camera
        // along its screen right/up axes for a 1:1 "grab" feel. Applied directly from
        // the accumulated pixel delta (not velocity-smoothed, not dt-scaled).
        if input.pan_delta_x != 0.0 || input.pan_delta_y != 0.0 {
            const PAN: f32 = 0.01;
            let sp = self.cam_pitch.sin();
            let cp = self.cam_pitch.cos();
            // Full view forward (includes pitch); screen-up is right × forward.
            let forward_full = Vec3::new(cp * sy, sp, -cp * cy);
            let screen_up = right.cross(forward_full);
            let pan_speed = PAN * input.move_speed.max(1.0);
            // Grab convention: dragging right moves content right (camera goes left);
            // dragging down moves content down (camera goes up).
            self.cam_pos += right * (-input.pan_delta_x) * pan_speed;
            self.cam_pos += screen_up * input.pan_delta_y * pan_speed;
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.inner.is_some()
    }

    pub fn get_metrics(&self) -> RenderMetrics {
        self.metrics.lock().map(|m| m.clone()).unwrap_or_default()
    }

    pub fn get_gpu_profiler_data(&self) -> GpuProfilerData {
        self.gpu_profiler.clone()
    }

    // ── Editor Integration ───────────────────────────────────────────────────

    /// Queue a new gizmo mode to be applied at the start of the next render frame.
    pub fn queue_gizmo_mode(&self, mode: crate::GizmoMode) {
        if let Ok(mut guard) = self.pending_gizmo_mode.lock() {
            *guard = Some(mode);
        }
    }

    /// Request that the editor state deselects the current object next frame.
    pub fn queue_deselect(&self) {
        self.pending_deselect
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Queue a left-click for the render thread to process at the top of
    /// its next frame, instead of the UI thread calling `handle_left_click`
    /// directly through `gpu_engine`. See [`PendingPointerEvent`]'s doc.
    pub fn queue_left_click(&self, norm_x: f32, norm_y: f32) {
        if let Ok(mut events) = self.pending_pointer_events.lock() {
            events.push(PendingPointerEvent::LeftClick { norm_x, norm_y });
        }
    }

    /// Queue a left-release the same way. See [`PendingPointerEvent`]'s doc
    /// -- this is the one that used to be silently droppable via a lost
    /// `gpu_engine.try_lock()` race, permanently wedging `is_dragging()`.
    pub fn queue_left_release(&self) {
        if let Ok(mut events) = self.pending_pointer_events.lock() {
            events.push(PendingPointerEvent::LeftRelease);
        }
    }

    /// Request `force_full_resync()` at the start of the next render frame,
    /// via the same always-delivered mailbox mechanism as `pending_deselect`
    /// rather than a `gpu_engine.lock()` call that could race and drop the
    /// request. See [`Self::pending_force_full_resync`]'s doc for why this
    /// one specifically must never be silently dropped.
    pub fn queue_force_full_resync(&self) {
        self.pending_force_full_resync
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get the active gizmo state from the scene store (gizmo_type, highlighted_axis, etc.).
    pub fn get_scene_gizmo_type(&self) -> crate::scene::GizmoType {
        self.scene_store.read().get_gizmo_state().gizmo_type
    }

    /// Set the active gizmo type on the scene store.
    pub fn set_scene_gizmo_type(&self, t: crate::scene::GizmoType) {
        self.scene_store.write().set_gizmo_type(t);
    }

    /// Return the scene-store-level selected object ID (set by
    /// `select_object_atomic` on viewport click or by the hierarchy panel).
    pub fn get_scene_db_selected_id(&self) -> Option<String> {
        self.scene_store.read().get_selected_id()
    }

    /// Force the next `sync_scene`/`sync_scene_delta` call to take the full
    /// (non-delta) path, exactly as if this were the first frame.
    ///
    /// Needed after anything that replaces `WorldSceneStore` wholesale rather
    /// than mutating it in place -- undo/redo (Pulsar-Native#554) being the
    /// motivating case. `sync_scene_delta` diffs against `known_ids` and the
    /// dirty/removed sets *of the store instance it's looking at right now*;
    /// it has no way to notice that an entity present a moment ago in a
    /// now-discarded store instance no longer exists. A full `sync_scene`
    /// pass sidesteps that entirely -- it recomputes `live_keys` from
    /// scratch and tears down anything in Helio's caches that isn't in it,
    /// which is correct regardless of *how* an object disappeared.
    ///
    /// No-op if the renderer hasn't produced its first frame yet (`inner` is
    /// `None`) -- that frame is already guaranteed to run a full sync by
    /// construction (`last_scene_revision`/`known_ids` start at their
    /// "first frame" values), so there's nothing to reset.
    pub fn force_full_resync(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.last_scene_revision = 0;
            inner.known_ids.clear();
        }
    }

    /// A small, cheaply-`Clone`-able bundle of this renderer's cross-thread
    /// mailbox handles, for UI code (`panel.rs`) that wants to send an
    /// editor command without going through `gpu_engine`'s blocking
    /// `Mutex`. See [`HelioEditorMailbox`]'s own doc.
    pub fn editor_mailbox(&self) -> HelioEditorMailbox {
        HelioEditorMailbox {
            scene_store: self.scene_store.clone(),
            pending_gizmo_mode: self.pending_gizmo_mode.clone(),
            pending_deselect: self.pending_deselect.clone(),
            pending_force_full_resync: self.pending_force_full_resync.clone(),
        }
    }

    // ── Unified per-object scene mutations ────────────────────────────────────
    // These are called directly by SceneDatabase so every write path (user
    // actions, AI tools, content-drawer drops) hits Helio immediately instead
    // of waiting for the next sync_scene() pass.
    //
    // If Helio isn't initialized yet (first frame) the operation returns false
    // and sync_scene() will pick it up on the first ready frame.

    /// Set the gizmo mode (Translate, Rotate, Scale).
    pub fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        self.gizmo_dirty = true;
        if let Some(inner) = &mut self.inner {
            inner.editor_state.set_gizmo_mode(mode);
            tracing::info!("[HELIO] Gizmo mode set to: {:?}", mode);
        }
    }

    /// Get the currently selected object ID (Helio internal ID).
    pub fn get_selected_object(&self) -> Option<helio::SceneActorId> {
        self.inner.as_ref()?.editor_state.selected()
    }

    /// Get the SceneDb ID of the currently selected object.
    pub fn get_selected_scene_db_id(&self) -> Option<String> {
        use helio::SceneActorId;
        let inner = self.inner.as_ref()?;
        let tag = match inner.editor_state.selected()? {
            SceneActorId::Object(obj_id) => inner
                .renderer
                .scene()
                .iter_objects_for_editor()
                .find(|(id, _, _, _)| *id == obj_id)
                .map(|(_, _, _, t)| t)?,
            SceneActorId::Light(light_id) => inner
                .renderer
                .scene()
                .iter_lights()
                .find(|(id, _, _)| *id == light_id)
                .map(|(_, _, t)| t)?,
            _ => return None,
        };
        self.scene_store
            .read()
            .get_all_snapshots()
            .into_iter()
            .find(|snap| scene_id_to_tag(&snap.stable_id) == tag)
            .map(|snap| snap.stable_id)
    }

    /// Select an object or light by its SceneDb ID.
    pub fn select_by_scene_db_id(&mut self, scene_db_id: &str) -> bool {
        use helio::SceneActorId;
        self.gizmo_dirty = true;
        let Some(inner) = &mut self.inner else {
            return false;
        };
        let tag = scene_id_to_tag(scene_db_id);

        if let Some((obj_id, _, _, _)) = inner
            .renderer
            .scene()
            .iter_objects_for_editor()
            .find(|(_, _, _, t)| *t == tag)
        {
            inner.editor_state.select(SceneActorId::Object(obj_id));
            true
        } else if let Some((light_id, _, _)) = inner
            .renderer
            .scene()
            .iter_lights()
            .find(|(_, _, t)| *t == tag)
        {
            inner.editor_state.select(SceneActorId::Light(light_id));
            true
        } else {
            false
        }
    }

    /// Deselect the currently selected object.
    pub fn deselect(&mut self) {
        self.gizmo_dirty = true;
        if let Some(inner) = &mut self.inner {
            inner.editor_state.deselect();
            tracing::info!("[HELIO] Deselected");
        }
    }

    /// Request TAA history reset on the next rendered frame.
    pub fn reset_taa(&mut self) {
        self.reset_taa_next_frame = true;
    }

    /// Atomically select an object by SceneDb ID in both SceneDb and Helio EditorState.
    /// This ensures both systems are always in sync without needing a reconciliation loop.
    /// Returns true if the object was found and selected.
    pub fn select_object_atomic(&mut self, scene_db_id: Option<String>) -> bool {
        use helio::SceneActorId;

        // Mark gizmo dirty so the next rendered frame rebuilds gizmo geometry.
        self.gizmo_dirty = true;

        // First update SceneDb (single source of truth for object list)
        self.scene_store.write().select_object(scene_db_id.clone());

        // Then update Helio EditorState (for gizmo rendering)
        let Some(inner) = &mut self.inner else {
            return false;
        };

        if let Some(ref id) = scene_db_id {
            let tag = scene_id_to_tag(id);
            if let Some((obj_id, _, _, _)) = inner
                .renderer
                .scene()
                .iter_objects_for_editor()
                .find(|(_, _, _, t)| *t == tag)
            {
                inner.editor_state.select(SceneActorId::Object(obj_id));
                tracing::info!("[ATOMIC] Selected object: {}", id);
                true
            } else if let Some((light_id, _, _)) = inner
                .renderer
                .scene()
                .iter_lights()
                .find(|(_, _, t)| *t == tag)
            {
                inner.editor_state.select(SceneActorId::Light(light_id));
                tracing::info!("[ATOMIC] Selected light: {}", id);
                true
            } else {
                tracing::warn!("[ATOMIC] Actor not found for scene ID: {}", id);
                false
            }
        } else {
            // Deselect in both
            inner.editor_state.deselect();
            tracing::info!("[ATOMIC] Deselected");
            true
        }
    }

    /// Build a ray from normalized cursor position for object picking.
    /// `norm_x` and `norm_y` are in [0.0, 1.0] relative to the viewport.
    /// This is DPI-agnostic: both GPUI logical coords and physical pixels normalize the same way.
    fn build_pick_ray(&self, norm_x: f32, norm_y: f32) -> (Vec3, Vec3) {
        let (width, height) = self.viewport_size;
        // Convert normalized [0,1] to physical pixel coordinates that ray_from_screen expects.
        let cursor_x = norm_x * width as f32;
        let cursor_y = norm_y * height as f32;
        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let fwd = Vec3::new(sy * cp, sp, -cy * cp);
        let aspect = width as f32 / height.max(1) as f32;
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect, 0.1, 10_000.0);
        let view = Mat4::look_at_rh(self.cam_pos, self.cam_pos + fwd, Vec3::Y);
        let vp_inv = (proj * view).inverse();
        EditorState::ray_from_screen(cursor_x, cursor_y, width as f32, height as f32, vp_inv)
    }

    /// Handle left-click for object selection or gizmo dragging.
    /// `norm_x`/`norm_y` must be in [0.0, 1.0] relative to the viewport area.
    pub fn handle_left_click(&mut self, norm_x: f32, norm_y: f32) {
        self.gizmo_dirty = true;
        use helio::SceneActorId;
        let (ray_o, ray_d) = self.build_pick_ray(norm_x, norm_y);

        // Determine what to select (if anything) by doing raycast and lookup
        let selection_target: Option<Option<String>> = {
            let Some(inner) = &mut self.inner else { return };

            // Try to start gizmo drag first.
            if inner
                .editor_state
                .try_start_drag(ray_o, ray_d, inner.renderer.scene())
            {
                // Gizmo drag started - don't change selection
                None
            } else {
                // No gizmo hit — do object picking.
                if let Some(hit) = inner
                    .scene_picker
                    .cast_ray(inner.renderer.scene(), ray_o, ray_d)
                {
                    match hit.actor_id {
                        SceneActorId::Object(_) | SceneActorId::Light(_) => {
                            // Resolve SceneDb ID by scanning for matching user_tag.
                            let scene_db_id = self
                                .scene_store
                                .read()
                                .get_all_snapshots()
                                .into_iter()
                                .find(|snap| scene_id_to_tag(&snap.stable_id) == hit.user_tag)
                                .map(|snap| snap.stable_id);
                            Some(scene_db_id)
                        }
                        _ => {
                            inner.editor_state.select(hit.actor_id);
                            None
                        }
                    }
                } else {
                    // No hit - deselect
                    Some(None)
                }
            }
        };

        // Now apply the selection atomically (if needed)
        if let Some(target) = selection_target {
            self.select_object_atomic(target);
        }
    }

    /// Handle mouse movement for gizmo hover highlighting and dragging.
    /// `norm_x`/`norm_y` must be in [0.0, 1.0] relative to the viewport area.
    pub fn handle_mouse_move(&mut self, norm_x: f32, norm_y: f32) {
        self.gizmo_dirty = true;
        let (ray_o, ray_d) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };

        // Mirror demo exactly: update_hover is always called (updates gizmo axis highlighting);
        // update_drag is called additionally when a drag is active.
        inner
            .editor_state
            .update_hover(ray_o, ray_d, &inner.renderer);
        if inner.editor_state.is_dragging() {
            inner
                .editor_state
                .update_drag(ray_o, ray_d, &mut inner.renderer);
        }
    }

    /// Handle left-click release to end gizmo dragging.
    /// If a gizmo drag was active, reads the final transform back from the Helio
    /// scene and writes it to the SceneDb so properties panels stay in sync.
    pub fn handle_left_release(&mut self) {
        self.gizmo_dirty = true;
        let Some(inner) = &mut self.inner else { return };

        // Capture the selected actor before ending the drag so we can read its final state.
        let dragged_actor = if inner.editor_state.is_dragging() {
            inner.editor_state.selected()
        } else {
            None
        };

        inner.editor_state.end_drag();

        // Write the final gizmo position back to SceneDb for whichever actor type was dragged.
        if let Some(actor) = dragged_actor {
            use helio::SceneActorId;
            match actor {
                SceneActorId::Object(obj_id) => {
                    if let Ok(mat) = inner.renderer.scene().get_object_transform(obj_id) {
                        let (scale_v, quat, pos_v) = mat.to_scale_rotation_translation();
                        let (yaw, pitch, roll) = quat.to_euler(EulerRot::YXZ);
                        let tag = inner
                            .renderer
                            .scene()
                            .iter_objects_for_editor()
                            .find(|(id, _, _, _)| *id == obj_id)
                            .map(|(_, _, _, t)| t)
                            .unwrap_or(0);
                        if let Some(scene_id) = self
                            .scene_store
                            .read()
                            .get_all_snapshots()
                            .into_iter()
                            .find(|snap| scene_id_to_tag(&snap.stable_id) == tag)
                            .map(|snap| snap.stable_id)
                        {
                            self.scene_store.write().apply_transform(
                                &scene_id,
                                [pos_v.x, pos_v.y, pos_v.z],
                                [pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees()],
                                [scale_v.x, scale_v.y, scale_v.z],
                            );
                        }
                    }
                }
                SceneActorId::Light(light_id) => {
                    if let Some(gpu_light) = inner.renderer.scene().get_light(light_id) {
                        let pos = [
                            gpu_light.position_range[0],
                            gpu_light.position_range[1],
                            gpu_light.position_range[2],
                        ];
                        let tag = inner
                            .renderer
                            .scene()
                            .iter_lights()
                            .find(|(id, _, _)| *id == light_id)
                            .map(|(_, _, t)| t)
                            .unwrap_or(0);
                        if let Some(scene_id) = self
                            .scene_store
                            .read()
                            .get_all_snapshots()
                            .into_iter()
                            .find(|snap| scene_id_to_tag(&snap.stable_id) == tag)
                            .map(|snap| snap.stable_id)
                        {
                            self.scene_store.write().apply_transform(
                                &scene_id,
                                pos,
                                [0.0, 0.0, 0.0],
                                [1.0, 1.0, 1.0],
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        // Rebuild picker BVH after an object may have been moved by a drag.
        if let Some(inner) = &mut self.inner {
            inner.scene_picker.rebuild_instances(inner.renderer.scene());
        }
    }

    // ── Scene Setup ──────────────────────────────────────────────────────────

    fn populate_initial_scene(&self, inner: &mut HelioInner) {
        tracing::info!("[HELIO SCENE] Populating initial scene...");

        // Sky
        inner.renderer.scene_mut().insert_actor(SceneActor::Sky(
            SkyActor::new().with_sky_color([0.5, 0.7, 1.0]),
        ));
        tracing::info!("[HELIO SCENE] Added sky");

        // The HIDDEN group is always hidden — objects toggled invisible in the
        // editor are assigned to this group so they don't render visually while
        // remaining in the scene for gizmo rendering and selection.
        inner.renderer.scene_mut().hide_group(GroupId::new(8));

        // Lights and meshes are driven exclusively through SceneDb via sync_scene()
        // so that the hierarchy panel and the renderer always show the same state.
        tracing::info!(
            "[HELIO SCENE] Scene population complete (sky only; all objects driven by SceneDb)"
        );
    }

    fn sync_scene(
        scene_store: &Arc<RwLock<WorldSceneStore>>,
        inner: &mut HelioInner,
        error_queue: &Arc<Mutex<Vec<String>>>,
    ) {
        // Skip sync while the gizmo is actively dragging.
        if inner.editor_state.is_dragging() {
            return;
        }

        // Lights and objects are managed incrementally via `Scene::light_by_tag`/
        // `object_by_tag` (Pulsar-Native#561) -- each component looks up its
        // own existing Helio actor by tag and updates it in place, rather
        // than the scene wholesale-clearing and re-inserting everything on
        // every sync pass.

        // ── Component sync pass ───────────────────────────────────────────────
        // Phase 1: READ lock, scoped as tightly as possible (Pulsar-Native
        // drag-release freeze fix -- see the plan this landed from). Every
        // `store` call in this block is `&self` (`get_all_snapshots`,
        // `entity_for`, `world()`; `dispatch_world_component_for_class` also
        // only takes `&World`), so a shared read lock is all this needs --
        // it no longer blocks a concurrent `SceneDatabase` write from the UI
        // thread the way a write lock held for this whole pass used to.
        // Previously this was ONE write-lock guard held across this entire
        // function, specifically so the snapshot pull and the dirty-flag
        // drain at the very end shared one critical section instead of two
        // racing acquisitions -- but everything between them (mesh loading,
        // GPU resource creation, cache teardown, a render-graph rebuild, a
        // *nested* `script_registry` lock, a BVH rebuild) never actually
        // touched `store` at all, so holding the write lock across all of it
        // was pure incidental scope creep, not a real requirement. Dirty-flag
        // draining now happens in its own short Phase 2 write lock, below.
        let mut pending_seam_upserts: Vec<PendingStaticMeshSeamUpsert> = Vec::new();
        let (snapshots, mut live_keys) = {
            let store = scene_store.read();
            let t_snap = std::time::Instant::now();
            let snapshots = store.get_all_snapshots();
            let snap_ms = t_snap.elapsed().as_secs_f64() * 1000.0;
            if snap_ms > 2.0 {
                tracing::warn!("[SYNC_SCENE] get_all_snapshots took {:.2}ms", snap_ms);
            }
            let mut live_keys = LiveKeySet::new();
            let project_root = engine_state::get_project_path()
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let mut planet_runtime_init_attempted = inner.planet_terrain.is_some();

            // Process all snapshots through the component system regardless of
            // visibility so objects exist in the Helio scene for gizmo rendering
            // and selection picking.
            for snap in &snapshots {
                Self::sync_snapshot_components(
                    inner,
                    &store,
                    snap,
                    error_queue,
                    &project_root,
                    &mut planet_runtime_init_attempted,
                    &mut live_keys,
                    &mut pending_seam_upserts,
                );
            }
            (snapshots, live_keys)
        }; // read guard dropped here -- everything below is lock-free w.r.t. `scene_store`.

        // NOTE (Pulsar-Native#561): there used to be a "remove stale scene
        // objects" sweep here, keyed off `inner.object_cache` (a
        // `SceneObjectCache`, since deleted). It was already silently
        // non-functional before this cleanup -- `SceneObjectCache` was never
        // actually populated anywhere (`StaticMeshComponent::sync_component`
        // resolves objects via `scene.object_by_tag` instead, confirmed by
        // grep), so `.map.keys()` was always empty and this loop's body
        // never ran. Meaning: removing a `StaticMeshComponent` from an
        // object while leaving the object itself alive does NOT currently
        // remove its mesh from the Helio scene. This is a real, pre-existing
        // gap (not introduced by this cleanup, which only removes dead
        // scaffolding that was already a no-op) -- a correct fix needs an
        // `object_by_tag`-based staleness check instead of a cache, tracked
        // as a follow-up rather than attempted here.

        // Remove stale foliage component instances (components didn't touch them
        // this pass): the cached type/layer/interactor/material are all torn down.
        let stale_foliage: Vec<String> = inner
            .foliage_cache
            .map
            .keys()
            .filter(|key| !live_keys.contains(*key))
            .cloned()
            .collect();
        for key in stale_foliage {
            remove_foliage_handles(inner.renderer.scene_mut(), &mut inner.foliage_cache, &key);
        }

        // NOTE (Pulsar-Native#561): same story as the object-cache sweep
        // above -- `LightCache` (since deleted) was never actually
        // populated (`LightComponent::sync_component` resolves via
        // `scene.light_by_tag` instead), so this was already a silent
        // no-op. Same pre-existing gap, same follow-up needed.

        // Drop stale portal sides (their object deleted, or their
        // PortalComponent removed/disabled while the object stayed —
        // PortalComponent itself handles the disabled-but-still-attached
        // case). A side disappearing tears down the real portal if the pair
        // was complete; the surviving side (if any) just waits for a new
        // partner.
        for action in inner.portal_link_cache.remove_stale(&live_keys) {
            if let Some((portal_id, id)) = apply_portal_pair_action(inner.renderer.scene_mut(), action) {
                match id {
                    Some(id) => inner.portal_link_cache.set_active(portal_id, id),
                    None => inner.portal_link_cache.clear_active(portal_id),
                }
            }
        }

        if let Some(planet_terrain) = inner.planet_terrain.as_mut() {
            if let Err(error) = planet_terrain.remove_stale_components(&live_keys) {
                let message = format!("Failed to remove stale planet terrain components: {error}");
                tracing::error!("{message}");
                if let Ok(mut errors) = error_queue.lock() {
                    errors.push(message);
                }
            }
        }

        Self::sync_planet_graph(inner, error_queue);
        if inner
            .planet_terrain
            .as_ref()
            .is_some_and(|runtime| !runtime.has_active_components())
        {
            inner.planet_terrain = None;
        }

        // Apply editor visibility: hidden objects remain in the Helio scene
        // (for gizmo rendering and selection picking) but are assigned to the
        // HIDDEN group so they don't render visually.
        //
        // `object_by_tag`, not the now-deleted `SceneObjectCache` -- that
        // cache was never actually populated anywhere in the codebase
        // (confirmed: no `.insert()` call existed), so this loop was a
        // silent no-op before this fix, in both this full-sync pass AND
        // `sync_scene_delta`'s own equivalent (`apply_visibility_patch`,
        // which uses the same `object_by_tag` resolution for consistency).
        for snap in &snapshots {
            let tag = scene_id_to_tag(snap.stable_id.as_str());
            if let Some(obj_id) = inner.renderer.scene().object_by_tag(tag) {
                let groups = if snap.visibility.visible {
                    GroupMask::NONE
                } else {
                    GroupMask::from(GroupId::new(8))
                };
                let _ = inner.renderer.scene_mut().set_object_groups(obj_id, groups);
            }
        }

        // Cull script registrations for objects no longer in the scene.
        let registry = script_registry();
        registry.write().retain_keys(live_keys.inner());

        // Rebuild scene picker BVH after any insertions or removals.
        let t_picker = std::time::Instant::now();
        inner.scene_picker.rebuild_instances(inner.renderer.scene());
        let picker_ms = t_picker.elapsed().as_secs_f64() * 1000.0;
        if picker_ms > 2.0 {
            tracing::warn!("[SYNC_SCENE] picker rebuild took {:.2}ms", picker_ms);
        }

        // Phase 2: short WRITE lock, the only part of this whole function
        // that genuinely needs `&mut WorldSceneStore`.
        //
        // Full sync just brought every object in `live_keys` fully up to
        // date from its snapshot, so clear their dirty flags here — full
        // sync never marked them dirty in the first place (that only
        // happens via WorldSceneStore::mark_dirty / a fresh spawn), but it
        // must still consume any that accumulated, or the delta-sync path
        // would see them as still needing work it just did and redo it
        // every frame until something happened to touch drain_dirty().
        //
        // Scoped to `live_keys` (this pass's snapshot) rather than a global
        // `store.drain_dirty()`, matching the pre-B1 behavior. Note this is
        // now a genuinely separate lock acquisition from Phase 1's read lock
        // above (no guard held across both), so in principle another writer
        // could interleave between them -- accepted, and harmless here: the
        // only other writers of dirty flags are `WorldSceneStore`'s own
        // mutation methods reacting to real edits, and any such edit that
        // lands in this narrow window just gets its dirty flag cleared one
        // pass later than it otherwise would (picked up by the very next
        // `sync_scene`/`sync_scene_delta`), not lost. Kept scoped to
        // `live_keys` anyway rather than switching to a global drain, since
        // that's a distinct, unrelated behavior change (it would also clear
        // dirty flags for objects this pass never actually synced to Helio,
        // which isn't what "full sync completed" should mean) and not
        // something this migration set out to change.
        {
            let mut store = scene_store.write();
            Self::apply_pending_seam_upserts(&mut store, inner, pending_seam_upserts);
        }
    }

    /// Applies every [`PendingStaticMeshSeamUpsert`] queued by
    /// `sync_snapshot_components`'s read-locked pass -- the one part of that
    /// pass that genuinely needs `&mut WorldSceneStore`, shared by
    /// `sync_scene`'s and `sync_scene_delta`'s own Phase 2 write locks
    /// (same reasoning as the dirty-flag drain each already does there).
    /// `is_alive` guarded: `World::insert` panics on a dead entity, and the
    /// entity could in principle have been despawned in the narrow window
    /// between Phase 1's read lock ending and this write lock starting
    /// (same accepted race every other cross-phase `store` read/write in
    /// this file already tolerates).
    ///
    /// Also drives `SceneDb::step()` + `HelioRenderSubsystem::apply_to`
    /// (Pulsar-Native#561 Phase D) -- the documented per-frame call sequence
    /// (`helio-scenedb/src/subsystem.rs`'s module doc) -- every call, not
    /// just when `pending` is non-empty: `step()`'s `simulate_b` drains
    /// `World`'s change tracker for ANY `RenderTransform`/`StaticMeshComponent`/
    /// etc mutation since the last call, not only ones this exact pass's
    /// `pending` list produced (e.g. a `RenderTransform` touched by some
    /// other future writer). Called with `store`'s write lock already held
    /// -- `register_subsystem`/`step` both only need `&mut SceneDb`, which
    /// is reached the same way `world_mut()` is.
    fn apply_pending_seam_upserts(
        store: &mut WorldSceneStore,
        inner: &mut HelioInner,
        pending: Vec<PendingStaticMeshSeamUpsert>,
    ) {
        {
            let world = store.world_mut();
            for upsert in pending {
                if !world.is_alive(upsert.entity) {
                    continue;
                }
                world.insert(
                    upsert.entity,
                    helio_scenedb::StaticMeshComponent::new(upsert.mesh, upsert.material),
                );
                world.insert(upsert.entity, helio_scenedb::RenderTransform(upsert.transform));
                world.insert(upsert.entity, helio_scenedb::RenderBounds(upsert.bounds));
                world.insert(
                    upsert.entity,
                    helio_scenedb::RenderFlags {
                        flags: 0,
                        movability: Some(Movability::Movable),
                        groups: GroupMask::NONE,
                    },
                );
            }
        }

        let delta = store.take_last_delta();
        let scene_db = store.scene_db_mut();
        if let Some(subsystem) = scene_db.subsystem_mut::<HelioRenderSubsystem>() {
            if let Some(delta) = delta {
                subsystem.push_delta(delta);
            }
        }
        scene_db.step();
        if let Some(subsystem) = scene_db.subsystem_mut::<HelioRenderSubsystem>() {
            subsystem.apply_to(inner.renderer.scene_mut());
        }
    }

    /// Dispatches one snapshot's components into Helio -- the same
    /// `dispatch_world_component_for_class`/`apply_runtime_behavior_for_class`
    /// call `sync_scene`'s full pass has always made, factored out so
    /// `sync_scene_delta` can invoke it too (Pulsar-Native#561: the steady-state
    /// per-frame path previously never dispatched components at all -- see that
    /// fn's own doc). `store` only needs `&self` for the duration of this call
    /// (`entity_for`, `world()`); callers hold whatever lock (read is enough)
    /// gets them that reference.
    fn sync_snapshot_components(
        inner: &mut HelioInner,
        store: &WorldSceneStore,
        snap: &crate::scene::ObjectSnapshot,
        error_queue: &Arc<Mutex<Vec<String>>>,
        project_root: &Path,
        planet_runtime_init_attempted: &mut bool,
        live_keys: &mut LiveKeySet,
        pending_seam_upserts: &mut Vec<PendingStaticMeshSeamUpsert>,
    ) {
        let owner = RuntimeComponentOwner {
            scene_object_id: snap.stable_id.as_str(),
            position: snap.transform.position,
            rotation: snap.transform.rotation,
            scale: snap.transform.scale,
            props: &snap.render_props.props,
        };

        let component_instances = component_instances_from_snap(snap);
        // Captured before the dispatch loop below consumes `component_instances`
        // -- read back out afterward. `Some(None)` means a StaticMeshComponent
        // instance exists but its `mesh_asset` is empty/missing (surfaces the
        // same diagnostic `StaticMeshComponent::sync_component` used to,
        // before its body became a no-op -- Pulsar-Native#561 Phase D
        // cutover); `Some(Some(path))` is the real case; `None` means no
        // StaticMeshComponent instance on this object at all.
        let static_mesh_component_data: Option<Option<String>> = component_instances
            .iter()
            .find(|(_, class_name, _)| class_name == "StaticMeshComponent")
            .map(|(_, _, data)| {
                data.get("mesh_asset")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            });
        let needs_planet_runtime = component_instances.iter().any(|(_, class_name, data)| {
            class_name == PLANET_TERRAIN_CLASS_NAME
                && data
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true)
        });
        if needs_planet_runtime && inner.planet_terrain.is_none() && !*planet_runtime_init_attempted
        {
            *planet_runtime_init_attempted = true;
            match PlanetTerrainRuntime::new() {
                Ok(runtime) => inner.planet_terrain = Some(runtime),
                Err(error) => {
                    let message = format!("Planet terrain runtime initialization failed: {error}");
                    tracing::error!("{message}");
                    if let Ok(mut errors) = error_queue.lock() {
                        errors.push(message);
                    }
                }
            }
        }
        let mut subsystems = Subsystems::new();
        subsystems.register_ref::<Renderer>(&mut inner.renderer);
        subsystems.register_ref::<MeshCache>(&mut inner.mesh_cache);
        subsystems.register_ref::<FoliageCache>(&mut inner.foliage_cache);
        subsystems.register_ref::<PortalLinkCache>(&mut inner.portal_link_cache);
        if let Some(planet_terrain) = inner.planet_terrain.as_mut() {
            let (runtime, cache) = planet_terrain.component_context_mut();
            subsystems.register_ref(runtime);
            subsystems.register_ref(cache);
        }
        subsystems.register_ref::<LiveKeySet>(live_keys);
        let mut ctx = HelioRuntimeContext {
            renderer: &mut inner.renderer,
            subsystems,
            error_queue,
            project_root,
        };

        // Phase B4/B5 (Pulsar-Native#555/#556): a class registered with
        // `#[register_world_component]` dispatches directly off the
        // typed value `SceneDatabase` already hydrated into `World` --
        // no `serde_json::from_value` on this hot path. Falls back to
        // the JSON dispatch below for anything not yet migrated (most of
        // B5's list, at time of writing), or in the unexpected case
        // hydration didn't happen for some reason -- fails safe rather
        // than silently dropping the object's rendering.
        let entity = store.entity_for(snap.stable_id.as_str());
        for (component_index, class_name, data) in component_instances {
            if let Some(entity) = entity {
                if pulsar_world_registry::dispatch_world_component_for_class(
                    class_name.as_str(),
                    store.world(),
                    entity,
                    &owner,
                    component_index,
                    &mut ctx,
                ) {
                    continue;
                }
            }
            let _ = apply_runtime_behavior_for_class(
                class_name.as_str(),
                &owner,
                component_index,
                &data,
                &mut ctx,
            );
        }
        // `ctx`/`subsystems` (holding `&mut inner.mesh_cache`/`&mut inner.renderer`)
        // are done as of the loop above (NLL already treats this borrow as
        // ended at its last use inside the loop) -- dropped explicitly so the
        // direct `inner.mesh_cache`/`inner.renderer` access just below is
        // unambiguously a fresh borrow, not relying on an implicit
        // end-of-borrow inference.
        drop(ctx);

        // GPU-native seam (Pulsar-Native#561 Phase D): queue this entity's
        // StaticMeshComponent data for `World` insertion. The ONLY mesh-
        // resolution path left as of the Phase D cutover -- `StaticMeshComponent::
        // sync_component`'s own load-on-miss logic was deleted, not just
        // shadowed (see that fn's doc) -- so this one loads on a cache miss
        // itself now, same as that fn used to.
        match (entity, static_mesh_component_data) {
            (Some(entity), Some(Some(mesh_asset))) => {
                Self::queue_static_mesh_seam_upsert(
                    inner,
                    entity,
                    &owner,
                    &mesh_asset,
                    project_root,
                    error_queue,
                    pending_seam_upserts,
                );
            }
            (_, Some(None)) => {
                // A StaticMeshComponent instance exists but has no
                // mesh_asset -- same diagnostic `sync_component` used to
                // report (deduped the same way, so this doesn't spam every
                // dirty pass while the object stays meshless).
                if !Self::already_reported_empty_mesh_asset(snap.stable_id.as_str()) {
                    let message = format!(
                        "StaticMeshComponent on '{}' has no mesh_asset",
                        snap.stable_id
                    );
                    tracing::warn!("{message}");
                    if let Ok(mut errors) = error_queue.lock() {
                        errors.push(message);
                    }
                }
            }
            _ => {}
        }
    }

    /// Same one-report-per-(object,state) de-dup shape
    /// `StaticMeshComponent::sync_component` used to have internally (a
    /// `static Mutex<HashMap<...>>`) -- kept local to `engine_backend`
    /// rather than exposing `helio_component`'s private log, since this is
    /// now the only place reporting this specific diagnostic.
    fn already_reported_empty_mesh_asset(scene_object_id: &str) -> bool {
        static LOG: std::sync::LazyLock<Mutex<HashSet<String>>> =
            std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));
        let Ok(mut seen) = LOG.lock() else { return false };
        !seen.insert(scene_object_id.to_string())
    }

    /// Same hardcoded default `GpuMaterial` `StaticMeshComponent::
    /// sync_component` used to mint on a cache miss, before its body became
    /// a no-op (Pulsar-Native#561 Phase D cutover) -- `StaticMeshComponent`
    /// has no material fields of its own yet (per-instance materials are
    /// separate, later scope), so this is a faithful, no-behavior-change
    /// carry-over of what was already rendered, not a new default.
    fn default_seam_material() -> GpuMaterial {
        GpuMaterial {
            base_color: [0.22, 0.15, 0.08, 1.0],
            emissive: [0.0, 0.0, 0.0, 0.0],
            roughness_metallic: [0.7, 0.0, 1.5, 0.5],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal: GpuMaterial::NO_TEXTURE,
            tex_roughness: GpuMaterial::NO_TEXTURE,
            tex_emissive: GpuMaterial::NO_TEXTURE,
            tex_occlusion: GpuMaterial::NO_TEXTURE,
            workflow: 0,
            flags: 0,
            material_class: 0,
            class_params: [0.0; 4],
        }
    }

    /// Resolves `mesh_asset` to a `MeshId` (via `inner.mesh_cache`, loading
    /// and uploading on a miss -- the ONLY mesh-loading path left as of the
    /// Phase D cutover, `StaticMeshComponent::sync_component`'s own copy was
    /// deleted, not just shadowed) and queues a [`PendingStaticMeshSeamUpsert`].
    /// Still populates `inner.mesh_cache` on a miss (mints a pool `MaterialId`
    /// too, matching that cache's established `(MeshId, MaterialId)` shape --
    /// other consumers besides this seam still read it) so a re-sync of the
    /// same asset path is a cache hit, same caching contract the legacy path
    /// upheld.
    fn queue_static_mesh_seam_upsert(
        inner: &mut HelioInner,
        entity: pulsar_scenedb::Entity,
        owner: &RuntimeComponentOwner,
        mesh_asset: &str,
        project_root: &Path,
        error_queue: &Arc<Mutex<Vec<String>>>,
        pending: &mut Vec<PendingStaticMeshSeamUpsert>,
    ) {
        let abs_path = resolve_asset_path(project_root, mesh_asset)
            .to_string_lossy()
            .replace('\\', "/");

        let mesh_id = if let Some((mesh_id, _material_id)) = inner.mesh_cache.get(&abs_path) {
            mesh_id
        } else {
            let path = std::path::Path::new(&abs_path);
            let Some(upload) = load_mesh_upload(path) else {
                if !Self::already_reported_mesh_load_failure(owner.scene_object_id, &abs_path) {
                    let message = format!(
                        "StaticMeshComponent on '{}': failed to load '{}'",
                        owner.scene_object_id, abs_path
                    );
                    tracing::warn!("[SEAM] {message}");
                    if let Ok(mut errors) = error_queue.lock() {
                        errors.push(message);
                    }
                }
                return;
            };
            let Some(mesh_id) = inner.renderer.scene_mut().insert_actor(SceneActor::mesh(upload)).as_mesh()
            else {
                tracing::warn!("[SEAM] insert_actor returned no mesh id for {abs_path}");
                return;
            };
            let material_id = inner.renderer.scene_mut().insert_material(Self::default_seam_material());
            inner.mesh_cache.insert(abs_path.clone(), (mesh_id, material_id));
            mesh_id
        };

        let q = Quat::from_euler(
            EulerRot::YXZ,
            owner.rotation[1].to_radians(),
            owner.rotation[0].to_radians(),
            owner.rotation[2].to_radians(),
        );
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::from_array(owner.scale),
            q,
            Vec3::from_array(owner.position),
        );
        let pos = transform.w_axis.truncate();
        let radius = Vec3::from_array(owner.scale).length() * 0.5;

        pending.push(PendingStaticMeshSeamUpsert {
            entity,
            mesh: mesh_id,
            material: Self::default_seam_material(),
            transform,
            bounds: [pos.x, pos.y, pos.z, radius.max(0.1)],
        });
    }

    /// Same one-report-per-(object,asset) de-dup shape
    /// `StaticMeshComponent::sync_component` used to have internally.
    fn already_reported_mesh_load_failure(scene_object_id: &str, abs_path: &str) -> bool {
        static LOG: std::sync::LazyLock<Mutex<HashMap<String, String>>> =
            std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
        let Ok(mut map) = LOG.lock() else { return false };
        match map.get(scene_object_id) {
            Some(prev) if prev == abs_path => true,
            _ => {
                map.insert(scene_object_id.to_string(), abs_path.to_string());
                false
            }
        }
    }

    fn sync_planet_graph(inner: &mut HelioInner, error_queue: &Arc<Mutex<Vec<String>>>) {
        let wants_planet_graph = inner
            .planet_terrain
            .as_ref()
            .is_some_and(PlanetTerrainRuntime::has_active_components);
        let has_planet_graph = inner
            .planet_terrain
            .as_ref()
            .is_some_and(|runtime| runtime.renderer_ready(&inner.renderer));
        if wants_planet_graph == has_planet_graph {
            return;
        }

        let renderer_config = inner.renderer.renderer_config();
        let debug_state = inner.renderer.debug_state();
        let graph = if wants_planet_graph {
            helio_default_graphs::build_default_graph_external_with_planetary_voxels(
                &inner.device,
                &inner.queue,
                inner.renderer.scene(),
                renderer_config,
                debug_state,
                inner.renderer.debug_camera_buf(),
                inner.renderer.cull_stats_buf(),
                None,
                PlanetTerrainRuntime::renderer_config(),
            )
            .map_err(|error| error.to_string())
        } else {
            Ok(helio_default_graphs::build_default_graph_external(
                &inner.device,
                &inner.queue,
                inner.renderer.scene(),
                renderer_config,
                debug_state,
                inner.renderer.debug_camera_buf(),
                inner.renderer.cull_stats_buf(),
                None,
            ))
        };

        match graph {
            Ok(graph) => {
                inner.renderer.set_graph(graph);
                inner.planet_graph_rebuilt = wants_planet_graph;
            }
            Err(error) => {
                let message = format!("Failed to configure planetary render graph: {error}");
                tracing::error!("{message}");
                if let Ok(mut errors) = error_queue.lock() {
                    errors.push(message);
                }
            }
        }
    }

    /// Steady-state per-frame sync path -- everything after the very first
    /// `sync_scene` full pass (or a `force_full_resync()`) goes through here
    /// instead. Pulsar-Native#561: this function used to compute `added`/
    /// `updated` and then discard them (the return value was never assigned
    /// at the `render_frame` call site), and never inspected `flags` at all
    /// -- so no per-frame change of ANY kind (not transform, not visibility,
    /// not component data) actually reached `helio::Scene` after the first
    /// frame. Fixed here: entities whose dirty flags include `COMPONENTS`/
    /// `PROPS` (or that are new since the last pass) get the same full
    /// per-component dispatch `sync_scene`'s full pass has always used
    /// (`sync_snapshot_components` -- the same registered `sync_component`
    /// translations, e.g. `LightComponent::to_gpu_light`, now actually run
    /// continuously instead of only once). A `TRANSFORM`-only or
    /// `VISIBILITY`-only change on an already-known entity takes a cheaper
    /// direct-patch path instead of a full re-dispatch. Removed entities now
    /// actually get removed from `helio::Scene` too (previously only
    /// `known_ids` bookkeeping happened; the actor lingered until the next
    /// full resync).
    fn sync_scene_delta(
        scene_store: &Arc<RwLock<WorldSceneStore>>,
        inner: &mut HelioInner,
        error_queue: &Arc<Mutex<Vec<String>>>,
    ) -> SceneDbDelta {
        let mut store_write = scene_store.write();
        let revision = store_write.dirty_gen();
        let dirty = store_write.drain_dirty();
        let removed = store_write.take_removed_ids();

        let mut added = Vec::new();
        let mut updated = Vec::new();
        let anything_changed = !dirty.is_empty() || !removed.is_empty();
        let mut pending_seam_upserts: Vec<PendingStaticMeshSeamUpsert> = Vec::new();

        // Phase 1: downgrade to READ lock atomically. This prevents concurrent
        // structural modifications (e.g. entities deleted by another thread)
        // between Phase 0 and Phase 1, eliminating race conditions while
        // still allowing other systems to read the scene during GPU dispatch.
        if !dirty.is_empty() {
            let store = parking_lot::RwLockWriteGuard::downgrade(store_write);
            let project_root = engine_state::get_project_path()
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let mut planet_runtime_init_attempted = inner.planet_terrain.is_some();

            for (entity_ref, flags) in &dirty {
                let entity = *entity_ref;
                let Some(id_str) = store.stable_id_of(entity) else {
                    continue;
                };
                let is_known = inner.known_ids.contains(id_str);

                let transform = store.transform(entity).unwrap_or_default();
                let visibility = store.visibility(entity).unwrap_or_default();

                if !is_known || flags.intersects(ObjectDirtyFlags::COMPONENTS | ObjectDirtyFlags::PROPS)
                {
                    // Full per-component dispatch (heavy path, requires full snapshot)
                    let Some(snap) = store.get_object(id_str) else { continue; };
                    let mut live_keys = LiveKeySet::new();
                    Self::sync_snapshot_components(
                        inner,
                        &store,
                        &snap,
                        error_queue,
                        &project_root,
                        &mut planet_runtime_init_attempted,
                        &mut live_keys,
                        &mut pending_seam_upserts,
                    );
                } else {
                    // Fast path: Zero-copy transform/visibility sync.
                    // Avoids cloning the entire RenderProps payload when only moving objects.
                    if flags.contains(ObjectDirtyFlags::TRANSFORM) {
                        Self::apply_transform_patch_direct(inner, id_str, &transform);
                    }

                    if flags.contains(ObjectDirtyFlags::VISIBILITY) {
                        Self::apply_visibility_patch_direct(inner, id_str, &visibility);
                    }
                }

                if is_known {
                    updated.push(ObjectUpdate {
                        id: id_str.to_string(),
                        transform: Some(build_transform_parts(
                            transform.position,
                            transform.rotation,
                            transform.scale,
                        )),
                        visible: Some(visibility.visible),
                        name: None,
                    });
                } else {
                    added.push(id_str.to_string());
                }
            }
        } else {
            drop(store_write);
        } // read guard dropped here -- everything below is lock-free w.r.t. `scene_store`.

        // Phase 2: short WRITE lock -- same reasoning as `sync_scene`'s own
        // Phase 2 (the seam upserts are the only `&mut WorldSceneStore` work
        // Phase 1 above produced). No-op (lock acquired, immediately
        // released) when nothing was queued, e.g. no dirty `StaticMeshComponent`
        // this pass.
        if !pending_seam_upserts.is_empty() {
            let mut store = scene_store.write();
            Self::apply_pending_seam_upserts(&mut store, inner, pending_seam_upserts);
        }

        // Removed entities: actually tear down their Helio-side actor now,
        // rather than leaving it to linger until the next full resync.
        for id in &removed {
            let tag = scene_id_to_tag(id.as_str());
            let scene = inner.renderer.scene_mut();
            if let Some(obj_id) = scene.object_by_tag(tag) {
                let _ = scene.remove_object(obj_id);
            } else if let Some(light_id) = scene.light_by_tag(tag) {
                let _ = scene.remove_light(light_id);
            }
            inner.known_ids.remove(id);
        }
        for id in &added {
            inner.known_ids.insert(id.clone());
        }

        if anything_changed {
            inner.scene_picker.rebuild_instances(inner.renderer.scene());
        }

        SceneDbDelta {
            added,
            removed,
            updated,
            revision,
        }
    }

    /// Cheap fast path for a `TRANSFORM`-only change on an entity already
    /// known to Helio -- skips the full per-component re-dispatch.
    /// Uses zero-copy Transform directly instead of ObjectSnapshot.
    fn apply_transform_patch_direct(inner: &mut HelioInner, stable_id: &str, transform: &crate::scene::Transform) {
        let tag = scene_id_to_tag(stable_id);
        let helio_transform = build_transform_parts(
            transform.position,
            transform.rotation,
            transform.scale,
        );
        let scene = inner.renderer.scene_mut();
        if let Some(obj_id) = scene.object_by_tag(tag) {
            let _ = scene.update_object_transform(obj_id, helio_transform);
        } else if let Some(light_id) = scene.light_by_tag(tag) {
            if let Some(mut light) = scene.get_light(light_id) {
                light.position_range[0] = transform.position[0];
                light.position_range[1] = transform.position[1];
                light.position_range[2] = transform.position[2];
                let _ = scene.update_light(light_id, light);
            }
        }
    }

    /// Cheap fast path for a `VISIBILITY`-only change.
    /// Uses zero-copy Visibility directly instead of ObjectSnapshot.
    fn apply_visibility_patch_direct(inner: &mut HelioInner, stable_id: &str, visibility: &crate::scene::Visibility) {
        let tag = scene_id_to_tag(stable_id);
        let scene = inner.renderer.scene_mut();
        if let Some(obj_id) = scene.object_by_tag(tag) {
            let groups = if visibility.visible {
                GroupMask::NONE
            } else {
                GroupMask::from(GroupId::new(8))
            };
            let _ = scene.set_object_groups(obj_id, groups);
        }
    }
}
