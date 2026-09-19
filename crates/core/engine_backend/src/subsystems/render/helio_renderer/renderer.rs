//! Main HelioRenderer — wgpu + Helio scene renderer backed by SceneDB.

use glam::{Mat4, Vec3};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use helio::{Camera, Renderer, RendererConfig};
use helio_component::{PlanetTerrainFrameInput, PlanetTerrainRuntime};

use super::core::{CameraInput, GpuProfilerData, RenderMetrics, RenderSpikeLogConfig};
use crate::scene::{GizmoType, SceneWorldExt};
use crate::services::terrain_edit::TerrainEditMailbox;

use super::interaction::SceneInteraction;
type GizmoMode = GizmoType;

/// Camera velocity squared below this threshold is considered stopped.
const CAMERA_IDLE_EPSILON: f32 = 0.001;

// ── Compatibility types retained for existing UI wiring ───────────────────────

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
/// Pointer events are serialized with rendering so SceneDB-backed
/// interaction state observes click and release in order. The render thread
/// drains this queue before it evaluates the frame path, which keeps transient
/// drag state independent from the GPU engine mutex.
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
/// cheap mailbox state) -- never `gpu_engine` -- so none of them can block on
/// the render thread's
/// per-frame lock hold at all.
#[derive(Clone)]
pub struct HelioEditorMailbox {
    pending_gizmo_mode: Arc<Mutex<Option<GizmoMode>>>,
    pending_deselect: Arc<AtomicBool>,
    pending_force_full_resync: Arc<AtomicBool>,
}

impl HelioEditorMailbox {
    /// Queue the SceneDB interaction gizmo mode for the render thread to apply
    /// at the next frame boundary.
    pub fn queue_gizmo(&self, mode: GizmoMode) {
        if let Ok(mut guard) = self.pending_gizmo_mode.lock() {
            *guard = Some(mode);
        }
    }

    /// Request that the SceneDB selection is cleared next frame.
    pub fn queue_deselect(&self) {
        self.pending_deselect.store(true, Ordering::Relaxed);
    }

    /// Request a fresh SceneDB step at the start of the next render frame.
    /// See HelioRenderer::pending_force_full_resync's doc for why this
    /// must never be silently dropped.
    pub fn queue_force_full_resync(&self) {
        self.pending_force_full_resync
            .store(true, Ordering::Relaxed);
    }
}

/// Render-frame marker retained for the editor-ui integration point.
///
/// The custom instrumentation stream has a single owner: the flamegraph
/// collector. This type must remain a no-op with respect to profiling state;
/// render-frame code must never drain or toggle the global collector.
#[cfg(feature = "editor-ui")]
struct WgpuiProfileBridge;

#[cfg(feature = "editor-ui")]
impl WgpuiProfileBridge {
    fn begin() -> Self {
        // The flamegraph collector is the sole owner of the custom
        // instrumentation stream. This render-frame bridge intentionally does
        // not enable, disable, or drain it: doing so from the render thread
        // races the collector and takes the profiling store lock during GPU
        // submission.
        Self
    }
}

#[cfg(feature = "editor-ui")]
impl Drop for WgpuiProfileBridge {
    fn drop(&mut self) {}
}

// ── HelioRenderer ─────────────────────────────────────────────────────────────

/// Main renderer coordinating Helio 3D rendering with GPUI.
pub struct HelioRenderer {
    // ── Scene & Input ──
    pub camera_input: Arc<Mutex<CameraInput>>,
    pub scene_store: crate::scene::SharedScene,

    // ── Legacy (unused) ──
    pub command_sender: mpsc::Sender<RendererCommand>,
    pub command_receiver: mpsc::Receiver<RendererCommand>,

    // ── Pending editor commands (written by UI thread, read by render thread) ──
    /// Next gizmo mode to apply; consumed at start of render_frame.
    pub pending_gizmo_mode: Arc<Mutex<Option<GizmoMode>>>,
    /// When true, the render thread should clear SceneDB selection next frame.
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

    /// Frame-boundary mailbox for the level editor's terrain tool mode: the
    /// scene's planet definitions and the brush ring come in, the canonical
    /// `TerrainRuntimeHandle` goes out. See `services::terrain_edit`.
    terrain: TerrainEditMailbox,

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
    interaction: SceneInteraction,
    /// Transient execution state for the optional planetary pass.
    planet_terrain: Option<PlanetTerrainRuntime>,
    planet_graph_rebuilt: bool,
    /// Frame-pacing revision; never used as a renderer-side world mirror.
    last_scene_revision: u64,
    has_rendered_frame: bool,
}

