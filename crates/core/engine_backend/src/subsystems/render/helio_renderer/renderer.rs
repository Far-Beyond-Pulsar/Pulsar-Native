//! Main HelioRenderer — wgpu + Helio scene renderer backed by SceneDB.

use crate::services::render_runtimes::FrameKind;
use glam::{DVec3, Vec2, Vec3};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use super::core::{CameraInput, GpuProfilerData, RenderMetrics, RenderSpikeLogConfig};
use crate::scene::{GizmoType, SceneWorldExt};

use super::camera_ground::{altitude_speed_scale, CameraGround};
use super::fly_camera::{direction, yaw_pitch, FlyCamera};
use super::gpu_trace::emit_helio_gpu_passes;
use super::interaction::SceneInteraction;
#[cfg(test)]
use super::scene_renderer::ortho_camera;
use super::scene_renderer::{
    ortho_depth_range, DeviceOwner, FrameCrop, FrameProjection, FrameView, PreparedFrame,
    SceneRenderer, SceneRendererOptions,
};
type GizmoMode = GizmoType;

/// Camera velocity squared below this threshold is considered stopped.
const CAMERA_IDLE_EPSILON: f32 = 0.001;

/// Finish temporal reconstruction after activity stops, then return to idle.
/// TSR accumulates history at ~4 % a frame: three time constants (~75
/// frames) bring a moving view's softer history to its still sharpness.
/// After 32 frames the editor held a half-converged, blurred frame.
const TEMPORAL_SETTLING_FRAMES: u8 = 90;

#[derive(Default)]
struct TemporalSettling {
    remaining: u8,
}

impl TemporalSettling {
    fn observe_activity(&mut self, temporal_enabled: bool, active: bool) {
        if !temporal_enabled {
            self.remaining = 0;
        } else if active {
            self.remaining = TEMPORAL_SETTLING_FRAMES;
        }
    }

    fn needs_frame(&self) -> bool {
        self.remaining > 0
    }

    fn complete_frame(&mut self, temporal_enabled: bool, active: bool, rendered: bool) {
        if !rendered {
            return;
        }
        // Graph installation can enable TSR after the idle check this frame.
        self.observe_activity(temporal_enabled, active);
        // Activity frames rearm the budget; failures and idle calls consume none.
        if !active {
            self.remaining = self.remaining.saturating_sub(1);
        }
    }
}

/// Where a frame's time went: milliseconds between successive marks, logged
/// for slow frames (`VOXEL_FRAME_PHASES`).
struct FramePhases {
    last: Instant,
    list: Vec<(&'static str, f32)>,
}

impl FramePhases {
    fn new(start: Instant) -> Self {
        Self {
            last: start,
            list: Vec::with_capacity(16),
        }
    }

    fn mark(&mut self, name: &'static str) {
        let now = Instant::now();
        self.list
            .push((name, now.duration_since(self.last).as_secs_f32() * 1000.0));
        self.last = now;
    }

