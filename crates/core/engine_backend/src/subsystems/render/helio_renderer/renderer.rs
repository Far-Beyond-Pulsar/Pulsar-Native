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
use pulsar_rendering::{
    subsystems::{
        apply_portal_pair_action, remove_foliage_handles, FoliageCache, LightCache, MeshCache,
        PortalLinkCache, SceneObjectCache,
    },
    PlanetTerrainFrameInput, PlanetTerrainRuntime, PLANET_TERRAIN_CLASS_NAME,
};
use pulsar_scene::{build_transform_parts, component_instances_from_props};

use crate::scene::{ObjectUpdate, SceneDbDelta, WorldSceneStore};
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
    /// Tracks scene object instances keyed by tag for incremental
    /// update (avoid cascade-free on clear-all-insert-each-frame).
    object_cache: SceneObjectCache,
    /// Persists foliage component handles (types/layers/interactors/materials)
    /// so the editor's per-sync component pass updates them in place instead of
    /// re-registering (which re-rolls GPU placement) every scene change.
    foliage_cache: FoliageCache,
    /// Tracks light actors keyed by scene-object ID so LightComponent can
    /// update them in place instead of the scene wholesale-clearing and
    /// re-inserting every light on every sync pass.
    light_cache: LightCache,
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
                device: device_arc,
                queue: queue_arc,
                editor_state: EditorState::new(),
                scene_picker: ScenePicker::new(),
                mesh_cache: MeshCache::new(),
                object_cache: SceneObjectCache::new(),
                foliage_cache: FoliageCache::new(),
                light_cache: LightCache::new(),
                portal_link_cache: PortalLinkCache::new(),
                planet_terrain: None,
                planet_graph_rebuilt: false,
                last_scene_revision: 0,
                known_ids: HashSet::new(),
            };
            self.populate_initial_scene(&mut inner);
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
                Self::sync_scene_delta(&self.scene_store, inner);
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
        // component_instances_from_snap now delegates to pulsar_scene's shared impl.
        fn component_instances_from_snap(
            snap: &crate::scene::ObjectSnapshot,
        ) -> Vec<(usize, String, serde_json::Value)> {
            component_instances_from_props(
                &snap.render_props.props,
                snap.render_props.component_instances.as_ref(),
            )
        }

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

        // Skip sync while the gizmo is actively dragging.
        if inner.editor_state.is_dragging() {
            return;
        }

        // Lights are managed incrementally through LightCache (like objects
        // via SceneObjectCache below): LightComponent looks up its cached
        // LightId and calls Scene::update_light in place instead of the
        // scene being wholesale-cleared and every light re-inserted fresh
        // every sync pass.
        // Objects are managed incrementally through SceneObjectCache:
        // components call get_subsystem!(context, SceneObjectCache) to look up
        // existing objects by tag, then either update transforms in-place or
        // insert new ones.  After the sync pass we remove stale entries (those
        // the component system didn't touch this frame).

        // ── Component sync pass ───────────────────────────────────────────────
        // Single write-lock held for the whole pass -- see this module's top
        // doc / SceneDatabase's B1 migration note for why: WorldSceneStore
        // has no lock-free per-entry design, so a snapshot pull followed by a
        // dirty-flag drain needs one held guard, not two racing acquisitions.
        let mut store = scene_store.write();
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
            let owner = RuntimeComponentOwner {
                scene_object_id: snap.stable_id.as_str(),
                position: snap.transform.position,
                rotation: snap.transform.rotation,
                scale: snap.transform.scale,
                props: &snap.render_props.props,
            };

            let component_instances = component_instances_from_snap(snap);
            let needs_planet_runtime = component_instances.iter().any(|(_, class_name, data)| {
                class_name == PLANET_TERRAIN_CLASS_NAME
                    && data
                        .get("enabled")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true)
            });
            if needs_planet_runtime
                && inner.planet_terrain.is_none()
                && !planet_runtime_init_attempted
            {
                planet_runtime_init_attempted = true;
                match PlanetTerrainRuntime::new() {
                    Ok(runtime) => inner.planet_terrain = Some(runtime),
                    Err(error) => {
                        let message =
                            format!("Planet terrain runtime initialization failed: {error}");
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
            subsystems.register_ref::<SceneObjectCache>(&mut inner.object_cache);
            subsystems.register_ref::<FoliageCache>(&mut inner.foliage_cache);
            subsystems.register_ref::<LightCache>(&mut inner.light_cache);
            subsystems.register_ref::<PortalLinkCache>(&mut inner.portal_link_cache);
            if let Some(planet_terrain) = inner.planet_terrain.as_mut() {
                let (runtime, cache) = planet_terrain.component_context_mut();
                subsystems.register_ref(runtime);
                subsystems.register_ref(cache);
            }
            subsystems.register_ref::<LiveKeySet>(&mut live_keys);
            let mut ctx = HelioRuntimeContext {
                renderer: &mut inner.renderer,
                subsystems,
                error_queue,
                project_root: &project_root,
            };

            for (component_index, class_name, data) in component_instances {
                let _ = apply_runtime_behavior_for_class(
                    class_name.as_str(),
                    &owner,
                    component_index,
                    &data,
                    &mut ctx,
                );
            }
        }

        // Remove stale scene objects and cache entries (components didn't touch them).
        let stale_ids: Vec<String> = inner
            .object_cache
            .map
            .keys()
            .filter(|id| !live_keys.inner().contains(*id))
            .cloned()
            .collect();
        for scene_id in stale_ids {
            if let Some((obj_id, _)) = inner.object_cache.remove(&scene_id) {
                let _ = inner.renderer.scene_mut().remove_object(obj_id);
            }
        }

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

        // Remove stale lights (object deleted, or its LightComponent removed
        // while the object stayed — LightComponent itself handles the
        // disabled-but-still-attached case).
        let stale_lights: Vec<String> = inner
            .light_cache
            .map
            .keys()
            .filter(|key| !live_keys.contains(*key))
            .cloned()
            .collect();
        for key in stale_lights {
            if let Some(light_id) = inner.light_cache.remove(&key) {
                let _ = inner.renderer.scene_mut().remove_light(light_id);
            }
        }

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
        for snap in &snapshots {
            if let Some((obj_id, _)) = inner.object_cache.get(&snap.stable_id) {
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

        // Full sync just brought every object in `live_keys` fully up to
        // date from its snapshot, so clear their dirty flags here — full
        // sync never marked them dirty in the first place (that only
        // happens via WorldSceneStore::mark_dirty / a fresh spawn), but it
        // must still consume any that accumulated, or the delta-sync path
        // would see them as still needing work it just did and redo it
        // every frame until something happened to touch drain_dirty().
        //
        // Scoped to `live_keys` (this pass's snapshot) rather than a global
        // `store.drain_dirty()`, matching the pre-B1 behavior: this whole
        // function now holds `store`'s write lock for its entire duration
        // (see the guard taken above), so the original race this comment
        // used to describe -- another thread inserting into a
        // concurrently-mutated `DashMap` between the snapshot pull and this
        // loop -- can no longer happen; the lock rules it out. Kept scoped
        // to `live_keys` anyway rather than switching to a global drain,
        // since that's a distinct, unrelated behavior change (it would also
        // clear dirty flags for objects this pass never actually synced to
        // Helio, which isn't what "full sync completed" should mean) and
        // not something this migration set out to change.
        for id in live_keys.inner() {
            let _ = store.take_dirty_flags(id);
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

    fn sync_scene_delta(
        scene_store: &Arc<RwLock<WorldSceneStore>>,
        inner: &mut HelioInner,
    ) -> SceneDbDelta {
        let mut store = scene_store.write();
        let revision = store.dirty_gen();
        let dirty = store.drain_dirty();
        let removed = store.take_removed_ids();

        let mut added = Vec::new();
        let mut updated = Vec::new();

        for (id, flags) in dirty {
            if inner.known_ids.contains(&id) {
                if let Some(snap) = store.get_object(&id) {
                    updated.push(ObjectUpdate {
                        id,
                        transform: Some(build_transform_parts(
                            snap.transform.position,
                            snap.transform.rotation,
                            snap.transform.scale,
                        )),
                        visible: Some(snap.visibility.visible),
                        name: None,
                    });
                }
            } else {
                added.push(id);
            }
        }

        for id in &removed {
            inner.known_ids.remove(id);
        }
        for id in &added {
            inner.known_ids.insert(id.clone());
        }

        SceneDbDelta {
            added,
            removed,
            updated,
            revision,
        }
    }
}