impl HelioRenderer {
    pub fn new(scene_store: crate::scene::SharedScene) -> Self {
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
            terrain: TerrainEditMailbox::new(),
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
        // When the Inspector capture is active, put the engine-side render
        // phases into the same bounded WGPUI timeline as Window::draw. The
        // cfg keeps the renderer usable without the optional editor UI, and
        // the macro is a single relaxed capture check when idle.
        #[cfg(feature = "editor-ui")]
        let _wgpui_profile_bridge = WgpuiProfileBridge::begin();
        #[cfg(feature = "editor-ui")]
        gpui::flamegraph_span!("pulsar: HelioRenderer::render_frame");
        profiling::set_track_name("Helio Render");
        profiling::profile_scope!("helio_frame");
        let frame_start = Instant::now();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.frame_count += 1;
        self.profiler_frame_counter += 1;

        // ── Lazy init (first frame only) ────────────────────────────────────────
        if self.inner.is_none() {
            #[cfg(feature = "editor-ui")]
            gpui::flamegraph_span!("pulsar: HelioRenderer::lazy_init");
            tracing::info!("Initializing Helio renderer...");

            let device_arc = Arc::new(_device.clone());
            let queue_arc = Arc::new(_queue.clone());
            let config = RendererConfig::new(width, height, format);
            // ── project/streaming → renderer translation (Helio#238 §5) ────
            // The canonical keys live in pulsar_settings' streaming schema;
            // this is the only place the engine hands them to Helio. The
            // matching SceneDB budget lands in attach_gpu_render_seam's ONE
            // configure_tiers call below.
            let streaming_int = |key: &str| -> Option<i64> {
                match engine_state::settings::global_config().get(
                    engine_state::settings::NS_PROJECT,
                    "streaming",
                    key,
                ) {
                    Ok(engine_state::settings::ConfigValue::Int(i)) => Some(i),
                    _ => None,
                }
            };
            let vt_enabled = matches!(
                engine_state::settings::global_config().get(
                    engine_state::settings::NS_PROJECT,
                    "streaming",
                    "virtual_texturing_enabled",
                ),
                Ok(engine_state::settings::ConfigValue::Bool(true))
            );
            let pool_mb = streaming_int("texture_stream_pool_mb")
                .unwrap_or(512)
                .clamp(64, 16384) as u32;
            let tile_px = streaming_int("virtual_texture_tile_size")
                .and_then(|s| u32::try_from(s).ok())
                .unwrap_or(128)
                .max(16);
            // Streaming stays OFF unless the canonical toggle says otherwise:
            // defaults must preserve today's behavior exactly.
            //
            // ── SceneDB GPU-native render seam (Pulsar-Native#561 Phase D,
            // shared with the play-mode renderers via #637's helio_bridge)
            // ──────────────────────────────────────────────────────────
            // The GPU mirror MUST be attached before the renderer is
            // constructed: `RendererBuilder::new` requires a `SceneDbHandle`
            // up front (SceneDB is the sole scene authority — there is no
            // valid renderer configuration without one), so this can no
            // longer be a post-construction `&mut Renderer` step. Idempotent
            // inside the bridge (a second renderer sharing the same
            // `scene_store`, e.g. another viewport, gets back the same
            // mirror instead of clobbering it).
            let scene_db_handle = crate::scene::ensure_gpu_mirror(
                &mut self.scene_store.write(),
                device_arc.clone(),
                queue_arc.clone(),
            );
            let mut builder = helio::RendererBuilder::new(config, scene_db_handle.clone())
                .with_external_device()
                .with_editor_mode(true)
                .with_clear_color([0.15, 0.18, 0.25, 1.0])
                .with_ambient([0.0, 0.0, 0.0], 0.0)
                .with_vt_tile_size(tile_px);
            if vt_enabled {
                builder = builder.with_texture_streaming(pool_mb);
            }
            let r = builder
                .with_pass_build_context(Box::new(
                    helio_default_graphs::build_default_graph_external_with_context,
                ))
                .build(device_arc.clone(), queue_arc.clone(), width, height, format);

            let inner = HelioInner {
                renderer: r,
                device: device_arc.clone(),
                queue: queue_arc.clone(),
                interaction: SceneInteraction::default(),
                planet_terrain: None,
                planet_graph_rebuilt: false,
                last_scene_revision: 0,
                has_rendered_frame: false,
            };
            self.inner = Some(inner);
            self.viewport_size = (width, height);

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
        #[cfg(feature = "editor-ui")]
        let _pointer_events_profile = (!pending_pointer_events.is_empty()).then(|| {
            gpui::enter_span(
                gpui::SpanName::Static("pulsar: HelioRenderer::pointer_events"),
                gpui::SpanCategory::UserDefined,
                None,
            )
        });
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

        #[cfg(feature = "editor-ui")]
        let _engine_frame_diagnostic = (self.frame_count, width, height);

        {
            profiling::profile_scope!("helio_camera_input");
            self.apply_camera_input(dt);
        }

        // Reconcile the terrain runtime with the scene's planets before the
        // idle check below reads `planet_terrain` -- creating or retiring a
        // planet must itself be able to wake the frame up.
        self.sync_terrain_planets();

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
        let scene_revision = self.scene_store.read().world.revision();
        // A newly-created/loaded SceneDB can have revision 0. The first
        // renderer frame still steps the database so its GPU mirror is current
        // before Helio reads it.

        let needs_initial_scene_sync = !inner.has_rendered_frame;
        let force_scene_sync = self.pending_force_full_resync.swap(false, Ordering::AcqRel);
        let has_pending_scene =
            needs_initial_scene_sync || force_scene_sync || scene_revision != inner.last_scene_revision;
        let has_pending_editor = self.pending_deselect.load(Ordering::Acquire)
            || self.pending_gizmo_mode.lock().is_ok_and(|g| g.is_some())
            || self.pending_force_full_resync.load(Ordering::Acquire);
        let camera_stopped = self.cam_local_velocity.length_squared() <= CAMERA_IDLE_EPSILON
            && !self.had_camera_input;
        // A terrain edit (or a brush-ring move) changes what is on screen
        // without touching the camera or the scene database, so it has to
        // defeat the idle early-out itself -- otherwise a sculpt stroke made
        // while the camera is parked would not appear until the user moved.
        // Also true while a body is still streaming in: that work only progresses
        // on rendered frames, so a parked camera must not idle it to a halt.
        let terrain_streaming = inner.planet_terrain.as_ref().is_some_and(|runtime| {
            runtime.renderer_ready(&inner.renderer) && runtime.has_pending_work(&inner.renderer)
        });
        let has_pending_terrain = self.terrain.wants_advance() || terrain_streaming;
        let is_idle = camera_stopped
            && !has_pending_scene
            && !has_pending_editor
            && !has_pending_terrain
            && !self.gizmo_dirty
            && !viewport_resized
            && !self.reset_taa_next_frame;

        // Clear the sticky input flag when camera actually stopped.
        if camera_stopped {
            self.had_camera_input = false;
        }

        // ── Resize ──────────────────────────────────────────────────────────────
        if viewport_resized {
            #[cfg(feature = "editor-ui")]
            gpui::flamegraph_span!("pulsar: HelioRenderer::resize");
            #[cfg(feature = "editor-ui")]
            let _engine_resize_diagnostic = (width, height, scene_revision, self.frame_count);
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
            self.scene_store.write().world.select(None);
            inner.interaction.cancel_drag();
            self.gizmo_dirty = true;
        }
        if let Ok(mut pending) = self.pending_gizmo_mode.lock() {
            if let Some(mode) = pending.take() {
                inner.interaction.set_mode(mode);
                self.gizmo_dirty = true;
            }
        }
        // Inlined rather than calling `self.force_full_resync()` -- `inner`
        // above is already a live `&mut` borrow of `self.inner` at this
        // point, and `force_full_resync` needs the same borrow itself.
        if force_scene_sync {
            inner.last_scene_revision = 0;
            inner.has_rendered_frame = false;
        }

        // ── Early out when idle ─────────────────────────────────────────────────
        // No GPU work, no gizmo rebuild, no planet terrain tick, no profiler reads.
        if is_idle {
            // Idle frames must still serve inspector requests.
            self.scene_store.read().world.publish_inspector_snapshot();
            if let Ok(mut m) = self.metrics.lock() {
                m.fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                m.frame_time_ms = dt * 1000.0;
                m.frames_rendered = self.frame_count;
            }
            return None;
        }

        // SceneDB owns all world state and performs its own GPU-mirror flush.
        // The renderer only advances that authoritative database at the frame
        // boundary; it never builds a CPU projection or submits per-object data.
        let mut sync_ms = 0.0;
        if has_pending_scene {
            #[cfg(feature = "editor-ui")]
            gpui::flamegraph_span!("pulsar: HelioRenderer::scene_db_step");
            profiling::profile_scope!("helio_scene_db_step");
            let t_sync = Instant::now();
            let mut scene_store = self.scene_store.write();
            crate::scene::editor_rows::sync_editor_light_rows(&mut scene_store.world, true);
            crate::scene::sync_static_mesh_rows(&mut scene_store);
            scene_store.step();
            sync_ms = t_sync.elapsed().as_secs_f64() * 1000.0;
            inner.last_scene_revision = scene_revision;
        }

        // SceneDB Inspector bridge: throttled inside SceneDB, and a no-op unless
        // an inspector launched this process. After the GPU flush above.
        self.scene_store.read().world.publish_inspector_snapshot();

        // ── Camera / planet / gizmo / render ────────────────────────────────────
        let t_prepare = Instant::now();
        let camera = {
            #[cfg(feature = "editor-ui")]
            gpui::flamegraph_span!("pulsar: HelioRenderer::frame_prepare");
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

            // Planet terrain advance: whenever the camera is moving, or when a
            // terrain edit is waiting to be streamed back in.
            let terrain_dirty = self.terrain.take_pending_advance();
            let should_advance_planet = !viewport_resized
                && inner.planet_terrain.as_ref().is_some_and(|runtime| {
                    runtime.has_active_components() && runtime.renderer_ready(&inner.renderer)
                })
                && (!camera_stopped || viewport_resized || terrain_dirty || terrain_streaming);
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

            // Debug geometry is transient GPU execution state. World content is
            // read by Helio passes directly from the SceneDB GPU mirror.
            inner.renderer.debug_clear();
            let store = self.scene_store.read();
            inner
                .interaction
                .draw_gizmo(&mut inner.renderer, &store.world, self.cam_pos);
            // Terrain brush ring, from the tool-mode mailbox. Same transient
            // debug-geometry sink the gizmo uses, so it is rebuilt per frame
            // and needs no lifetime management of its own.
            if let Some(brush) = self.terrain.brush_cursor() {
                inner.renderer.debug_torus(
                    brush.center_m,
                    brush.normal,
                    brush.radius_m,
                    (brush.radius_m * 0.02).max(0.02),
                    brush.color,
                    48,
                    6,
                );
            }
            camera
        };

        if self.reset_taa_next_frame {
            self.reset_taa_next_frame = false;
        }

        let prepare_ms = t_prepare.elapsed().as_secs_f64() * 1000.0;
        let t_render = Instant::now();
        let submission_index = {
            #[cfg(feature = "editor-ui")]
            gpui::flamegraph_span!("pulsar: HelioRenderer::render_submit");
            profiling::profile_scope!("helio_render_submit");
            // `SceneDb::step()` above (behind `has_pending_scene`) already
            // flushes the World's GPU mirror when it runs, but that gate
            // exists to skip the *simulation* step on an idle frame, not to
            // gate GPU visibility of writes queued elsewhere this frame
            // (gizmo drag, script/tool mutation, a `World::insert` from
            // outside this renderer's own sync point). An explicit,
            // unconditional flush right before every render call is a cheap
            // no-op `RwLock` read plus a `queue.write_buffer` per dirty row
            // when there IS nothing new -- and it's the one thing proven,
            // empirically (Helio's own examples showed a fully black/empty
            // render with valid SceneDB rows and zero GPU-side errors until
            // this call was added), to be required for CPU-side SceneDB
            // writes to ever become visible on the GPU at all. TODO(review):
            // this and the `material_textures`/`template_registry` fallback
            // buffers in `helio::Renderer::setup` are centrally-owned state
            // that the zero-central-knowledge architecture mandate says
            // shouldn't exist here -- flagged for a follow-up pass, not
            // fixed now.
            self.scene_store.read().world.flush_gpu_mirror(&inner.queue);
            if let Err(e) = inner.renderer.render(&camera, &view) {
                tracing::error!("Helio render error: {:?}", e);
            }
            Some(
                inner
                    .queue
                    .submit(std::iter::empty::<wgpu::CommandBuffer>()),
            )
        };
        self.gizmo_dirty = false;
        inner.has_rendered_frame = true;
        let render_ms = t_render.elapsed().as_secs_f64() * 1000.0;
        let frame_ms = frame_start.elapsed().as_secs_f32() * 1_000.0;
        if frame_ms >= 50.0 {
            tracing::warn!(
                target: "flamegraph.workload",
                frame_ms,
                render_ms,
                frame_index = self.profiler_frame_counter,
                profiling_enabled = profiling::is_profiling_enabled(),
                producer_queue = profiling::init_profiler().pending_event_count(),
                retained_events = profiling::init_profiler().retained_event_count(),
                dropped_events = profiling::init_profiler().dropped_event_count(),
                "slow Helio frame"
            );
        }
        // Emit the boundary from the render thread. The profiler collector
        // must not inspect the renderer registry or GPU mutex from a second
        // thread just to obtain this value.
        profiling::record_frame_time(frame_ms);

        // ── GPU profiler ───────────────────────────────────────────────────────
        // Continuous flamegraph capture needs every completed asynchronous
        // readback. Keep the old 30-frame cadence only for the cheap
        // always-on diagnostic cache when no instrumentation capture owns the
        // profiler.
        if profiling::is_profiling_enabled() || self.profiler_frame_counter >= 30 {
            self.profiler_frame_counter = 0;
            self.gpu_profiler
                .update_from_snapshot(inner.renderer.timing_snapshot());
        }

        let gpu_frame = self.gpu_profiler.gpu_frame_count;
        let new_gpu_result = gpu_frame.is_some() && gpu_frame != self.last_reported_gpu_frame;
        if new_gpu_result {
            emit_helio_gpu_passes(&self.gpu_profiler);
        }
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

    // ── SceneDB-backed editor integration ───────────────────────────────────

    pub fn queue_gizmo_mode(&self, mode: GizmoMode) {
        if let Ok(mut guard) = self.pending_gizmo_mode.lock() {
            *guard = Some(mode);
        }
    }

    pub fn queue_deselect(&self) {
        self.pending_deselect.store(true, Ordering::Release);
    }

    pub fn queue_left_click(&self, norm_x: f32, norm_y: f32) {
        if let Ok(mut events) = self.pending_pointer_events.lock() {
            events.push(PendingPointerEvent::LeftClick { norm_x, norm_y });
        }
    }

    pub fn queue_left_release(&self) {
        if let Ok(mut events) = self.pending_pointer_events.lock() {
            events.push(PendingPointerEvent::LeftRelease);
        }
    }

    pub fn queue_force_full_resync(&self) {
        self.pending_force_full_resync
            .store(true, Ordering::Release);
    }

    pub fn get_scene_db_selected_id(&self) -> Option<String> {
        self.scene_store.read().world.selected_id()
    }

    pub fn force_full_resync(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.last_scene_revision = 0;
            inner.has_rendered_frame = false;
            inner.interaction.cancel_drag();
        }
    }

    pub fn editor_mailbox(&self) -> HelioEditorMailbox {
        HelioEditorMailbox {
            pending_gizmo_mode: self.pending_gizmo_mode.clone(),
            pending_deselect: self.pending_deselect.clone(),
            pending_force_full_resync: self.pending_force_full_resync.clone(),
        }
    }

    /// Cheap handle bundle for the terrain tool mode. Like
    /// [`Self::editor_mailbox`] this is fetched once and never takes the
    /// renderer's per-frame lock afterwards.
    pub fn terrain_mailbox(&self) -> TerrainEditMailbox {
        self.terrain.clone()
    }

    /// Reconcile the terrain runtime with the terrain bodies the editor
    /// posted, creating the runtime on first use and retiring it when the last
    /// body disappears.
    ///
    /// A body is a planet or a flat voxel volume; this path does not care
    /// which, because `upsert_body_component` registers either one the same
    /// way and the component cache keys off the shared body identity.
    ///
    /// This is deliberately driven by an explicit mailbox rather than by the
    /// generic world-component dispatch: that dispatch was removed with the
    /// SceneDB nativization work, and the terrain seam must not depend on when
    /// it comes back.
    fn sync_terrain_planets(&mut self) {
        let Some(definitions) = self.terrain.take_pending_bodies() else {
            return;
        };
        let Some(inner) = self.inner.as_mut() else {
            return;
        };

        if definitions.is_empty() {
            if inner.planet_terrain.take().is_some() {
                self.terrain.publish_runtime(None);
                Self::sync_planet_graph(inner, &self.pending_errors);
            }
            return;
        }

        if inner.planet_terrain.is_none() {
            match PlanetTerrainRuntime::new() {
                Ok(runtime) => inner.planet_terrain = Some(runtime),
                Err(error) => {
                    let message = format!("Planet terrain runtime initialization failed: {error}");
                    tracing::error!("{message}");
                    if let Ok(mut errors) = self.pending_errors.lock() {
                        errors.push(message);
                    }
                    return;
                }
            }
        }

        let Some(planet_terrain) = inner.planet_terrain.as_mut() else {
            return;
        };
        let (runtime, cache) = planet_terrain.component_context_mut();
        let runtime = runtime.clone();
        let mut live_keys = pulsar_reflection::LiveKeySet::new();
        for (source_key, definition) in &definitions {
            live_keys.insert(source_key.clone());
            match runtime.upsert_body_component(source_key.clone(), *definition) {
                Ok(_) => cache.record(source_key.clone(), definition.body_id()),
                Err(error) => {
                    let message = format!("Planet terrain component sync failed: {error}");
                    tracing::error!("{message}");
                    if let Ok(mut errors) = self.pending_errors.lock() {
                        errors.push(message);
                    }
                }
            }
        }
        if let Err(error) = planet_terrain.remove_stale_components(&live_keys) {
            tracing::error!("Planet terrain stale-component cleanup failed: {error}");
        }

        self.terrain.publish_runtime(Some(runtime));
        Self::sync_planet_graph(inner, &self.pending_errors);
    }

    pub fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        self.gizmo_dirty = true;
        if let Some(inner) = &mut self.inner {
            inner.interaction.set_mode(mode);
        }
    }