    fn describe(&self) -> String {
        self.list
            .iter()
            .filter(|(_, ms)| *ms >= 0.5)
            .map(|(name, ms)| format!("{name}={ms:.1}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Opens a WGPUI flamegraph span for the rest of the enclosing block when the
/// editor UI is built in.
macro_rules! frame_span {
    ($name:literal) => {
        #[cfg(feature = "editor-ui")]
        gpui::flamegraph_span!($name);
    };
}

/// Marks the render frame in the WGPUI timeline when the editor UI is built in.
macro_rules! frame_profile_bridge {
    () => {
        #[cfg(feature = "editor-ui")]
        let _wgpui_profile_bridge = WgpuiProfileBridge::begin();
    };
}

/// Opens the pointer-event span while `$active` is true, when the editor UI
/// is built in.
macro_rules! pointer_events_span {
    ($active:expr) => {
        #[cfg(feature = "editor-ui")]
        let _pointer_events_profile = ($active).then(|| {
            gpui::enter_span(
                gpui::SpanName::Static("pulsar: HelioRenderer::pointer_events"),
                gpui::SpanCategory::UserDefined,
                None,
            )
        });
    };
}

/// Keeps a frame's identifying values in a local for crash dumps and debuggers.
macro_rules! frame_diagnostic {
    ($value:expr) => {
        let _frame_diagnostic = $value;
    };
}

/// The size and format of the surface a frame is drawn to.
#[derive(Clone, Copy)]
struct SurfaceSpec {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}

/// What one editor frame draws into.
struct FrameRequest<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    view: &'a wgpu::TextureView,
    surface: SurfaceSpec,
    /// The tile of a larger output being drawn, if any.
    crop: Option<FrameCrop>,
}

/// When a frame started and how long since the previous one.
struct FrameClock {
    start: Instant,
    now: Instant,
    /// Seconds since the previous frame; zero for a capture tile.
    dt: f32,
}

/// One frame's camera move: where it started and ended, the per-axis steps
/// and the pan.
struct FlyMove {
    before: DVec3,
    end: DVec3,
    steps: [DVec3; 3],
    pan: DVec3,
}

/// What camera updating changed this frame.
struct CameraUpdate {
    view_kind_changed: bool,
    camera_synced: bool,
}

/// Editor state sampled before idle detection.
struct EditorFrameState {
    postprocess_changed: bool,
    debug_view_changed: bool,
    ortho_half_height: f32,
    view_payloads: Vec<(&'static str, Option<ViewPayload>)>,
}

/// Which inputs changed since the last frame; any of them wakes an idle viewport.
#[derive(Clone, Copy)]
struct FrameChanges {
    needs_resize: bool,
    view_kind: bool,
    camera_synced: bool,
    postprocess: bool,
    debug_view: bool,
    view_payloads: bool,
    measured_deferred_light: bool,
}

/// How long after something drove a frame the temporal resolve is still
/// converging on it: those frames are the user's (full pace), not ambient.
const DRIVEN_CONVERGENCE: std::time::Duration = std::time::Duration::from_millis(400);

/// The outcome of idle detection for one frame.
struct FrameActivity {
    viewport_resized: bool,
    realtime: bool,
    scene_revision: u64,
    has_pending_scene: bool,
    temporal_activity: bool,
    is_idle: bool,
    camera_stopped: bool,
}

/// Timings of one drawn frame, in milliseconds.
#[derive(Clone, Copy)]
struct FrameTimes {
    frame_ms: f32,
    sync_ms: f64,
    prepare_ms: f64,
    render_ms: f64,
}

mod camera_move;
mod frame;
mod frame_report;
mod frame_steps;
mod prepare;

// ── Compatibility types retained for existing UI wiring ───────────────────────

#[derive(Debug, Clone)]
pub enum RendererCommand {
    ToggleFeature(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EditorCameraState {
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl EditorCameraState {
    /// The point `distance` ahead of the camera, which the other viewports of a
    /// split view centre on.
    pub fn focus_point(&self, distance: f64) -> [f64; 3] {
        let ahead = direction(self.yaw, self.pitch).as_dvec3() * distance;
        (DVec3::from_array(self.position) + ahead).to_array()
    }
}

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

/// How a viewport looks at the scene: the free perspective camera, or an
/// orthographic one. The axis views are fixed in orientation; the editor
/// pans and zooms them instead of turning them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewKind {
    #[default]
    Perspective,
    /// Orthographic projection from the free camera's orientation.
    Orthographic,
    /// Looking straight down (-Y), +X to the right.
    Top,
    /// Looking along -Z, +X to the right.
    Front,
    /// Looking along -X, -Z to the right.
    Side,
}

impl ViewKind {
    fn to_u8(self) -> u8 {
        match self {
            Self::Perspective => 0,
            Self::Orthographic => 1,
            Self::Top => 2,
            Self::Front => 3,
            Self::Side => 4,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Orthographic,
            2 => Self::Top,
            3 => Self::Front,
            4 => Self::Side,
            _ => Self::Perspective,
        }
    }

    pub fn is_orthographic(self) -> bool {
        self != Self::Perspective
    }

    /// Whether the orientation is fixed by the view (and the mouse pans
    /// instead of looking).
    pub fn is_axis_aligned(self) -> bool {
        matches!(self, Self::Top | Self::Front | Self::Side)
    }

    /// World-space forward, right and up of an axis view.
    fn axis_basis(self) -> Option<(Vec3, Vec3, Vec3)> {
        match self {
            Self::Top => Some((Vec3::NEG_Y, Vec3::X, Vec3::NEG_Z)),
            Self::Front => Some((Vec3::NEG_Z, Vec3::X, Vec3::Y)),
            Self::Side => Some((Vec3::NEG_X, Vec3::NEG_Z, Vec3::Y)),
            Self::Perspective | Self::Orthographic => None,
        }
    }
}

/// How far in front of the perspective camera the point lies that the other
/// viewports of a split view centre on, and the default length of their
/// "boom arms". An orthographic view at boom length `L` shows what a 45 degree
/// perspective camera `L` from the focus would.
pub const CAMERA_PIVOT_DISTANCE: f64 = 20.0;
const ORTHO_MIN_BOOM: f32 = 0.1;
const ORTHO_MAX_BOOM: f32 = 250_000.0;
/// Two positions closer than this are the same place to the camera sync.
const SYNC_EPSILON: f64 = 1.0e-6;

/// A camera position shared by the viewports of a split view. Each renderer
/// that has joined publishes where its camera went when the user moved it and
/// follows what the others publish, unless it is unlocked.
pub struct CameraSync {
    shared: parking_lot::Mutex<SharedCameraPosition>,
    /// How far ahead of the perspective camera the shared point lies, and the
    /// starting boom length of the orthographic views.
    pivot_distance: f64,
}

impl Default for CameraSync {
    fn default() -> Self {
        Self {
            shared: Default::default(),
            pivot_distance: CAMERA_PIVOT_DISTANCE,
        }
    }
}

impl CameraSync {
    /// A group centred on `focus` from the start. `pivot_distance` is how far
    /// ahead of the perspective camera that point is, and so how far out the
    /// orthographic views start: pass the scale the scene is viewed at (the
    /// distance to what is being looked at).
    pub fn at(focus: [f64; 3], pivot_distance: f64) -> Self {
        Self {
            shared: parking_lot::Mutex::new(SharedCameraPosition {
                generation: 1,
                position: Some(focus),
            }),
            pivot_distance: pivot_distance
                .clamp(f64::from(ORTHO_MIN_BOOM), f64::from(ORTHO_MAX_BOOM)),
        }
    }
}

#[derive(Default)]
struct SharedCameraPosition {
    /// Bumped at every publish.
    generation: u64,
    /// `None` until something moved.
    position: Option<[f64; 3]>,
}

/// What one renderer remembers of its place in a [`CameraSync`].
struct CameraSyncMember {
    group: Arc<CameraSync>,
    /// The generation this renderer last published or adopted. `u64::MAX`
    /// forces the next adoption (after being unlocked).
    seen_generation: u64,
    /// Where this renderer last published or adopted; a camera anywhere else
    /// was moved by the user.
    synced_position: DVec3,
    was_locked: bool,
}

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
#[derive(Debug, Clone)]
pub enum PendingPointerEvent {
    /// Latest-wins pointer position.  Unlike clicks, intermediate hover
    /// positions have no semantic value and must not build up a queue during
    /// a high-Hz mouse drag.
    MouseMove {
        norm_x: f32,
        norm_y: f32,
    },
    LeftClick {
        norm_x: f32,
        norm_y: f32,
    },
    /// A tool acting along the ray under the pointer (the sculpt tool's
    /// strokes): the render thread casts the ray with the frame's camera and
    /// calls the tool with the scene. `start` begins a stroke (pointer
    /// down); the drag samples after it are latest-wins, since a stroke
    /// fills the gaps between its samples.
    ToolRay {
        norm_x: f32,
        norm_y: f32,
        start: bool,
        tool: ToolRay,
    },
    LeftRelease,
    /// A box dragged over the viewport with Shift held, then released:
    /// select the objects inside it (normalized viewport corners, either
    /// first). A box too small to be a drag adds or removes the object under
    /// the pointer instead.
    BoxSelect {
        from: [f32; 2],
        to: [f32; 2],
    },
}

impl PendingPointerEvent {
    /// Hover positions and stroke drag samples: only the latest matters.
    fn latest_wins(&self) -> bool {
        matches!(
            self,
            Self::MouseMove { .. } | Self::ToolRay { start: false, .. }
        )
    }

    /// Queue `event`. A latest-wins event replaces the queued one of its
    /// kind when only latest-wins events follow it, so a high-Hz drag that
    /// interleaves hover moves and brush samples queues one of each per
    /// frame, never a backlog; order-sensitive events stay in order.
    pub fn queue(events: &mut Vec<Self>, event: Self) {
        if event.latest_wins() {
            for queued in events.iter_mut().rev() {
                if !queued.latest_wins() {
                    break;
                }
                if std::mem::discriminant(queued) == std::mem::discriminant(&event) {
                    *queued = event;
                    return;
                }
            }
        }
        events.push(event);
    }
}

/// What a tool does along a pointer ray: called on the render thread with
/// the scene, the ray's origin (the camera's eye, world frame) and
/// direction, and whether it starts a stroke.
#[derive(Clone)]
pub struct ToolRay(pub Arc<dyn Fn(&mut pulsar_scenedb::World, DVec3, DVec3, bool) + Send + Sync>);

impl std::fmt::Debug for ToolRay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ToolRay")
    }
}

/// Cheap, `Clone`-able handle bundle for issuing editor commands
/// (gizmo-mode change, deselect) without ever taking
/// `gpu_engine`'s blocking `std::sync::Mutex`.
///
/// `panel.rs` previously did `self.gpu_engine.lock()` for several one-shot
/// UI actions (tool switch, escape-to-deselect) -- a blocking
/// call that could stall the UI thread for as long as the render thread
/// holds `gpu_engine` (unconditionally, every frame, for the whole
/// `render_frame` call). Each of `queue_gizmo`/`queue_deselect` below only
/// ever touches its own small
/// `Arc<Mutex<...>>`/`Arc<AtomicBool>` mailbox (or `scene_store`'s already
/// cheap mailbox state) -- never `gpu_engine` -- so none of them can block on
/// the render thread's
/// per-frame lock hold at all.
#[derive(Clone)]
pub struct HelioEditorMailbox {
    pending_gizmo_mode: Arc<Mutex<Option<GizmoMode>>>,
    pending_camera_state: Arc<Mutex<Option<EditorCameraState>>>,
    pending_deselect: Arc<AtomicBool>,
    viewport_bloom: Arc<AtomicBool>,
    viewport_realtime: Arc<AtomicBool>,
    viewport_debug_mode: Arc<AtomicU32>,
    view_kind: Arc<AtomicU8>,
    camera_sync_locked: Arc<AtomicBool>,
    follow_space_transitions: Arc<AtomicBool>,
    camera_sync_join: Arc<Mutex<Option<Arc<CameraSync>>>>,
    viewport_debug_views: Arc<Mutex<Vec<ViewportDebugView>>>,
    pending_view_payloads: Arc<Mutex<Vec<(&'static str, Option<ViewPayload>)>>>,
    static_drag_warning: Arc<Mutex<Option<StaticDragWarning>>>,
    completed_drags: Arc<Mutex<Vec<CompletedDrag>>>,
    requested_pipeline: super::pipelines::RequestedPipeline,
}

/// A debug view advertised by a pass in the editor viewport's render graph.
pub type ViewportDebugLegend = helio::DebugViewLegend;

/// What an editor publishes for one viewport's passes, under a feature's
/// payload contract key (`helio_core::PayloadContract`).
pub type ViewPayload = Arc<dyn std::any::Any + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportDebugView {
    pub name: &'static str,
    pub debug_mode: u32,
    pub description: &'static str,
    /// Menu group ("Virtual Geometry"); empty for the default group.
    pub category: &'static str,
    pub legend: Option<ViewportDebugLegend>,
}

/// A gizmo drag that moved its objects, reported when the mouse is released
/// so the editor can record it as one undo step.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletedDrag {
    /// Each object the drag moved, with the (parent-relative) transform it
    /// had when the drag started.
    pub moved: Vec<(pulsar_scenedb::Entity, crate::scene::Transform)>,
}

/// A gizmo drag started on an object whose authored movability promises a
/// fixed transform. Moving it anyway leaves
/// cached data (the static shadow atlas) describing its old place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticDragWarning {
    pub object_id: String,
    pub object_name: String,
    pub movability: helio::Movability,
}

impl HelioEditorMailbox {
    pub fn set_follow_space_transitions(&self, follow: bool) {
        self.follow_space_transitions
            .store(follow, Ordering::Release);
    }

    /// Apply a loaded level camera at the next render-frame boundary.
    pub fn queue_camera(&self, camera: EditorCameraState) {
        if let Ok(mut pending) = self.pending_camera_state.lock() {
            *pending = Some(camera);
        }
    }

    /// The latest Static/Stationary drag the render thread saw, if the UI
    /// has not taken it yet.
    pub fn take_static_drag_warning(&self) -> Option<StaticDragWarning> {
        self.static_drag_warning.lock().ok()?.take()
    }

    /// The gizmo drags finished since the last call, oldest first.
    pub fn take_completed_drags(&self) -> Vec<CompletedDrag> {
        self.completed_drags
            .lock()
            .map(|mut drags| std::mem::take(&mut *drags))
            .unwrap_or_default()
    }

    /// Queue the SceneDB interaction gizmo mode for the render thread to apply
    /// at the next frame boundary.
    pub fn queue_gizmo(&self, mode: GizmoMode) {
        if let Ok(mut guard) = self.pending_gizmo_mode.lock() {
            *guard = Some(mode);
        }
    }

    /// Update the live transform gizmo snapping increments.
    pub fn set_gizmo_snap_settings(&self, location: f32, rotation: f32, scale: f32) {
        super::interaction::set_snap_settings(location, rotation, scale);
    }

    /// Request that the SceneDB selection is cleared next frame.
    pub fn queue_deselect(&self) {
        self.pending_deselect.store(true, Ordering::Relaxed);
    }

    /// Show or hide bloom in the viewport (the toolbar's Bloom toggle). The
    /// project's graphics settings must also enable it; see
    /// [`crate::scene::EditorPostProcess`]. Takes the state, not a toggle,
    /// so the viewport follows the toolbar even if an update is superseded.
    pub fn set_viewport_bloom(&self, enabled: bool) {
        self.viewport_bloom.store(enabled, Ordering::Release);
    }

    /// The toolbar's Realtime toggle: whether the viewport's animation
    /// (foliage wind, particles, shader-graph `time`) runs on wall time, or
    /// is frozen. See [`HelioRenderer::set_viewport_realtime`].
    pub fn set_viewport_realtime(&self, realtime: bool) {
        self.viewport_realtime.store(realtime, Ordering::Release);
    }

    /// The toolbar's view mode: the debug mode the viewport's graph renders
    /// (one a pass advertises in its `debug_views`), or 0 for the lit image.
    pub fn set_viewport_debug_mode(&self, mode: u32) {
        self.viewport_debug_mode.store(mode, Ordering::Release);
    }

    /// The level's render pipeline (World Settings): a registered pipeline id
    /// (see [`super::pipelines`]), or `None` for no pipeline. The viewport
    /// rebuilds its renderer from it at the next frame boundary, also when the
    /// asset behind the id is saved again; an id that is not registered runs
    /// no pipeline (there is no default).
    pub fn set_render_pipeline(&self, id: Option<String>) {
        self.requested_pipeline.set(id);
    }

    /// Publish `payload` for this viewport's passes under a feature's payload
    /// contract key (`None` withdraws it): editor-only data a plugin's
    /// overlay draws (a portal's labels and what it maps), beside what the
    /// scene publishes. Applied at the next frame, which it wakes.
    pub fn set_view_payload(&self, key: &'static str, payload: Option<ViewPayload>) {
        if let Ok(mut pending) = self.pending_view_payloads.lock() {
            pending.push((key, payload));
        }
    }

    /// Choose how this viewport looks at the scene (perspective,
    /// orthographic, or a fixed axis view). Applied at the next frame.
    pub fn set_view_kind(&self, kind: ViewKind) {
        self.view_kind.store(kind.to_u8(), Ordering::Release);
    }

    /// Whether this viewport follows (and leads) the camera of its
    /// [`CameraSync`] group. Unlocked, its camera is its own.
    pub fn set_camera_sync_locked(&self, locked: bool) {
        self.camera_sync_locked.store(locked, Ordering::Release);
    }

    /// Put this viewport in a camera sync group, from the next frame.
    pub fn join_camera_sync(&self, group: Arc<CameraSync>) {
        if let Ok(mut slot) = self.camera_sync_join.lock() {
            *slot = Some(group);
        }
    }

    /// The view the viewport was last asked for.
    pub fn view_kind(&self) -> ViewKind {
        ViewKind::from_u8(self.view_kind.load(Ordering::Acquire))
    }

    /// Snapshot the debug views advertised by the current editor render graph.
    pub fn viewport_debug_views(&self) -> Vec<ViewportDebugView> {
        self.viewport_debug_views
            .lock()
            .map(|views| views.clone())
            .unwrap_or_default()
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

    // ── Legacy feature commands ──
    /// Drained every frame and otherwise ignored: viewport feature state
    /// reaches the renderer through [`HelioEditorMailbox`].
    pub command_sender: mpsc::Sender<RendererCommand>,
    pub command_receiver: mpsc::Receiver<RendererCommand>,

    // ── Pending editor commands (written by UI thread, read by render thread) ──
    /// Next gizmo mode to apply; consumed at start of render_frame.
    pub pending_gizmo_mode: Arc<Mutex<Option<GizmoMode>>>,
    pending_camera_state: Arc<Mutex<Option<EditorCameraState>>>,
    /// When true, the render thread should clear SceneDB selection next frame.
    pub pending_deselect: Arc<AtomicBool>,
    /// Left-click/left-release events queued by the UI thread, drained in
    /// order at the top of every `render_frame` -- see [`PendingPointerEvent`].
    pub pending_pointer_events: Arc<Mutex<Vec<PendingPointerEvent>>>,
    pointer_events_scratch: Vec<PendingPointerEvent>,
    /// The toolbar's Bloom toggle; see [`HelioEditorMailbox::set_viewport_bloom`].
    pub viewport_bloom: Arc<AtomicBool>,
    /// The toolbar's Realtime toggle; see [`Self::set_viewport_realtime`].
    viewport_realtime: Arc<AtomicBool>,
    /// The toolbar's view mode: the debug mode the graph renders (0 lit).
    /// See [`HelioEditorMailbox::set_viewport_debug_mode`].
    viewport_debug_mode: Arc<AtomicU32>,
    /// See [`HelioEditorMailbox::set_view_kind`].
    view_kind: Arc<AtomicU8>,
    /// See [`HelioEditorMailbox::set_camera_sync_locked`].
    camera_sync_locked: Arc<AtomicBool>,
    follow_space_transitions: Arc<AtomicBool>,
    /// See [`HelioEditorMailbox::join_camera_sync`].
    camera_sync_join: Arc<Mutex<Option<Arc<CameraSync>>>>,
    camera_sync: Option<CameraSyncMember>,
    /// The view the camera is currently built for (`view_kind` as of the last
    /// frame), so a change can be noticed and drawn.
    active_view: ViewKind,
    /// Half the visible world height of an orthographic view.
    ortho_boom: f32,
    /// Debug views advertised by the active render graph, exposed to editor UI.
    viewport_debug_views: Arc<Mutex<Vec<ViewportDebugView>>>,
    pending_view_payloads: Arc<Mutex<Vec<(&'static str, Option<ViewPayload>)>>>,
    /// A fixed frame delta for deterministic captures; see
    /// [`Self::set_frame_delta_override`].
    frame_delta_override: Option<f32>,
    /// A fixed projection jitter for deterministic captures; see
    /// [`Self::set_camera_jitter_override`].
    camera_jitter_override: Option<[f32; 2]>,
    /// Whether the scene holds content that animates on its own, per world
    /// revision (it keeps a realtime viewport rendering).
    animated_content: super::animated_content::AnimatedContent,
    /// What the last frame was for (the frame policy's input).
    frame_kind: FrameKind,
    /// When something the user or the scene did last drove a frame.
    last_driven: Option<Instant>,
    /// Written by the render thread when a gizmo drag starts on a fixed-
    /// movability object; taken by the UI (`HelioEditorMailbox`).
    pub static_drag_warning: Arc<Mutex<Option<StaticDragWarning>>>,
    /// Gizmo drags that moved their object, taken by the UI for undo.
    completed_drags: Arc<Mutex<Vec<CompletedDrag>>>,

    // ── Host parts ──
    /// Applied to the plugins' render-host parts of every scene renderer
    /// this makes (tests, tools: a plugin's settings in place of the
    /// project's).
    host_part_setups: Vec<super::scene_renderer::HostPartSetup>,

    // ── Renderer State ──
    /// Error messages from mesh loading failures, drained by the UI viewport for notifications.
    pub pending_errors: Arc<Mutex<Vec<String>>>,

    inner: Option<HelioInner>,
    applied_graph_settings: Option<Arc<ProjectGraphSettings>>,
    graph_settings_cache: ProjectGraphSettingsCache,
    /// The pipeline the level asks for (World Settings); see
    /// [`HelioEditorMailbox::set_render_pipeline`].
    requested_pipeline: super::pipelines::RequestedPipeline,
    /// Where the viewport's render pipeline builds ([`PipelineBuildMode`]):
    /// blocking unless the host shows the status screen meanwhile.
    pipeline_build: super::scene_renderer::PipelineBuildMode,
    /// The editor's `construct` pump, run by the scene renderer each frame.
    construct_pump: Option<crate::scene::EditorConstructPump>,
    /// What `inner` was built from: the pipeline, or why none runs.
    applied_pipeline: Option<super::pipelines::PipelineState>,

    // ── Camera State ──
    /// The perspective camera's pose: its frame follows the ground's local
    /// vertical (the game's free camera flies the same way).
    cam: FlyCamera,
    camera_space: u32,
    active_space_traversal: bool,
    // Smoothed local-space velocity: x=right, y=up, z=forward (units/sec).
    cam_local_velocity: Vec3,
    viewport_size: (u32, u32),

    // ── TAA reset ──
    pub reset_taa_next_frame: bool,

    // ── Device loss ──
    /// Whether the device the renderer was built on was lost; a lost device
    /// is not drawn on again. A frame for another device rebuilds the
    /// renderer, and the scene's GPU copy, on it.
    device_loss: Option<crate::services::device_loss::DeviceLoss>,
    reported_device_loss: bool,

    // ── Metrics ──
    pub metrics: Arc<Mutex<RenderMetrics>>,
    pub gpu_profiler: GpuProfilerData,
    gpu_profiler_instance: u64,
    last_frame: Instant,
    frame_count: u64,
    /// The output's refresh interval, handed to the scene renderer each
    /// frame ([`Self::set_frame_budget`]).
    frame_budget: Option<std::time::Duration>,
    spike_log_config: RenderSpikeLogConfig,
    last_spike_warning: Option<Instant>,
    last_reported_gpu_frame: Option<(u64, u64)>,

    // ── Idle tracking ──
    /// Set when raw keyboard/mouse input was non-zero this frame, cleared
    /// once the camera decelerates to a stop.  Prevents the renderer from
    /// going idle the instant the user releases a key while velocity is
    /// still smoothing toward zero.
    had_camera_input: bool,
    temporal_settling: TemporalSettling,
    /// Tracks whether the editor selection or gizmo mode changed since
    /// the last rendered frame.  When false the gizmo geometry is not
    /// rebuilt.
    gizmo_dirty: bool,
    /// Frame counter used to throttle GPU profiler reads to once every
    /// N frames so a fast idle loop doesn't hammer the timing API.
    profiler_frame_counter: u32,
    /// The ground under the camera (local vertical, height above it), from
    /// the last frame.
    camera_ground: CameraGround,
    /// Log why the viewport is not idle (`PULSAR_VOXEL_ACTIVITY`).
    activity_log: bool,
    last_activity_log: Instant,
    /// Frames, and in how many of them each cause of activity held, since the
    /// last `VOXEL_ACTIVITY` line.
    activity_frames: u32,
    activity_counts: [u32; 15],
    /// `PULSAR_VOXEL_STATS`: log voxel streaming diagnostics twice a second.
    stats_log: bool,
    /// The last rendered frame's phases (`FramePhases`), for harnesses.
    last_phases: Vec<(&'static str, f32)>,
    last_stats_log: Instant,
    /// Frames rendered since the last stats line and the slowest of them
    /// (ms): the viewport's own rate, beside the renderer's GPU time.
    stats_frames: (u32, f32),
}

struct HelioInner {
    /// The scene's renderer, as the game builds it.
    scene: SceneRenderer,
    /// The device it was built on.
    device: wgpu::Device,
    queue: Arc<wgpu::Queue>,
    interaction: SceneInteraction,
    /// Frame-pacing revision; never used as a renderer-side world mirror.
    last_scene_revision: u64,
    has_rendered_frame: bool,
    /// The post-process baseline last set on this graph's resolver.
    applied_postprocess: Option<crate::scene::EditorPostProcess>,
    /// The debug mode last handed to the graph (0: lit).
    applied_debug_mode: u32,
    /// The plugins' editor line sets (`EditorLinesRegistration`), each
    /// watching the scene; created with the first sync.
    editor_lines: Option<Vec<(&'static str, Box<dyn super::render_features::EditorLines>)>>,
}

use super::project_settings::{ProjectGraphSettings, ProjectGraphSettingsCache};

impl HelioInner {
    /// Hand the queued per-view payloads to the scene; true when there were any.
    fn apply_view_payloads(&mut self, payloads: Vec<(&'static str, Option<ViewPayload>)>) -> bool {
        let mut changed = false;
        for (key, payload) in payloads {
            changed |= self.scene.set_view_payload(key, payload);
        }
        changed
    }
}

impl HelioRenderer {
    /// Publishes the debug views the current pipeline's passes advertise, for
    /// the viewport's view menu. A pipeline swap changes them.
    fn publish_debug_views(&self) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let Ok(mut views) = self.viewport_debug_views.lock() else {
            return;
        };
        let r = &inner.scene.renderer;
        let legends = r.available_debug_view_legends();
        let categories = r.available_debug_view_categories();
        *views = r
            .available_debug_views()
            .into_iter()
            .map(|view| ViewportDebugView {
                name: view.name,
                debug_mode: view.debug_mode,
                description: view.description,
                category: categories
                    .iter()
                    .find(|(mode, _)| *mode == view.debug_mode)
                    .map_or("", |(_, category)| *category),
                legend: legends
                    .iter()
                    .find(|(mode, _)| *mode == view.debug_mode)
                    .map(|(_, legend)| *legend),
            })
            .collect();
    }

    /// The scene renderer's host part of type `T` (a plugin's), once the
    /// renderer is built.
    pub fn host_part<T: super::host_parts::RenderHostPart>(&self) -> Option<&T> {
        self.inner.as_ref()?.scene.host_part::<T>()
    }

    /// The scene renderer's host part of type `T` (a plugin's), once the
    /// renderer is built.
    pub fn host_part_mut<T: super::host_parts::RenderHostPart>(&mut self) -> Option<&mut T> {
        self.inner.as_mut()?.scene.host_part_mut::<T>()
    }

    /// Apply `setup` to the host parts now and to those of every scene
    /// renderer this makes later; a later setup under the same `key`
    /// replaces it. Tests and tools (a plugin's settings in place of the
    /// project's).
    pub fn set_up_host_parts(
        &mut self,
        key: &'static str,
        setup: Arc<dyn Fn(&mut dyn super::host_parts::RenderHostPart) + Send + Sync>,
    ) {
        if let Some(inner) = self.inner.as_mut() {
            inner.scene.set_up_host_parts(setup.as_ref());
        }
        self.host_part_setups
            .retain(|(existing, _)| *existing != key);
        self.host_part_setups.push((key, setup));
    }

    /// Run the editor's `construct` pump (`crate::scene::construct_pump`) each
    /// frame, before the frame's change window closes. Set before the first
    /// frame: the scene renderer is built then.
    pub fn set_construct_pump(&mut self, pump: crate::scene::EditorConstructPump) {
        self.construct_pump = Some(pump);
    }

    /// Where this viewport's render pipeline builds. The default is blocking:
    /// the frame after the renderer exists draws the pipeline (tools and tests
    /// read it). The editor's viewport builds in the background and shows the
    /// status screen "Building the render pipeline" meanwhile (its first build
    /// of a session takes 10 to 14 s).
    pub fn set_pipeline_build(&mut self, mode: super::scene_renderer::PipelineBuildMode) {
        self.pipeline_build = mode;
    }

    pub fn new(scene_store: crate::scene::SharedScene) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        Self {
            camera_input: Arc::new(Mutex::new(CameraInput::new())),
            scene_store,
            command_sender,
            command_receiver,
            pending_gizmo_mode: Arc::new(Mutex::new(None)),
            pending_camera_state: Arc::new(Mutex::new(None)),
            pending_deselect: Arc::new(AtomicBool::new(false)),
            pending_pointer_events: Arc::new(Mutex::new(Vec::new())),
            pointer_events_scratch: Vec::new(),
            // Matches the toolbar's default until the UI reports its state.
            viewport_bloom: Arc::new(AtomicBool::new(true)),
            viewport_realtime: Arc::new(AtomicBool::new(true)),
            viewport_debug_mode: Arc::new(AtomicU32::new(0)),
            view_kind: Arc::new(AtomicU8::new(ViewKind::Perspective.to_u8())),
            camera_sync_locked: Arc::new(AtomicBool::new(true)),
            follow_space_transitions: Arc::new(AtomicBool::new(false)),
            camera_sync_join: Arc::new(Mutex::new(None)),
            camera_sync: None,
            active_view: ViewKind::Perspective,
            ortho_boom: CAMERA_PIVOT_DISTANCE as f32,
            viewport_debug_views: Arc::new(Mutex::new(Vec::new())),
            pending_view_payloads: Arc::new(Mutex::new(Vec::new())),
            frame_delta_override: None,
            camera_jitter_override: None,
            animated_content: Default::default(),
            frame_kind: FrameKind::Interactive,
            last_driven: None,
            static_drag_warning: Arc::new(Mutex::new(None)),
            completed_drags: Arc::new(Mutex::new(Vec::new())),
            host_part_setups: Vec::new(),
            reset_taa_next_frame: false,
            device_loss: None,
            reported_device_loss: false,
            inner: None,
            applied_graph_settings: None,
            graph_settings_cache: ProjectGraphSettingsCache::new(),
            requested_pipeline: Default::default(),
            pipeline_build: super::scene_renderer::PipelineBuildMode::Blocking,
            construct_pump: None,
            applied_pipeline: None,
            pending_errors: Arc::new(Mutex::new(Vec::new())),
            cam: FlyCamera::new(DVec3::new(8.0, 6.0, 12.0), -0.5, -0.3),
            camera_space: 0,
            active_space_traversal: false,
            cam_local_velocity: Vec3::ZERO,
            viewport_size: (0, 0),
            metrics: Arc::new(Mutex::new(RenderMetrics::default())),
            gpu_profiler: GpuProfilerData::default(),
            gpu_profiler_instance: 0,
            last_frame: Instant::now(),
            frame_count: 0,
            frame_budget: None,
            spike_log_config: RenderSpikeLogConfig::default(),
            last_spike_warning: None,
            last_reported_gpu_frame: None,
            had_camera_input: false,
            temporal_settling: TemporalSettling::default(),
            gizmo_dirty: true,
            profiler_frame_counter: 0,
            camera_ground: CameraGround::default(),
            activity_log: std::env::var_os("PULSAR_VOXEL_ACTIVITY").is_some(),
            last_activity_log: Instant::now(),
            activity_frames: 0,
            activity_counts: [0; 15],
            stats_log: std::env::var_os("PULSAR_VOXEL_STATS").is_some(),
            last_phases: Vec::new(),
            last_stats_log: Instant::now(),
            stats_frames: (0, 0.0),
        }
    }

    /// Camera pose with yaw and pitch of the world-space view direction
    /// (world Y up), independent of the camera's reference frame.
    pub fn editor_camera_state(&self) -> EditorCameraState {
        let (forward, _, _) = self.camera_basis();
        let (yaw, pitch) = yaw_pitch(forward);
        EditorCameraState {
            position: self.cam.position.to_array(),
            yaw,
            pitch,
        }
    }

    pub fn set_editor_camera_state(&mut self, state: EditorCameraState) {
        self.camera_space = 0;
        // Over a voxel world the frame at the new position is known next frame.
        self.cam.set_pose(
            DVec3::from_array(state.position),
            direction(state.yaw, state.pitch),
        );
        self.cam_local_velocity = Vec3::ZERO;

        if let Ok(mut input) = self.camera_input.lock() {
            input.forward = 0.0;
            input.right = 0.0;
            input.up = 0.0;
            input.clear_transient_deltas();
        }
    }

    /// The ground under the camera as of the last frame, as the scene's
    /// content reports it: its local vertical and the camera's height above
    /// it (what the camera's speed scales with).
    pub fn camera_ground(&self) -> CameraGround {
        self.camera_ground
    }

    /// The last measured frame's GPU milliseconds in all and by pass, for
    /// harnesses.
    pub fn gpu_timings(&self) -> Option<(f32, Vec<(&'static str, f32)>)> {
        let snapshot = self.inner.as_ref()?.scene.renderer.timing_snapshot();
        Some((
            snapshot.total_gpu_ms?,
            snapshot
                .passes
                .iter()
                .filter_map(|p| Some((p.name, p.gpu_ms?)))
                .collect(),
        ))
    }

    /// The passes the frame's graph schedules: its members, the passes whose
    /// content the scene holds this frame (`helio::Renderer::graph_members`).
    pub fn graph_members(&self) -> Vec<&'static str> {
        self.inner
            .as_ref()
            .map_or_else(Vec::new, |inner| inner.scene.renderer.graph_members())
    }

    /// GPU milliseconds of each secondary view the frame rendered (a capture,
    /// such as a reflection; `helio_core::view`): the requesting pass, the view's
    /// key and the sum of its passes' GPU times. A graph's own span
    /// (`__graph_*`) is left out: a view-rendering pass submits in the middle
    /// of its graph, so the span also holds the time the CPU spent recording
    /// the views. [`Self::gpu_timings`] is the primary view's.
    pub fn view_gpu_timings(&self) -> Vec<(&'static str, u64, f32)> {
        self.inner.as_ref().map_or_else(Vec::new, |inner| {
            inner
                .scene
                .renderer
                .view_timing_snapshots()
                .into_iter()
                .map(|(pass, key, snapshot)| {
                    let ms = snapshot
                        .passes
                        .iter()
                        .filter(|timing| !timing.name.starts_with("__"))
                        .filter_map(|timing| timing.gpu_ms)
                        .sum();
                    (pass, key, ms)
                })
                .collect()
        })
    }

    /// The last rendered frame's milliseconds by phase (pointer, scene,
    /// publish, render, ...), for harnesses.
    pub fn last_frame_phases(&self) -> &[(&'static str, f32)] {
        &self.last_phases
    }

    /// The renderer's diagnostics lines (`PULSAR_VOXEL_STATS`), for
    /// harnesses.
    pub fn diagnostics(&self) -> Vec<String> {
        self.inner
            .as_ref()
            .map_or_else(Vec::new, |inner| inner.scene.diagnostics())
    }

    /// The pipeline's state and the last rejected shader edit (see
    /// `SceneRenderer::health`); `None` before the renderer is built.
    pub fn health(&self) -> Option<(helio::PipelineStatus, Option<String>)> {
        self.inner.as_ref().map(|inner| inner.scene.health())
    }

    /// The camera's view direction in world space.
    pub fn camera_forward(&self) -> Vec3 {
        self.camera_basis().0
    }

    /// The up the frame camera is built with, in world space: over a voxel
    /// world the local vertical (the camera's frame), else +Y.
    pub fn camera_up(&self) -> Vec3 {
        self.active_view
            .axis_basis()
            .map_or(self.cam.frame_up(), |(_, _, up)| up)
    }

    /// World-space forward, right and up of the editor camera.
    fn camera_basis(&self) -> (Vec3, Vec3, Vec3) {
        self.active_view
            .axis_basis()
            .unwrap_or_else(|| self.cam.basis())
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

    /// Half the height of the orthographic view: what a 45 degree perspective
    /// camera sees at the boom length.
    fn ortho_half_height(&self) -> f32 {
        self.ortho_boom * std::f32::consts::FRAC_PI_8.tan()
    }

    fn sync_pivot_distance(&self) -> f64 {
        self.camera_sync
            .as_ref()
            .map_or(CAMERA_PIVOT_DISTANCE, |member| member.group.pivot_distance)
    }

    /// The point of the world this viewport is centred on, which the viewports
    /// of a split view share. An orthographic view looks at its camera position;
    /// the perspective camera looks at a point [`CAMERA_PIVOT_DISTANCE`] ahead.
    fn sync_anchor(&self) -> DVec3 {
        if self.active_view.is_orthographic() {
            self.cam.position
        } else {
            self.cam.position + self.camera_basis().0.as_dvec3() * self.sync_pivot_distance()
        }
    }

    /// Move the camera so the viewport is centred on `anchor`.
    fn set_sync_anchor(&mut self, anchor: DVec3) {
        self.cam.position = if self.active_view.is_orthographic() {
            anchor
        } else {
            anchor - self.camera_basis().0.as_dvec3() * self.sync_pivot_distance()
        };
    }

    /// Keep this camera's position in step with the others of its sync group.
    /// Returns whether it was moved by one of them.
    fn sync_camera(&mut self) -> bool {
        if let Some(group) = self
            .camera_sync_join
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
        {
            // Start as far out as the group was set up for, and adopt where it
            // is centred from the first frame.
            self.ortho_boom = group.pivot_distance as f32;
            self.camera_sync = Some(CameraSyncMember {
                group,
                seen_generation: u64::MAX,
                synced_position: self.sync_anchor(),
                was_locked: true,
            });
        }
        let locked = self.camera_sync_locked.load(Ordering::Acquire);
        let anchor = self.sync_anchor();
        let Some(member) = self.camera_sync.as_mut() else {
            return false;
        };
        if !locked {
            member.was_locked = false;
            return false;
        }
        if !member.was_locked {
            // Back in the group: rejoin where it is, not where this camera
            // wandered off to.
            member.was_locked = true;
            member.synced_position = anchor;
            member.seen_generation = u64::MAX;
        }

        let mut shared = member.group.shared.lock();
        if (anchor - member.synced_position).length() > SYNC_EPSILON {
            // Moved by the user: the others follow.
            shared.generation += 1;
            shared.position = Some(anchor.to_array());
            member.seen_generation = shared.generation;
            member.synced_position = anchor;
            return false;
        }
        let adopted = match shared.position {
            Some(position) if shared.generation != member.seen_generation => {
                member.seen_generation = shared.generation;
                member.synced_position = DVec3::from_array(position);
                Some(member.synced_position)
            }
            _ => None,
        };
        drop(shared);
        let Some(adopted) = adopted else {
            return false;
        };
        self.set_sync_anchor(adopted);
        // The temporal filters see a moving camera.
        self.had_camera_input = true;
        true
    }

    /// Adopt the view the editor asked for. Returns whether it changed.
    fn apply_view_kind(&mut self) -> bool {
        let wanted = ViewKind::from_u8(self.view_kind.load(Ordering::Acquire));
        if wanted == self.active_view {
            return false;
        }
        self.active_view = wanted;
        // The projection changed under the temporal history.
        self.reset_taa_next_frame = true;
        self.gizmo_dirty = true;
        true
    }

    /// Input for a fixed-orientation orthographic view: the mouse pans the
    /// view plane 1:1 (it never turns the camera) and the movement keys move
    /// along the plane's right and up axes, scaled to the zoom.
    fn apply_axis_view_input(&mut self, input: &CameraInput, dt: f32) {
        let (_, right, up) = self.camera_basis();
        let units_per_pixel = 2.0 * self.ortho_half_height() / input.viewport_height.max(1.0);

        let speed = (self.ortho_half_height() / 10.0)
            * if input.boost {
                input.move_speed * 3.0
            } else {
                input.move_speed
            };
        self.cam_local_velocity = Vec3::new(input.right * speed, input.up * speed, 0.0);
        self.cam.position +=
            ((right * self.cam_local_velocity.x + up * self.cam_local_velocity.y) * dt).as_dvec3();

        // Grab convention: dragging right moves the content right, so the
        // camera goes left; dragging down moves it down, so the camera goes up.
        let drag_x = input.mouse_delta_x + input.pan_delta_x;
        let drag_y = input.mouse_delta_y + input.pan_delta_y;
        self.cam.position += ((-right * drag_x + up * drag_y) * units_per_pixel).as_dvec3();
    }

    /// Hand the graph the view mode the toolbar chose. Returns whether it
    /// changed this frame.
    fn apply_viewport_debug_mode(&mut self) -> bool {
        let Some(inner) = self.inner.as_mut() else {
            return false;
        };
        let mode = self.viewport_debug_mode.load(Ordering::Acquire);
        if inner.applied_debug_mode == mode {
            return false;
        }
        inner.scene.renderer.set_debug_mode(mode);
        inner.applied_debug_mode = mode;
        tracing::info!(mode, "Editor viewport debug view changed");
        true
    }

    /// Keep the viewport's post-process baseline in step with the toolbar's
    /// Bloom toggle and the project's graphics settings: a renderer setting
    /// ([`crate::scene::EditorPostProcess`]), published as the frame's
    /// `helio_pass_postprocess::POST_BASELINE` (a view payload), so nothing
    /// is written into the scene. Returns whether it changed this frame.
    /// Also drains the legacy feature commands, which carry no state.
    fn apply_postprocess_baseline(&mut self) -> bool {
        while let Ok(command) = self.command_receiver.try_recv() {
            match command {
                RendererCommand::ToggleFeature(feature) => {
                    tracing::debug!(%feature, "Feature toggle received; viewport state arrives through the editor mailbox");
                }
            }
        }
        let desired = crate::scene::EditorPostProcess::from_project_settings(
            self.viewport_bloom.load(Ordering::Acquire),
        );
        let Some(inner) = self.inner.as_mut() else {
            return false;
        };
        if inner.applied_postprocess == Some(desired) {
            return false;
        }
        inner.scene.set_view_payload(
            helio_pass_postprocess::POST_BASELINE.key(),
            Some(Arc::new(desired.settings())),
        );
        inner.applied_postprocess = Some(desired);
        tracing::info!(
            bloom = desired.bloom_enabled,
            intensity = desired.bloom_intensity,
            "Editor viewport post-process updated"
        );
        true
    }

    /// The post-process baseline the editor viewport publishes, once the
    /// renderer is initialized and has published one.
    pub fn postprocess_defaults(&mut self) -> Option<helio_pass_postprocess::PostProcessSettings> {
        let inner = self.inner.as_mut()?;
        inner
            .applied_postprocess
            .map(|baseline| baseline.settings())
    }

    pub fn is_initialized(&self) -> bool {
        self.inner.is_some()
    }

    /// Whether the next [`prepare_for_render`](Self::prepare_for_render) builds
    /// a graph: the first one, a pipeline the level asks for that is not the
    /// one built, or graph-affecting project settings that changed. A build
    /// compiles pipelines (seconds on a cold driver), so a scheduler runs it off
    /// its frame loop.
    /// What the last frame was for: drawn for the user or the scene, for
    /// content that moves on its own, or skipped because nothing changed.
    pub fn frame_kind(&self) -> FrameKind {
        self.frame_kind
    }

    pub fn needs_build(&mut self) -> bool {
        let Some(applied) = self.applied_pipeline.as_ref() else {
            return true;
        };
        if self.inner.is_none() {
            return true;
        }
        let settings_changed = self.applied_graph_settings.as_ref().is_some_and(|applied| {
            let current = self.graph_settings_cache.load();
            !Arc::ptr_eq(applied, &current) && !applied.same_graph(&current)
        });
        settings_changed
            || *applied
                != super::pipelines::pipeline_state(self.requested_pipeline.get().as_deref())
    }

    /// Drop the Helio renderer (graph, passes and their GPU resources). The
    /// scene, camera and settings stay; the next
    /// [`prepare_for_render`](Self::prepare_for_render) builds it again.
    pub fn release_gpu_resources(&mut self) {
        self.inner = None;
        self.applied_graph_settings = None;
        self.applied_pipeline = None;
        self.reset_taa_next_frame = true;
    }

    /// Estimated bytes of the textures the current render graph allocates.
    pub fn graph_texture_bytes(&self) -> Option<u64> {
        self.inner
            .as_ref()
            .map(|inner| inner.scene.renderer.graph_texture_bytes())
    }

    pub fn get_metrics(&self) -> RenderMetrics {
        self.metrics.lock().map(|m| m.clone()).unwrap_or_default()
    }

    pub fn get_gpu_profiler_data(&self) -> GpuProfilerData {
        self.gpu_profiler.clone()
    }

    /// Helio's scene depth buffer (`Depth32Float`, `COPY_SRC`) as of the last
    /// encoded frame; `None` before the first. Read-only diagnostics: lets a
    /// test tell geometry that rasterized but shaded black from geometry
    /// that was never drawn.
    pub fn debug_depth_texture(&self) -> Option<&wgpu::Texture> {
        self.inner
            .as_ref()
            .map(|inner| inner.scene.renderer.debug_depth_texture())
    }

    /// The graph's pass of type `T` as of the last encoded frame; `None`
    /// before the renderer is initialized or when the graph has none.
    /// Read-only diagnostics: lets a plugin's test read its pass's state back
    /// (a water simulation's height field).
    pub fn debug_pass<T: helio_core::RenderPass + 'static>(&self) -> Option<&T> {
        self.inner
            .as_ref()
            .and_then(|inner| inner.scene.renderer.find_pass::<T>())
    }

    /// The default graph's passes, in execution order, and whether each
    /// recorded CPU work in the last frame (Helio's timing snapshot).
    /// Read-only diagnostics: lets a test show every pass of the graph runs.
    pub fn debug_pass_activity(&self) -> Vec<(String, bool)> {
        let Some(inner) = self.inner.as_ref() else {
            return Vec::new();
        };
        let timed = &inner.scene.renderer.timing_snapshot().passes;
        inner
            .scene
            .renderer
            .graph_timeline()
            .passes
            .into_iter()
            .map(|pass| {
                let ran = timed
                    .iter()
                    .any(|t| t.name == pass.name && t.cpu_ms.is_some());
                (pass.name, ran)
            })
            .collect()
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

    /// The ray under view point (`norm_x`, `norm_y`) (0..1 from the top
    /// left) from the camera's eye, world frame: zero when degenerate.
    fn pointer_ray(&self, norm_x: f32, norm_y: f32) -> (DVec3, DVec3) {
        let (width, height) = self.viewport_size;
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let (forward, right, up) = self.camera_basis();
        let tan = (std::f32::consts::FRAC_PI_4 * 0.5).tan();
        let x = norm_x.clamp(0.0, 1.0) * 2.0 - 1.0;
        let y = 1.0 - norm_y.clamp(0.0, 1.0) * 2.0;
        let direction = (forward + right * x * aspect * tan + up * y * tan)
            .normalize_or_zero()
            .as_dvec3();
        (self.cam.position, direction)
    }

    pub fn get_scene_db_selected_id(&self) -> Option<String> {
        self.scene_store.read().world.selected_id()
    }

    pub fn editor_mailbox(&self) -> HelioEditorMailbox {
        HelioEditorMailbox {
            pending_gizmo_mode: self.pending_gizmo_mode.clone(),
            pending_camera_state: self.pending_camera_state.clone(),
            pending_deselect: self.pending_deselect.clone(),
            viewport_bloom: self.viewport_bloom.clone(),
            viewport_realtime: self.viewport_realtime.clone(),
            viewport_debug_mode: self.viewport_debug_mode.clone(),
            view_kind: self.view_kind.clone(),
            camera_sync_locked: self.camera_sync_locked.clone(),
            follow_space_transitions: self.follow_space_transitions.clone(),
            camera_sync_join: self.camera_sync_join.clone(),
            viewport_debug_views: self.viewport_debug_views.clone(),
            pending_view_payloads: self.pending_view_payloads.clone(),
            static_drag_warning: self.static_drag_warning.clone(),
            completed_drags: self.completed_drags.clone(),
            requested_pipeline: self.requested_pipeline.clone(),
        }
    }

    /// Whether the viewport is realtime (the toolbar's Realtime toggle,
    /// on by default): its animation (foliage wind, particles, shader-graph
    /// `time`) runs on wall time, and it keeps rendering while the scene
    /// holds such content. Off, the animation is frozen and the viewport
    /// renders only when something changes. Play-in-Editor's game renders
    /// on the game clock instead (`pulsar_game`).
    pub fn set_viewport_realtime(&self, realtime: bool) {
        self.viewport_realtime.store(realtime, Ordering::Release);
    }

    /// Advance every frame by exactly `seconds` instead of the measured
    /// frame time (Helio's `Renderer::set_frame_delta_override`): the
    /// temporal filters and, while realtime, the animation clock. For
    /// deterministic captures and tests; `None` restores measured time.
    pub fn set_frame_delta_override(&mut self, seconds: Option<f32>) {
        assert!(seconds.is_none_or(|v| v.is_finite() && v > 0.0));
        self.frame_delta_override = seconds;
    }

    /// Fix the projection's sub-pixel jitter (Helio's
    /// `Renderer::set_camera_jitter_override`, in render pixels), so two
    /// frames of an unchanged scene rasterize alike. For deterministic
    /// captures and tests; `None` restores the temporal sequence.
    pub fn set_camera_jitter_override(&mut self, jitter: Option<[f32; 2]>) {
        assert!(jitter.is_none_or(|v| v.iter().all(|x| x.is_finite())));
        self.camera_jitter_override = jitter;
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

    /// The newest frame's named statistics (`helio::stats`: draw calls,
    /// vertices, instances and whatever passes add), a frame or two behind;
    /// `None` before the renderer is built.
    pub fn frame_stats(&self) -> Option<std::sync::Arc<helio::StatsSnapshot>> {
        self.inner
            .as_ref()
            .map(|inner| inner.scene.renderer.frame_stats().latest())
    }

    /// How long a frame should take: the output's refresh interval (`None`:
    /// unpaced). Passes that stream work in size it against this
    /// (`helio::Renderer::set_frame_budget`).
    pub fn set_frame_budget(&mut self, budget: Option<std::time::Duration>) {
        self.frame_budget = budget;
    }

    pub fn select_object_atomic(&mut self, scene_db_id: Option<String>) -> bool {
        let exists = {
            let mut scene = self.scene_store.write();
            let entity = scene_db_id
                .as_deref()
                .and_then(|id| scene.world.entity_for(id));
            scene.world.select(entity);
            scene_db_id.is_none() || entity.is_some()
        };
        if let Some(inner) = &mut self.inner {
            inner.interaction.cancel_drag();
        }
        self.gizmo_dirty = true;
        exists
    }

    /// A world-space ray from normalized viewport coordinates: from the
    /// camera, along its basis through the pixel (a 45 degree view). No
    /// matrix holds the camera's position: on a planet an f32 one is off by
    /// metres.
    fn build_pick_ray(&self, norm_x: f32, norm_y: f32) -> (DVec3, Vec3) {
        let (width, height) = self.viewport_size;
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let tan = std::f32::consts::FRAC_PI_8.tan();
        let (forward, right, up) = self.camera_basis();
        if self.active_view.is_orthographic() {
            // Parallel rays: each pixel's ray starts on the image plane.
            let half_height = self.ortho_half_height();
            let offset = right * ((norm_x * 2.0 - 1.0) * half_height * aspect)
                + up * ((1.0 - norm_y * 2.0) * half_height);
            return (self.view_eye() + offset.as_dvec3(), forward);
        }
        let direction = forward
            + right * ((norm_x * 2.0 - 1.0) * tan * aspect)
            + up * ((1.0 - norm_y * 2.0) * tan);
        (self.cam.position, direction.normalize_or_zero())
    }

    /// World-space placement target on the editor's ground plane (y = 0).
    /// Uses the same perspective/orthographic camera ray as object picking so
    /// drag placement stays aligned with the pixel under the pointer.
    pub fn editor_placement_position(
        &self,
        norm_x: f32,
        norm_y: f32,
        ignore_id: Option<&str>,
    ) -> Option<[f64; 3]> {
        let (origin, direction) = self.build_pick_ray(norm_x, norm_y);
        if let Some(inner) = &self.inner {
            let scene = self.scene_store.read();
            if let Some(point) =
                inner
                    .interaction
                    .placement_point(&scene.world, origin, direction, ignore_id)
            {
                return Some(point.to_array());
            }
        }
        let ground_distance = if direction.y.abs() >= 1.0e-6 {
            let distance = -origin.y as f32 / direction.y;
            (distance.is_finite() && distance >= 0.0).then_some(distance as f64)
        } else {
            None
        };
        // Above the horizon, keep the object under the cursor on a camera
        // facing plane instead of snapping it back to the scene origin.
        let distance = ground_distance.unwrap_or(10.0);
        Some((origin + direction.as_dvec3() * distance).to_array())
    }

    pub fn handle_left_click(&mut self, norm_x: f32, norm_y: f32) {
        self.configure_gizmo_view();
        self.gizmo_dirty = true;
        let (ray_origin, ray_direction) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };
        let store = self.scene_store.read();
        if inner
            .interaction
            .try_start_drag(&store.world, ray_origin, ray_direction)
        {
            // What the objects' mesh and light instances author: the flag
            // the caches the drag would invalidate key on.
            let fixed = store.world.selection().into_iter().find_map(|selected| {
                // Each object itself, or anything dragging it would also move
                // (a group's contents); a rigid frame's members stay fixed in
                // its space.
                let (entity, movability) =
                    crate::scene::first_fixed_in_group(&store.world, selected)?;
                Some(StaticDragWarning {
                    object_id: store
                        .world
                        .stable_id_of(entity)
                        .unwrap_or_default()
                        .to_string(),
                    object_name: store
                        .world
                        .get::<crate::scene::Name>(entity)
                        .map(|name| name.0.clone())
                        .unwrap_or_default(),
                    movability,
                })
            });
            if let (Some(warning), Ok(mut slot)) = (fixed, self.static_drag_warning.lock()) {
                *slot = Some(warning);
            }
            return;
        }
        let target = inner
            .interaction
            .pick(&store.world, ray_origin, ray_direction);
        drop(store);
        self.select_object_atomic(target);
    }

    pub fn handle_mouse_move(&mut self, norm_x: f32, norm_y: f32) {
        self.configure_gizmo_view();
        let (ray_origin, ray_direction) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };
        if inner.interaction.is_dragging() {
            let mut store = self.scene_store.write();
            inner
                .interaction
                .update_drag(&mut store.world, ray_origin, ray_direction);
            self.gizmo_dirty = true;
        } else {
            let store = self.scene_store.read();
            self.gizmo_dirty |=
                inner
                    .interaction
                    .update_hover(&store.world, ray_origin, ray_direction);
        }
    }

    pub fn handle_left_release(&mut self) {
        self.gizmo_dirty = true;
        let Some(inner) = &mut self.inner else { return };
        let Some(started) = inner.interaction.finish_drag() else {
            return;
        };
        let moved: Vec<_> = {
            let store = self.scene_store.read();
            started
                .into_iter()
                .filter(|(entity, before)| {
                    store
                        .world
                        .get::<crate::scene::Transform>(*entity)
                        .is_some_and(|now| now != before)
                })
                .collect()
        };
        if !moved.is_empty() {
            if let Ok(mut drags) = self.completed_drags.lock() {
                drags.push(CompletedDrag { moved });
            }
        }
    }

    /// Select what a Shift-dragged box covers ([`SceneInteraction::box_pick`]),
    /// replacing the selection. A box under a few pixels was a Shift-click:
    /// it adds the object under the pointer to the selection (as the new
    /// primary), or takes it out if it was already selected.
    pub fn handle_box_select(&mut self, from: [f32; 2], to: [f32; 2]) {
        const CLICK_PIXELS: f32 = 4.0;
        self.configure_gizmo_view();
        self.gizmo_dirty = true;
        let (width, height) = self.viewport_size;
        let size = Vec2::new(width.max(1) as f32, height.max(1) as f32);
        let (from, to) = (Vec2::from_array(from), Vec2::from_array(to));
        let click = ((to - from) * size).abs().max_element() < CLICK_PIXELS;
        let (origin, direction) = self.build_pick_ray(to.x, to.y);
        let Some(inner) = &mut self.inner else { return };
        inner.interaction.cancel_drag();
        let mut store = self.scene_store.write();
        let world = &mut store.world;
        if click {
            let Some(hit) = inner
                .interaction
                .pick(world, origin, direction)
                .and_then(|id| world.entity_for(&id))
            else {
                return;
            };
            let mut selection = world.selection();
            if let Some(at) = selection.iter().position(|e| *e == hit) {
                selection.remove(at);
            } else {
                selection.push(hit);
            }
            world.select_all(&selection);
            return;
        }
        let picked = inner.interaction.box_pick(world, from, to);
        world.select_all(&picked);
    }
    fn configure_gizmo_view(&mut self) {
        let (forward, _, up) = self.camera_basis();
        let (w, h) = self.viewport_size;
        let size = self
            .camera_input
            .lock()
            .ok()
            .map(|input| glam::Vec2::new(input.viewport_width, input.viewport_height))
            .filter(|size| size.x > 1.0 && size.y > 1.0)
            .unwrap_or(glam::Vec2::new(w.max(1) as f32, h.max(1) as f32));
        let aspect = w.max(1) as f32 / h.max(1) as f32;
        let half_height_for_range = self.ortho_half_height();
        let ortho = self
            .active_view
            .is_orthographic()
            .then_some(self.ortho_half_height());
        let projection = match ortho {
            Some(half_height) => glam::camera::rh::proj::directx::orthographic(
                -half_height * aspect,
                half_height * aspect,
                -half_height,
                half_height,
                0.1,
                10_000.0,
            ),
            None => glam::camera::rh::proj::directx::perspective(
                std::f32::consts::FRAC_PI_4,
                aspect,
                0.1,
                10_000.0,
            ),
        };
        // With the eye at the origin: the gizmo works in offsets from it.
        let view = glam::camera::rh::view::look_at_mat4(Vec3::ZERO, forward, up);
        let eye = self.view_eye();
        if let Some(inner) = &mut self.inner {
            inner.interaction.set_view(
                eye,
                forward,
                projection * view,
                size,
                10_000.0f32.max(4.0 * ortho_depth_range(half_height_for_range)),
            );
            inner.interaction.set_orthographic(ortho);
        }
    }

    /// Where the view is taken from: the camera, or for an orthographic view
    /// the point [`ortho_depth_range`] behind its focus.
    fn view_eye(&self) -> DVec3 {
        if self.active_view.is_orthographic() {
            // Behind everything the view reaches, so picks and the gizmo see
            // objects on both sides of the focus (this is not the render eye).
            let (forward, _, _) = self.camera_basis();
            let range = ortho_depth_range(self.ortho_half_height());
            self.cam.position - forward.as_dvec3() * f64::from(range)
        } else {
            self.cam.position
        }
    }
}

#[cfg(test)]
mod temporal_settling_tests {
    use super::{TemporalSettling, TEMPORAL_SETTLING_FRAMES};

    #[test]
    fn stopped_camera_renders_exactly_one_bounded_history_budget() {
        let mut settling = TemporalSettling::default();
        assert!(!settling.needs_frame());
        settling.observe_activity(true, true);
        settling.complete_frame(true, true, true);
        for frame in 0..TEMPORAL_SETTLING_FRAMES {
            // Quiet renders must not rearm themselves merely because they run.
            settling.observe_activity(true, false);
            assert!(settling.needs_frame(), "stopped frame {frame}");
            settling.complete_frame(true, false, true);
        }
        for _ in 0..100 {
            settling.observe_activity(true, false);
            assert!(!settling.needs_frame());
            settling.complete_frame(true, false, false);
        }
    }

    #[test]
    fn skipped_or_failed_renders_do_not_consume_history_and_new_activity_rearms() {
        let mut settling = TemporalSettling::default();
        settling.observe_activity(true, true);
        for _ in 0..12 {
            settling.complete_frame(true, false, true);
        }
        for _ in 0..100 {
            settling.observe_activity(true, false);
            settling.complete_frame(true, false, false);
        }
        assert_eq!(settling.remaining, TEMPORAL_SETTLING_FRAMES - 12);
        // Scene edits, loading and camera activity all use the same explicit arm.
        settling.observe_activity(true, true);
        assert_eq!(settling.remaining, TEMPORAL_SETTLING_FRAMES);
        settling.complete_frame(true, true, true);
        assert_eq!(settling.remaining, TEMPORAL_SETTLING_FRAMES);
    }

    #[test]
    fn installing_temporal_graph_on_activity_frame_arms_history_after_render() {
        let mut settling = TemporalSettling::default();
        // The idle check sees the old graph before a scene edit installs TSR.
        settling.observe_activity(false, true);
        assert!(!settling.needs_frame());
        settling.complete_frame(true, true, true);
        assert_eq!(settling.remaining, TEMPORAL_SETTLING_FRAMES);
        for _ in 0..TEMPORAL_SETTLING_FRAMES {
            settling.observe_activity(true, false);
            assert!(settling.needs_frame());
            settling.complete_frame(true, false, true);
        }
        assert!(!settling.needs_frame());
        // The inverse graph change must leave no unnecessary settling frames.
        settling.observe_activity(true, true);
        settling.complete_frame(false, true, true);
        assert!(!settling.needs_frame());
    }

    #[test]
    fn non_temporal_graph_returns_immediately_to_idle() {
        let mut settling = TemporalSettling::default();
        settling.observe_activity(true, true);
        settling.observe_activity(false, true);
        assert!(!settling.needs_frame());
        settling.observe_activity(false, false);
        assert!(!settling.needs_frame());
    }
}

#[cfg(test)]
mod view_kind_tests {
    use super::*;

    #[test]
    fn axis_views_are_right_handed_orthonormal_bases() {
        for kind in [ViewKind::Top, ViewKind::Front, ViewKind::Side] {
            let (forward, right, up) = kind.axis_basis().expect("axis view");
            assert!((forward.length() - 1.0).abs() < 1e-6);
            assert!(forward.dot(right).abs() < 1e-6 && forward.dot(up).abs() < 1e-6);
            // Camera right is forward x up, as in the perspective camera.
            assert!(forward.cross(up).abs_diff_eq(right, 1e-6), "{kind:?}");
        }
        assert!(ViewKind::Perspective.axis_basis().is_none());
        assert!(ViewKind::Orthographic.axis_basis().is_none());
    }

    #[test]
    fn view_kind_survives_the_mailbox_encoding() {
        for kind in [
            ViewKind::Perspective,
            ViewKind::Orthographic,
            ViewKind::Top,
            ViewKind::Front,
            ViewKind::Side,
        ] {
            assert_eq!(ViewKind::from_u8(kind.to_u8()), kind);
        }
        assert_eq!(ViewKind::from_u8(200), ViewKind::Perspective);
    }

    #[test]
    fn the_orthographic_camera_centres_its_focus_and_scales_by_half_height() {
        let focus = DVec3::new(100.0, 5.0, -40.0);
        let (forward, _, up) = ViewKind::Front.axis_basis().unwrap();
        let camera = ortho_camera(focus, forward, up, 2.0, 10.0);
        let ndc = |p: Vec3| {
            let clip = camera.view_proj_in(DVec3::ZERO) * p.extend(1.0);
            clip.truncate() / clip.w
        };

        let centre = ndc(focus.as_vec3());
        assert!(centre.x.abs() < 1e-4 && centre.y.abs() < 1e-4, "{centre:?}");
        // Ten units up is the top edge; twenty to the right (aspect 2) the
        // right edge, at the same depth as the focus.
        let top = ndc(focus.as_vec3() + Vec3::Y * 10.0);
        let right = ndc(focus.as_vec3() + Vec3::X * 20.0);
        assert!((top.y - 1.0).abs() < 1e-4 && (right.x - 1.0).abs() < 1e-4);
        // Geometry in front of and behind the focus is inside the depth range.
        for depth in [-1000.0, 1000.0] {
            let z = ndc(focus.as_vec3() + Vec3::Z * depth).z;
            assert!((0.0..=1.0).contains(&z), "z {z} at {depth}");
        }
    }

    #[test]
    fn the_orthographic_projection_follows_the_reversed_depth_convention() {
        let (forward, _, up) = ViewKind::Top.axis_basis().unwrap();
        let camera = ortho_camera(DVec3::ZERO, forward, up, 1.0, 5.0);
        // Nearer to the eye (higher in a top view) gets the larger depth.
        let depth = |y: f32| {
            let clip = camera.view_proj_in(DVec3::ZERO) * Vec3::new(0.0, y, 0.0).extend(1.0);
            clip.z / clip.w
        };
        assert!(depth(100.0) > depth(-100.0));
    }
}

#[cfg(test)]
mod camera_sync_tests {
    use super::*;

    fn renderer() -> HelioRenderer {
        HelioRenderer::new(Arc::new(
            parking_lot::RwLock::new(crate::scene::new_scene()),
        ))
    }

    fn join(renderer: &mut HelioRenderer, group: &Arc<CameraSync>) {
        renderer.editor_mailbox().join_camera_sync(group.clone());
        renderer.sync_camera();
    }

    fn near(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1.0e-6
    }

    #[test]
    fn orthographic_views_centre_on_the_point_the_perspective_camera_looks_at() {
        let mut perspective = renderer();
        let mut top = renderer();
        top.editor_mailbox().set_view_kind(ViewKind::Top);
        top.apply_view_kind();

        let state = perspective.editor_camera_state();
        let group = Arc::new(CameraSync::at(
            state.focus_point(CAMERA_PIVOT_DISTANCE),
            CAMERA_PIVOT_DISTANCE,
        ));
        join(&mut perspective, &group);
        join(&mut top, &group);

        let ahead = perspective.cam.position
            + perspective.camera_basis().0.as_dvec3() * CAMERA_PIVOT_DISTANCE;
        assert!(
            near(top.cam.position, ahead),
            "{:?} vs {ahead:?}",
            top.cam.position
        );
        // The perspective camera itself does not move.
        assert!(near(perspective.sync_anchor(), ahead));
    }

    #[test]
    fn locked_cameras_follow_the_one_the_user_moved() {
        let group = Arc::new(CameraSync::default());
        let (mut a, mut b) = (renderer(), renderer());
        b.cam.position = a.cam.position;
        join(&mut a, &group);
        join(&mut b, &group);

        a.cam.position += DVec3::new(5.0, 0.0, -3.0);
        assert!(!a.sync_camera(), "the one that moved is not moved again");
        assert!(b.sync_camera());
        assert!(near(b.cam.position, a.cam.position));
        // Nothing left to follow, and no echo back.
        assert!(!b.sync_camera());
        assert!(!a.sync_camera());
    }

    #[test]
    fn panning_an_orthographic_view_moves_the_perspective_camera() {
        let (mut perspective, mut top) = (renderer(), renderer());
        let group = Arc::new(CameraSync::at(
            perspective
                .editor_camera_state()
                .focus_point(CAMERA_PIVOT_DISTANCE),
            CAMERA_PIVOT_DISTANCE,
        ));
        top.editor_mailbox().set_view_kind(ViewKind::Top);
        top.apply_view_kind();
        join(&mut perspective, &group);
        join(&mut top, &group);
        let eye = perspective.cam.position;

        top.cam.position += DVec3::new(4.0, 0.0, 9.0);
        top.sync_camera();
        assert!(perspective.sync_camera());
        assert!(near(
            perspective.cam.position,
            eye + DVec3::new(4.0, 0.0, 9.0)
        ));
        // Still looking the same way, so no ping-pong.
        assert!(!perspective.sync_camera());
        assert!(!top.sync_camera());
    }

    #[test]
    fn an_unlocked_camera_neither_follows_nor_leads() {
        let group = Arc::new(CameraSync::default());
        let (mut a, mut b) = (renderer(), renderer());
        b.cam.position = a.cam.position;
        join(&mut a, &group);
        join(&mut b, &group);

        b.editor_mailbox().set_camera_sync_locked(false);
        assert!(!b.sync_camera());
        let before = b.cam.position;
        a.cam.position += DVec3::X * 10.0;
        a.sync_camera();
        assert!(!b.sync_camera());
        assert_eq!(b.cam.position, before, "unlocked cameras stay put");

        b.cam.position += DVec3::Y * 7.0;
        assert!(!b.sync_camera());
        assert!(!a.sync_camera(), "an unlocked move is not published");

        // Locking again rejoins the group where it is.
        b.editor_mailbox().set_camera_sync_locked(true);
        assert!(b.sync_camera());
        assert!(near(b.cam.position, a.cam.position));
    }

    #[test]
    fn scrolling_changes_the_boom_length_and_with_it_the_view() {
        let mut top = renderer();
        top.editor_mailbox().set_view_kind(ViewKind::Top);
        top.apply_view_kind();
        let before = top.ortho_half_height();
        // Default: what a 45 degree camera sees at the pivot distance.
        let expected = CAMERA_PIVOT_DISTANCE as f32 * std::f32::consts::FRAC_PI_8.tan();
        assert!((before - expected).abs() < 1e-4);

        top.camera_input.lock().unwrap().zoom_delta = 5.0;
        top.apply_camera_input(0.016);
        assert!(
            top.ortho_half_height() < before,
            "scrolling in shortens the boom"
        );
        top.camera_input.lock().unwrap().zoom_delta = -50.0;
        top.apply_camera_input(0.016);
        assert!(
            top.ortho_half_height() > before,
            "scrolling out lengthens it"
        );
    }
}

#[cfg(test)]
mod pivot_distance_tests {
    use super::*;

    fn renderer() -> HelioRenderer {
        HelioRenderer::new(Arc::new(
            parking_lot::RwLock::new(crate::scene::new_scene()),
        ))
    }

    #[test]
    fn a_group_sets_how_far_out_the_orthographic_views_start() {
        let (mut perspective, mut side) = (renderer(), renderer());
        side.editor_mailbox().set_view_kind(ViewKind::Side);
        side.apply_view_kind();

        let state = perspective.editor_camera_state();
        let group = Arc::new(CameraSync::at(state.focus_point(900.0), 900.0));
        perspective.editor_mailbox().join_camera_sync(group.clone());
        side.editor_mailbox().join_camera_sync(group);
        perspective.sync_camera();
        side.sync_camera();

        // A big scene: the side view starts zoomed out to match.
        let expected = 900.0 * std::f32::consts::FRAC_PI_8.tan();
        assert!((side.ortho_half_height() - expected).abs() < 1e-2);
        // And centres on the point 900 ahead of the perspective camera.
        let ahead = perspective.cam.position + perspective.camera_basis().0.as_dvec3() * 900.0;
        assert!((side.cam.position - ahead).length() < 1e-6);
    }
}