    pub fn get_selected_object(&self) -> Option<pulsar_scenedb::Entity> {
        self.scene_store.read().world.selected_entity()
    }

    pub fn get_selected_scene_db_id(&self) -> Option<String> {
        self.get_scene_db_selected_id()
    }

    pub fn select_by_scene_db_id(&mut self, scene_db_id: &str) -> bool {
        let mut scene = self.scene_store.write();
        let entity = scene.world.entity_for(scene_db_id);
        if entity.is_some() {
            scene.world.select(entity);
            drop(scene);
            self.gizmo_dirty = true;
        }
        entity.is_some()
    }

    pub fn deselect(&mut self) {
        self.scene_store.write().world.select(None);
        if let Some(inner) = &mut self.inner {
            inner.interaction.cancel_drag();
        }
        self.gizmo_dirty = true;
    }

    pub fn reset_taa(&mut self) {
        self.reset_taa_next_frame = true;
    }

    pub fn select_object_atomic(&mut self, scene_db_id: Option<String>) -> bool {
        let exists = {
            let mut scene = self.scene_store.write();
            let entity = scene_db_id.as_deref().and_then(|id| scene.world.entity_for(id));
            scene.world.select(entity);
            scene_db_id.is_none() || entity.is_some()
        };
        if let Some(inner) = &mut self.inner {
            inner.interaction.cancel_drag();
        }
        self.gizmo_dirty = true;
        exists
    }

    /// Build a world-space ray from normalized viewport coordinates using only
    /// the renderer camera pose and local projection math.
    fn build_pick_ray(&self, norm_x: f32, norm_y: f32) -> (Vec3, Vec3) {
        let (width, height) = self.viewport_size;
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        let x = norm_x.clamp(0.0, 1.0) * 2.0 - 1.0;
        let y = 1.0 - norm_y.clamp(0.0, 1.0) * 2.0;
        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = Vec3::new(sy * cp, sp, -cy * cp);
        let projection =
            Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, width / height, 0.1, 10_000.0);
        let view = Mat4::look_at_rh(self.cam_pos, self.cam_pos + forward, Vec3::Y);
        let inverse = (projection * view).inverse();
        let near = inverse.project_point3(Vec3::new(x, y, 0.0));
        let far = inverse.project_point3(Vec3::new(x, y, 1.0));
        (near, (far - near).normalize_or_zero())
    }

    pub fn handle_left_click(&mut self, norm_x: f32, norm_y: f32) {
        self.gizmo_dirty = true;
        let (ray_origin, ray_direction) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };
        let store = self.scene_store.read();
        if inner
            .interaction
            .try_start_drag(&store.world, ray_origin, ray_direction, self.cam_pos)
        {
            return;
        }
        let target = inner.interaction.pick(&store.world, ray_origin, ray_direction);
        drop(store);
        self.select_object_atomic(target);
    }

    pub fn handle_mouse_move(&mut self, norm_x: f32, norm_y: f32) {
        self.gizmo_dirty = true;
        let (ray_origin, ray_direction) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };
        let mut store = self.scene_store.write();
        inner
            .interaction
            .update_hover(&store.world, ray_origin, ray_direction, self.cam_pos);
        if inner.interaction.is_dragging() {
            inner
                .interaction
                .update_drag(&mut store.world, ray_origin, ray_direction, self.cam_pos);
        }
    }

    pub fn handle_left_release(&mut self) {
        self.gizmo_dirty = true;
        if let Some(inner) = &mut self.inner {
            inner.interaction.cancel_drag();
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

        let context = || helio::PassBuildContext {
            device: &inner.device,
            queue: &inner.queue,
            config: inner.renderer.renderer_config(),
            debug_state: inner.renderer.debug_state(),
            // The *scene* camera buffer. This used to pass `debug_camera_buf()`
            // (64-byte UNIFORM-only), so the first planet/volume created in
            // the editor rebuilt the graph with a buffer `ShadowMatrixPass`
            // binds as storage -> wgpu validation panic on the render thread.
            camera_buffer: inner.renderer.camera_buf(),
            cull_stats_buffer: inner.renderer.cull_stats_buf(),
            owns_device: false,
            scene_db: inner.renderer.scene_db(),
        };
        let graph = if wants_planet_graph {
            helio_default_graphs::build_default_graph_external_with_planetary_voxels_with_context(
                context(),
                PlanetTerrainRuntime::renderer_config(),
            )
            .map_err(|error| error.to_string())
        } else {
            Ok(helio_default_graphs::build_default_graph_external_with_context(context()))
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
}

/// Publish completed Helio timestamp-query results into the shared trace.
///
/// These are deliberately submitted as GPU-track events rather than pretending
/// that the asynchronous query result ran on the render thread. `parent_name`
/// gives the viewer a stable relationship to the Helio frame scope, while the
/// GPU thread identity keeps the samples on the dedicated GPU lane.
fn emit_helio_gpu_passes(data: &GpuProfilerData) {
    if !profiling::is_profiling_enabled() {
        return;
    }

    let Some(total_gpu_ms) = data.total_gpu_ms else {
        return;
    };
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let mut cursor_ns = now_ns.saturating_sub((total_gpu_ms * 1_000_000.0) as u64);
    let profiler = profiling::init_profiler();
    let total_duration_ns = (total_gpu_ms * 1_000_000.0) as u64;
    let gpu_frame_scope_id = profiling::allocate_scope_id();
    let logical_parent = profiling::current_scope_context().parent_scope_id;

    profiler.submit_event(profiling::ProfileEvent {
        scope_id: gpu_frame_scope_id,
        parent_scope_id: logical_parent,
        name: "helio_frame".to_string(),
        thread_id: 0,
        thread_name: Some("GPU".to_string()),
        process_id: profiler.get_process_id(),
        parent_name: None,
        start_ns: cursor_ns,
        duration_ns: total_duration_ns,
        depth: 0,
        location: None,
        metadata: Some("domain=helio;track=gpu".to_string()),
        track_name: Some("GPU".to_string()),
    });

    for pass in &data.render_metrics {
        let Some(gpu_ms) = pass.gpu_ms else {
            continue;
        };
        let duration_ns = (gpu_ms * 1_000_000.0) as u64;
        profiler.submit_event(profiling::ProfileEvent {
            scope_id: profiling::allocate_scope_id(),
            parent_scope_id: Some(gpu_frame_scope_id),
            name: pass.name.to_string(),
            thread_id: 0,
            thread_name: Some("GPU".to_string()),
            process_id: profiler.get_process_id(),
            parent_name: Some("helio_frame".to_string()),
            start_ns: cursor_ns,
            duration_ns,
            depth: 1,
            location: None,
            metadata: Some("domain=helio;track=gpu".to_string()),
            track_name: Some("GPU".to_string()),
        });
        cursor_ns = cursor_ns.saturating_add(duration_ns);
    }
}
