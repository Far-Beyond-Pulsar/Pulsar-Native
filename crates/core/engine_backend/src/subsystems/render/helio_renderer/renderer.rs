//! Main HelioRenderer — wgpu + Helio scene renderer backed by SceneDB.

use glam::{DVec3, Mat4, Vec3};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

use helio::{Camera, Renderer, RendererConfig};

use super::core::{CameraInput, GpuProfilerData, RenderMetrics, RenderSpikeLogConfig};
use crate::scene::{GizmoType, SceneWorldExt};

use super::gpu_trace::emit_helio_gpu_passes;
use super::interaction::SceneInteraction;
use super::voxel_mesh_backend::MeshVoxelBackend;
use super::voxel_backend::{
    PlanetVoxelBackend, VoxelBackendRegistry, VoxelBrushCommit, VoxelRenderBackend, VoxelView,
};
type GizmoMode = GizmoType;

// Camera-relative frames (the world origin at the eye) keep a planet's
// coordinates exact in f32. Only consumers audited to use the same origin
// may share such a frame: any unreviewed authored component or GPU source
// keeps the frame in world coordinates.
#[derive(Default)]
struct RelativeCameraGate {
    /// The GPU store and its registry length the sources were classified at.
    snapshot: Option<(usize, usize)>,
    sources_compatible: bool,
}

impl RelativeCameraGate {
    /// Whether this frame may be camera-relative: the scene, classified once
    /// per revision (`VoxelSceneRead`), and every registered GPU source.
    fn compatible(&mut self, world: &pulsar_scenedb::World, scene_compatible: bool) -> bool {
        let Some(mirror) = world.gpu_mirror() else {
            return false;
        };
        let store = mirror.store();
        // Registrations are append-only: a new one re-classifies.
        let snapshot = (
            store as *const _ as usize,
            store.buffer_registry().len(),
        );
        if self.snapshot != Some(snapshot) {
            self.sources_compatible = relative_camera_incompatible_sources(world).is_empty();
            self.snapshot = Some(snapshot);
        }
        scene_compatible && self.sources_compatible
    }
}

/// GPU-mirror buffers that keep a frame out of camera-relative coordinates.
fn relative_camera_incompatible_sources(world: &pulsar_scenedb::World) -> Vec<String> {
    let Some(mirror) = world.gpu_mirror() else {
        return vec!["<no gpu mirror>".into()];
    };
    let store = mirror.store();
    store
        .buffer_registry()
        .telemetry_entries()
        .into_iter()
        .filter(|(key, kind, _, access, mode, _, _)| {
            !(relative_camera_source_compatible(key.as_str(), kind, *mode, *access)
                && relative_camera_source_schema_compatible(store, *key))
        })
        .map(|(key, kind, _, access, mode, _, _)| {
            format!("{} ({kind}, {mode:?}, {access:?})", key.as_str())
        })
        .collect()
}

/// A registered buffer holds the schema its key names (a host-provided
/// mirror could register another type under a reviewed key).
fn relative_camera_source_schema_compatible(
    store: &pulsar_scenedb::gpu::SceneGpuStore,
    key: pulsar_scenedb::gpu::BufferKey,
) -> bool {
    use helio_component::components as c;
    use pulsar_scenedb::component::type_of;
    let expected = match key.as_str() {
        "component_owners" => {
            pulsar_scene_model::attachments::ComponentOwner::packed_gpu_component_id()
        }
        "object_hidden" => pulsar_scene_model::ObjectHidden::packed_gpu_component_id(),
        "Transform::packed" => crate::scene::Transform::packed_gpu_component_id(),
        "light_sources" => c::LightSourceRow::packed_gpu_component_id(),
        "water_hitbox_sources" => pulsar_physics::WaterHitboxSourceRow::packed_gpu_component_id(),
        "global_fog_sources" => c::GlobalFogSourceRow::packed_gpu_component_id(),
        "local_fog_sources" => c::LocalFogSourceRow::packed_gpu_component_id(),
        "post_process_volume_sources" => c::PostProcessVolumeSourceRow::packed_gpu_component_id(),
        "camera_postprocess_sources" => c::CameraPostProcessSourceRow::packed_gpu_component_id(),
        "water_volume_sources" => c::WaterVolumeSourceRow::packed_gpu_component_id(),
        "foliage_sources" => c::FoliageSourceRow::packed_gpu_component_id(),
        "atmosphere_sources" => c::AtmosphereSourceRow::packed_gpu_component_id(),
        "decal_sources" => c::DecalSourceRow::packed_gpu_component_id(),
        "corona_emitter_sources" => c::CoronaEmitterSourceRow::packed_gpu_component_id(),
        "wind_sources" => c::GlobalWindSourceRow::packed_gpu_component_id(),
        "sprite_sources" => c::SpriteSourceRow::packed_gpu_component_id(),
        "builtin_mesh_vertex::handles" | "builtin_mesh_index::handles" => {
            return store.buffer_registry().element_type(key)
                == Some(Some(std::any::TypeId::of::<pulsar_scenedb::gpu::VarLenHandle>()));
        }
        // SceneDB's execution metadata and the mesh payload and draw fields
        // are checked by kind; their components are gated in the World.
        _ => return true,
    };
    store.buffer_registry().element_type(key) == Some(Some(type_of(expected)))
}

/// Whether a registered buffer may feed a camera-relative frame. The
/// buffers here are the scene and environment joins' inputs: what the passes
/// draw from them is decided by the authored components in the World
/// (`relative_camera_world_incompatibilities`), so a registered but unused
/// source is harmless. Anything else (a plugin's own pass rows) is not
/// reviewed for a moved origin.
fn relative_camera_source_compatible(
    key: &str,
    kind: &str,
    mode: Option<pulsar_scenedb::MirrorMode>,
    access: pulsar_scenedb::gpu::BufferAccess,
) -> bool {
    use pulsar_scenedb::{gpu::BufferAccess, MirrorMode};
    // Material textures: no positions; uploads write them.
    if key == "builtin_texture" {
        return kind == "texture_array" && mode.is_none();
    }
    if access != BufferAccess::ReadOnly {
        return false;
    }
    match key {
        "scenedb-instances" | "scenedb-instance-info" | "builtin_generation"
        | "builtin_slot_mirror" | "builtin_cell_metadata" | "component_owners"
        | "object_hidden" | "Transform::packed" | "light_sources" | "decal_sources"
        | "water_hitbox_sources" | "global_fog_sources" | "local_fog_sources"
        | "post_process_volume_sources" | "camera_postprocess_sources"
        | "water_volume_sources" | "foliage_sources" | "atmosphere_sources"
        | "corona_emitter_sources" | "wind_sources" | "sprite_sources" | "static_mesh_draw_bounds" | "static_mesh_draw_flags"
        | "builtin_mesh_vertex::handles" | "builtin_mesh_index::handles"
        | "static_mesh_draw_sections::handles" => {
            kind == "row" && mode == Some(MirrorMode::DirtyTracked)
        }
        "builtin_mesh_vertex" | "builtin_mesh_index" | "static_mesh_draw_sections" => {
            kind == "resource" && mode.is_none()
        }
        _ => false,
    }
}

/// What keeps the scene out of camera-relative frames: a positional light
/// (`positional_light`), and every component type not reviewed for a moved
/// origin. Empty when the scene is compatible.
fn relative_camera_world_incompatibilities(
    world: &pulsar_scenedb::World,
    positional_light: bool,
) -> Vec<String> {
    use crate::scene::{
        ComponentAttachments, Name, ObjectType, Parent, RenderProps, Selected, SiblingIndex,
        StableId, Transform, Visibility,
    };
    use pulsar_scenedb::component_id;
    let mut found = Vec::new();
    // Positional lights' world-space culling and shadows are not rebased.
    if positional_light {
        found.push("positional light".to_string());
    }
    let allowed = [
        // Objects and their component instances.
        component_id::<StableId>(),
        component_id::<Name>(),
        component_id::<Parent>(),
        component_id::<SiblingIndex>(),
        component_id::<Selected>(),
        component_id::<Transform>(),
        component_id::<Visibility>(),
        component_id::<ObjectType>(),
        // Editor metadata of an object: no position.
        component_id::<RenderProps>(),
        component_id::<ComponentAttachments>(),
        component_id::<helio::Movability>(),
        component_id::<pulsar_scene_model::ObjectHidden>(),
        component_id::<pulsar_scene_model::attachments::ComponentOwner>(),
        component_id::<pulsar_scene_model::attachments::ComponentMeta>(),
        // Kept data of an unregistered class: nothing draws it.
        component_id::<pulsar_scene_model::attachments::UnresolvedComponent>(),
        // The voxel world itself, traced camera-relative. (A free-standing
        // VoxelComponent draws as a world-space mesh, so it is not here.)
        component_id::<helio_component::VoxelTerrainComponent>(),
        component_id::<helio_component::VoxelTerrainLayersComponent>(),
        // Directional lights only (above); the scene join's editor light
        // icons are billboards, which subtract the origin.
        component_id::<helio_component::components::LightComponent>(),
        // The atmosphere pass subtracts the world origin from the planet's
        // centre; volume blending rebases volume bounds by it; a camera's
        // post-process baseline has no position.
        component_id::<helio_component::AtmosphereComponent>(),
        component_id::<helio_component::PostProcessVolumeComponent>(),
        component_id::<helio_component::CameraPostProcessComponent>(),
        // The global wind: a direction and speed, no position.
        component_id::<helio_component::components::WindComponent>(),
        // 2D sprites: screen space, drawn by their own camera.
        component_id::<helio_component::components::SpriteComponent>(),
    ];
    for archetype in world
        .archetypes
        .iter()
        .filter(|archetype| !archetype.entities.is_empty())
    {
        for &id in archetype.key.0.iter() {
            if !allowed.contains(&id) {
                let name = pulsar_scenedb::component::type_name(id).to_string();
                if !found.contains(&name) {
                    found.push(name);
                }
            }
        }
    }
    found
}

/// The frame camera: at the origin, looking along `forward`, for a
/// camera-relative frame; at `eye` otherwise. Orientation is built before
/// translation: adding a unit direction to a large f32 world position can
/// round away the look direction.
fn native_frame_camera(
    eye: DVec3,
    forward: Vec3,
    up: Vec3,
    aspect: f32,
    near: f32,
    far: f32,
    relative: bool,
) -> Camera {
    let mut camera = Camera::perspective_look_at(
        Vec3::ZERO,
        forward,
        up,
        std::f32::consts::FRAC_PI_4,
        aspect,
        near,
        far,
    );
    if !relative {
        camera.position = eye.as_vec3();
        camera.view = (camera.view.as_dmat4() * glam::DMat4::from_translation(-eye)).as_mat4();
    }
    camera
}

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

/// Append a backend's brush edit to the terrain's journal and advance the
/// source revision. SceneDB persists the journal with the level.
pub(super) fn apply_voxel_brush_commit(
    world: &mut pulsar_scenedb::World,
    commit: VoxelBrushCommit,
) -> bool {
    if commit.id.kind != 1 {
        return false;
    }
    let entity = pulsar_scenedb::Entity::from_bits(commit.id.entity_bits);
    let Some(mut terrain) = world.get_mut::<helio_component::VoxelTerrainComponent>(entity) else {
        return false;
    };
    if !terrain.editable {
        return false;
    }
    terrain.edits.push(commit.edit);
    terrain.edits.extend(commit.then);
    terrain.source_revision = terrain.source_revision.wrapping_add(1);
    true
}

/// A brush sample: the pointer ray when it was taken, and the pick that
/// asked the renderer for the terrain hit under it.
struct PendingBrush {
    pick: u64,
    /// The renderer's answer once it arrived: the hit (distance, cell size)
    /// or none (sky, or a column regenerating under the stroke).
    answer: Option<Option<(f64, f64)>>,
    requested: Instant,
    stroke: u64,
    origin: DVec3,
    direction: DVec3,
    request: VoxelBrushRequest,
}

/// How long a brush sample waits for the renderer's hit before it walks the
/// exact terrain without one.
const PICK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);
/// After a stamp within this distance, brush samples stamp on the frame of
/// their input: the exact walk from the eye to twice that distance
/// costs under a millisecond (0.1 ms at 10 m), where a renderer pick takes
/// frames.
const INSTANT_STAMP_M: f64 = 40.0;

/// Scripted sculpting through the editor's own brush queue: after 12 s to
/// load, it looks down (`=1`) or keeps the view and strokes the distant
/// terrain at the view's centre (`=far`), dragging a dig r1, a dig r4 and a
/// build r1 stroke for 8 s each, logging `VOXEL_NATIVE_SCULPT`.
struct NativeSculpt {
    armed_at: Instant,
    stage: Option<usize>,
    far: bool,
}

impl NativeSculpt {
    const LOAD_SECONDS: f32 = 12.0;
    const STAGE_SECONDS: f32 = 8.0;

    fn new(far: bool) -> Self {
        Self {
            armed_at: Instant::now(),
            stage: None,
            far,
        }
    }

    /// The brush event for this frame and whether a stroke starts.
    fn next(&mut self, now: Instant) -> Option<(PendingPointerEvent, bool)> {
        use helio_voxel_data::{VoxelBrushOp, VoxelBrushShape};
        let t = now.duration_since(self.armed_at).as_secs_f32() - Self::LOAD_SECONDS;
        if t < 0.0 {
            return None;
        }
        let stages = [
            ("dig_r1", VoxelBrushOp::Remove, VoxelBrushShape::Sphere, 1.0, 0),
            ("dig_r4", VoxelBrushOp::Remove, VoxelBrushShape::Sphere, 4.0, 0),
            (
                "build_r1",
                VoxelBrushOp::Add,
                VoxelBrushShape::Cube,
                1.0,
                helio_pass_voxel_planet::terrain::material::BRICK,
            ),
        ];
        let index = (t / Self::STAGE_SECONDS) as usize;
        if index >= stages.len() {
            if self.stage.take().is_some() {
                tracing::info!("VOXEL_NATIVE_SCULPT complete");
            }
            return None;
        }
        let started = self.stage != Some(index);
        let (name, op, shape, radius, material) = stages[index];
        if started {
            tracing::info!("VOXEL_NATIVE_SCULPT stage={name}");
            self.stage = Some(index);
        }
        let a = t * 1.3;
        let request = VoxelBrushRequest {
            op,
            shape,
            radius,
            material,
            single_block: false,
            level: Default::default(),
            tool: Default::default(),
        };
        let (x, y) = if self.far {
            (0.5 + 0.3 * a.cos(), 0.5 + 0.02 * a.sin())
        } else {
            (0.5 + 0.18 * a.cos(), 0.55 + 0.12 * a.sin())
        };
        Some((
            PendingPointerEvent::VoxelBrush {
                norm_x: x,
                norm_y: y,
                request,
                start: started,
            },
            started && !self.far,
        ))
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
    /// A sculpt stroke sample. `start` begins a stroke (pointer down); the
    /// drag samples after it are latest-wins, since the renderer fills the
    /// gap from the stroke's previous stamp.
    VoxelBrush {
        norm_x: f32,
        norm_y: f32,
        request: VoxelBrushRequest,
        start: bool,
    },
    LeftRelease,
}

impl PendingPointerEvent {
    /// Hover positions and stroke drag samples: only the latest matters.
    fn latest_wins(&self) -> bool {
        matches!(
            self,
            Self::MouseMove { .. } | Self::VoxelBrush { start: false, .. }
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

/// One sculpt-tool stroke sample, applied where the pointer ray first hits
/// voxel terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelBrushRequest {
    /// Remove digs into the hit block; Add builds on the face in front of
    /// it; Paint recolours solid blocks.
    pub op: helio_voxel_data::VoxelBrushOp,
    pub shape: helio_voxel_data::VoxelBrushShape,
    /// Brush radius (half size for a cube) in metres.
    pub radius: f32,
    /// Terrain material for Add and Paint.
    pub material: u32,
    /// Edit exactly one block, whatever the radius.
    pub single_block: bool,
    /// What a stamp does with the brush (see [`VoxelBrushTool`]).
    pub tool: VoxelBrushTool,
    /// Flatten: the ground height (radial, m) the stroke levels to, set by
    /// the renderer from the stroke's first stamp.
    pub level: Option<f64>,
}

/// What a sculpt stamp does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoxelBrushTool {
    /// The brush itself, with its op (dig, build, paint).
    #[default]
    Stamp,
    /// Level the ground under the brush to the height where the stroke
    /// started: carve above it, fill below it.
    Flatten,
    /// Ease the ground under the brush to its local average height.
    Smooth,
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
    static_drag_warning: Arc<Mutex<Option<StaticDragWarning>>>,
}

/// A gizmo drag started on an object whose authored movability promises a
/// fixed transform (Pulsar-Native#837). Moving it anyway leaves
/// cached data (the static shadow atlas) describing its old place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticDragWarning {
    pub object_id: String,
    pub object_name: String,
    pub movability: helio::Movability,
}

impl HelioEditorMailbox {
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
    /// The toolbar's Bloom toggle; see [`HelioEditorMailbox::set_viewport_bloom`].
    pub viewport_bloom: Arc<AtomicBool>,
    /// The toolbar's Realtime toggle; see [`Self::set_viewport_realtime`].
    viewport_realtime: Arc<AtomicBool>,
    /// A fixed frame delta for deterministic captures; see
    /// [`Self::set_frame_delta_override`].
    frame_delta_override: Option<f32>,
    /// A fixed projection jitter for deterministic captures; see
    /// [`Self::set_camera_jitter_override`].
    camera_jitter_override: Option<[f32; 2]>,
    /// Whether the scene holds content that animates on its own, per world
    /// revision (it keeps a realtime viewport rendering).
    animated_content: super::animated_content::AnimatedContent,
    /// Written by the render thread when a gizmo drag starts on a fixed-
    /// movability object; taken by the UI (`HelioEditorMailbox`).
    pub static_drag_warning: Arc<Mutex<Option<StaticDragWarning>>>,

    // ── Renderer State ──
    /// Error messages from mesh loading failures, drained by the UI viewport for notifications.
    pub pending_errors: Arc<Mutex<Vec<String>>>,

    inner: Option<HelioInner>,
    applied_graph_settings: Option<ProjectGraphSettings>,

    // ── Camera State ──
    cam_pos: DVec3,
    relative_camera_gate: RelativeCameraGate,
    /// Yaw and pitch relative to `cam_frame`.
    cam_yaw: f32,
    cam_pitch: f32,
    /// The camera's reference frame (local to world): world axes, or over a
    /// voxel world a frame whose up follows the local vertical, carried along
    /// by the smallest rotation as the camera moves, so the horizon stays
    /// level and "up" is away from the ground anywhere on a planet.
    cam_frame: glam::Quat,
    /// Local vertical of the voxel world at the camera, from the last frame.
    voxel_up: Option<DVec3>,
    /// A world-space view direction to restore once the frame at a newly set
    /// camera position is known.
    pending_view_direction: Option<Vec3>,
    // Smoothed local-space velocity: x=right, y=up, z=forward (units/sec).
    cam_local_velocity: Vec3,
    viewport_size: (u32, u32),

    // ── TAA reset ──
    pub reset_taa_next_frame: bool,

    // ── Metrics ──
    pub metrics: Arc<Mutex<RenderMetrics>>,
    pub gpu_profiler: GpuProfilerData,
    gpu_profiler_instance: u64,
    last_frame: Instant,
    frame_count: u64,
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
    voxel_backends: VoxelBackendRegistry,
    /// The current sculpt stroke (counted up at each pointer down) and the
    /// last stamp applied, with its stroke: a stroke's stamps are filled in
    /// between, never joined to another stroke's.
    voxel_stroke: u64,
    voxel_stroke_last: Option<(u64, VoxelBrushCommit)>,
    /// Flatten: the level each stroke started at.
    voxel_stroke_level: Option<(u64, f64)>,
    /// Brush samples waiting for the renderer's hit under them, in order.
    voxel_brush_picks: std::collections::VecDeque<PendingBrush>,
    /// Camera height above the voxel ground below it, from the last frame.
    voxel_altitude: Option<f64>,
    /// Coordinate space of the last frame (camera-relative or world).
    last_camera_relative: Option<bool>,
    /// Log why the viewport is not idle (`PULSAR_VOXEL_ACTIVITY`).
    activity_log: bool,
    last_activity_log: Instant,
    last_voxel_errors: Vec<String>,
    /// What the voxel path reads from the scene, and the world revision it
    /// was read at: camera-only frames reuse it instead of re-reading.
    voxel_scene: Option<(u64, VoxelSceneRead)>,
    /// `PULSAR_VOXEL_STATS`: log voxel streaming diagnostics twice a second.
    voxel_stats_log: bool,
    last_voxel_stats_log: Instant,
    native_voxel_flight: super::native_voxel_flight::NativeVoxelFlight,
    /// `PULSAR_VOXEL_NATIVE_SCULPT=1`: scripted sculpt strokes (diagnostic).
    native_sculpt: Option<NativeSculpt>,
}

struct HelioInner {
    renderer: Renderer,
    queue: Arc<wgpu::Queue>,
    interaction: SceneInteraction,
    /// Frame-pacing revision; never used as a renderer-side world mirror.
    last_scene_revision: u64,
    has_rendered_frame: bool,
    /// The post-process baseline last set on this graph's resolver.
    applied_postprocess: Option<crate::scene::EditorPostProcess>,
    /// Watches the scene for spline changes; created with the first sync.
    spline_lines: Option<helio_component::components::SplineLines>,
    /// Drops voxel edits made on ground a terrain no longer has; created
    /// with the first sync.
    voxel_world_sync: Option<crate::scene::voxel_frame::VoxelWorldSync>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectGraphSettings {
    shadow_quality: String,
    shadow_atlas_size: u32,
    screen_space_reflections: bool,
    planar_reflections: bool,
    render_mode: String,
}

impl ProjectGraphSettings {
    fn load() -> Self {
        let setting = |key: &str| {
            engine_state::settings::global_config().get(
                engine_state::settings::NS_PROJECT,
                "rendering",
                key,
            )
        };
        let text = |key: &str, fallback: &str| {
            setting(key)
                .ok()
                .and_then(|value| value.as_str().ok().map(str::to_owned))
                .unwrap_or_else(|| fallback.to_owned())
        };
        let boolean = |key: &str, fallback| {
            setting(key)
                .ok()
                .and_then(|value| value.as_bool().ok())
                .unwrap_or(fallback)
        };
        let shadow_atlas_size = setting("shadow_atlas_size")
            .ok()
            .and_then(|value| {
                value
                    .as_str()
                    .ok()
                    .and_then(|value| value.parse::<u32>().ok())
                    .or_else(|| {
                        value
                            .as_int()
                            .ok()
                            .and_then(|value| u32::try_from(value).ok())
                    })
            })
            .filter(|size| matches!(size, 512 | 1024 | 2048 | 4096))
            .unwrap_or(1024);
        Self {
            shadow_quality: text("shadow_quality", "medium"),
            shadow_atlas_size,
            screen_space_reflections: boolean("screen_space_reflections", false),
            planar_reflections: boolean("planar_reflections", false),
            render_mode: text("render_mode", "deferred"),
        }
    }
}

impl HelioRenderer {
    pub fn new(scene_store: crate::scene::SharedScene) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        let mut voxel_backends = VoxelBackendRegistry::new();
        voxel_backends
            .register(Box::new(PlanetVoxelBackend::new()))
            .expect("built-in voxel renderer ID must be unique");
        // Free-standing voxel objects: mesh rows in the shared scene.
        voxel_backends
            .register(Box::new(MeshVoxelBackend::new(Some(scene_store.clone()))))
            .expect("built-in voxel renderer ID must be unique");
        Self {
            camera_input: Arc::new(Mutex::new(CameraInput::new())),
            scene_store,
            command_sender,
            command_receiver,
            pending_gizmo_mode: Arc::new(Mutex::new(None)),
            pending_camera_state: Arc::new(Mutex::new(None)),
            pending_deselect: Arc::new(AtomicBool::new(false)),
            pending_pointer_events: Arc::new(Mutex::new(Vec::new())),
            // Matches the toolbar's default until the UI reports its state.
            viewport_bloom: Arc::new(AtomicBool::new(true)),
            viewport_realtime: Arc::new(AtomicBool::new(true)),
            frame_delta_override: None,
            camera_jitter_override: None,
            animated_content: Default::default(),
            static_drag_warning: Arc::new(Mutex::new(None)),
            reset_taa_next_frame: false,
            inner: None,
            applied_graph_settings: None,
            pending_errors: Arc::new(Mutex::new(Vec::new())),
            cam_pos: DVec3::new(8.0, 6.0, 12.0),
            relative_camera_gate: RelativeCameraGate::default(),
            cam_yaw: -0.5,
            cam_pitch: -0.3,
            cam_frame: glam::Quat::IDENTITY,
            voxel_up: None,
            pending_view_direction: None,
            cam_local_velocity: Vec3::ZERO,
            viewport_size: (0, 0),
            metrics: Arc::new(Mutex::new(RenderMetrics::default())),
            gpu_profiler: GpuProfilerData::default(),
            gpu_profiler_instance: 0,
            last_frame: Instant::now(),
            frame_count: 0,
            spike_log_config: RenderSpikeLogConfig::default(),
            last_spike_warning: None,
            last_reported_gpu_frame: None,
            had_camera_input: false,
            temporal_settling: TemporalSettling::default(),
            gizmo_dirty: true,
            profiler_frame_counter: 0,
            voxel_backends,
            voxel_stroke: 0,
            voxel_stroke_last: None,
            voxel_stroke_level: None,
            voxel_brush_picks: Default::default(),
            voxel_altitude: None,
            last_camera_relative: None,
            activity_log: std::env::var_os("PULSAR_VOXEL_ACTIVITY").is_some(),
            last_activity_log: Instant::now(),
            last_voxel_errors: Vec::new(),
            voxel_scene: None,
            voxel_stats_log: std::env::var_os("PULSAR_VOXEL_STATS").is_some(),
            last_voxel_stats_log: Instant::now(),
            native_voxel_flight: super::native_voxel_flight::NativeVoxelFlight::new(),
            native_sculpt: std::env::var("PULSAR_VOXEL_NATIVE_SCULPT")
                .ok()
                .filter(|v| v == "1" || v == "far")
                .map(|v| NativeSculpt::new(v == "far")),
        }
    }

    /// Register a voxel renderer before the first viewport frame.
    pub fn register_voxel_backend(
        &mut self,
        backend: Box<dyn VoxelRenderBackend>,
    ) -> Result<(), String> {
        if self.inner.is_some() {
            return Err("voxel renderers must be registered before graph construction".into());
        }
        self.voxel_backends.register(backend)
    }

    /// Camera pose with yaw and pitch of the world-space view direction
    /// (world Y up), independent of the camera's reference frame.
    pub fn editor_camera_state(&self) -> EditorCameraState {
        let (forward, _, _) = self.camera_basis();
        let (yaw, pitch) = yaw_pitch(forward);
        EditorCameraState {
            position: self.cam_pos.to_array(),
            yaw,
            pitch,
        }
    }

    pub fn set_editor_camera_state(&mut self, state: EditorCameraState) {
        self.cam_pos = DVec3::from_array(state.position);
        let forward = direction(state.yaw, state.pitch);
        self.cam_frame = glam::Quat::IDENTITY;
        self.set_view_direction(forward);
        // Over a voxel world the frame at the new position is known next frame.
        self.pending_view_direction = Some(forward);
        self.cam_local_velocity = Vec3::ZERO;

        if let Ok(mut input) = self.camera_input.lock() {
            input.forward = 0.0;
            input.right = 0.0;
            input.up = 0.0;
            input.clear_transient_deltas();
        }
    }

    /// Height of the camera above the voxel ground below it, when a voxel
    /// terrain is shown (what the camera's speed scales with).
    pub fn voxel_altitude(&self) -> Option<f64> {
        self.voxel_altitude
    }

    /// The camera's view direction in world space.
    pub fn camera_forward(&self) -> Vec3 {
        self.camera_basis().0
    }

    /// World-space forward, right and up of the editor camera.
    fn camera_basis(&self) -> (Vec3, Vec3, Vec3) {
        basis(self.cam_frame, self.cam_yaw, self.cam_pitch)
    }

    /// Point the camera along a world-space direction within its frame.
    fn set_view_direction(&mut self, forward: Vec3) {
        (self.cam_yaw, self.cam_pitch) = local_yaw_pitch(self.cam_frame, forward);
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
        profiling::profile_scope!("helio_frame");
        let frame_start = Instant::now();
        let mut phases = FramePhases::new(frame_start);
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.frame_count += 1;
        self.profiler_frame_counter += 1;

        // Level loading runs on the UI thread while rendering can hold the
        // engine mutex. Consume the camera mailbox before camera input and
        // idle detection so the saved pose cannot be silently skipped.
        let pending_camera = self
            .pending_camera_state
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        let external_camera = pending_camera.is_some();
        if let Some(camera) = pending_camera {
            self.set_editor_camera_state(camera);
            self.reset_taa_next_frame = true;
        }

        // Graph-affecting project settings rebuild at a frame boundary. This
        // keeps pass topology and its GPU allocations in sync with the UI.
        let graph_settings = ProjectGraphSettings::load();
        if self.inner.is_some()
            && self
                .applied_graph_settings
                .as_ref()
                .is_some_and(|applied| applied != &graph_settings)
        {
            self.inner = None;
            self.applied_graph_settings = None;
            self.reset_taa_next_frame = true;
        }

        // ── Lazy init (first frame only) ────────────────────────────────────────
        if self.inner.is_none() {
            #[cfg(feature = "editor-ui")]
            gpui::flamegraph_span!("pulsar: HelioRenderer::lazy_init");
            tracing::info!("Initializing Helio renderer...");

            let device_arc = Arc::new(_device.clone());
            let queue_arc = Arc::new(_queue.clone());
            let voxel_quality = {
                let store = self.scene_store.read();
                let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&store.world);
                self.voxel_backends
                    .temporal_quality(&entries, [width, height])
            };
            let project_setting = |key: &str| {
                engine_state::settings::global_config()
                    .get(engine_state::settings::NS_PROJECT, "rendering", key)
                    .ok()
            };
            let project_string = |key: &str, default: &str| {
                project_setting(key)
                    .and_then(|value| value.as_str().ok().map(str::to_owned))
                    .unwrap_or_else(|| default.to_owned())
            };
            let project_float = |key: &str, default: f32| {
                project_setting(key)
                    .and_then(|value| value.as_float().ok())
                    .filter(|value| value.is_finite())
                    .unwrap_or(default as f64) as f32
            };
            let mut config = RendererConfig::new(width, height, format)
                .with_render_scale(project_float("render_scale", 0.75).clamp(0.25, 1.0))
                .with_ssr(graph_settings.screen_space_reflections)
                .with_planar_reflections(graph_settings.planar_reflections);
            let shadow_quality = match graph_settings.shadow_quality.as_str() {
                "low" => helio::ShadowQuality::Low,
                "high" => helio::ShadowQuality::High,
                "ultra" => helio::ShadowQuality::Ultra,
                _ => helio::ShadowQuality::Medium,
            };
            config = config.with_shadow_quality(shadow_quality);
            config.shadow_atlas_size = graph_settings.shadow_atlas_size;
            config = match project_string("tsr_quality", "off").as_str() {
                "performance" => config.with_tsr_quality(helio::TsrQuality::Performance),
                "balanced" => config.with_tsr_quality(helio::TsrQuality::Balanced),
                "quality" => config.with_tsr_quality(helio::TsrQuality::Quality),
                "native" => config.with_tsr_quality(helio::TsrQuality::Native),
                _ => config.without_tsr(),
            };
            config = match graph_settings.render_mode.as_str() {
                "forward_opaque" => config.with_render_mode(helio::RenderMode::ForwardOpaque),
                "forward_only" => config.with_render_mode(helio::RenderMode::ForwardOnly),
                _ => config.with_render_mode(helio::RenderMode::Deferred),
            };
            // Voxel scenes have a workload-specific temporal preset and take
            // precedence over the project's general TSR preference.
            if let Some(quality) = voxel_quality {
                config = config.with_tsr_quality(quality);
            }
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
                .or_else(|| {
                    engine_state::settings::global_config()
                        .get(
                            engine_state::settings::NS_PROJECT,
                            "streaming",
                            "virtual_texture_tile_size",
                        )
                        .ok()
                        .and_then(|value| value.as_str().ok()?.parse::<i64>().ok())
                })
                .and_then(|s| u32::try_from(s).ok())
                .filter(|size| matches!(size, 64 | 128 | 256 | 512))
                .unwrap_or(128);
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
                // Meshes and lights: Helio joins the authored rows on the GPU.
                .with_scene_derivation(crate::scene::scene_join(&device_arc, true))
                .with_scene_derivation(crate::scene::environment_join(&device_arc))
                .with_clear_color([0.15, 0.18, 0.25, 1.0])
                .with_ambient([0.0, 0.0, 0.0], 0.0)
                .with_vt_tile_size(tile_px);
            if vt_enabled {
                builder = builder.with_texture_streaming(pool_mb);
            }
            let voxel_passes = self.voxel_backends.pass_factories();
            let r = builder
                .with_pass_build_context(Box::new(move |ctx| {
                    helio_default_graphs::build_default_graph_external_with_voxel_passes(
                        ctx,
                        voxel_passes,
                    )
                }))
                .build(device_arc.clone(), queue_arc.clone(), width, height, format);

            let inner = HelioInner {
                renderer: r,
                queue: queue_arc.clone(),
                interaction: SceneInteraction::default(),
                last_scene_revision: 0,
                has_rendered_frame: false,
                applied_postprocess: None,
                spline_lines: None,
                voxel_world_sync: None,
            };
            self.inner = Some(inner);
            self.applied_graph_settings = Some(graph_settings.clone());
            self.viewport_size = (width, height);

            tracing::info!(
                "[HELIO] Renderer initialized - camera at {:?}, yaw={}, pitch={}",
                self.cam_pos,
                self.cam_yaw,
                self.cam_pitch
            );
            // First frame must render (lazy init includes nothing visible)
        }

        let previous_viewport_size = self.viewport_size;
        self.viewport_size = (width, height);
        self.configure_gizmo_view();

        if let Some(inner) = &mut self.inner {
            if let Ok(mut pending) = self.pending_gizmo_mode.lock() {
                if let Some(mode) = pending.take() {
                    inner.interaction.set_mode(mode);
                    self.gizmo_dirty = true;
                }
            }
        }

        // ── Pending pointer events (queued by the UI thread, see
        // `PendingPointerEvent`'s doc) ──────────────────────────────────────────
        // Drained unconditionally, before `self.inner` is borrowed below and
        // before the idle/pending-scene checks that follow -- `handle_left_click`/
        // `handle_left_release` already set `self.gizmo_dirty = true`
        // internally, so processing them here needs no extra plumbing to keep
        // this frame from idling out on a drag-release commit.
        if self
            .inner
            .as_ref()
            .is_some_and(|inner| inner.has_rendered_frame)
        {
            if let Some(sculpt) = self.native_sculpt.as_mut() {
                if let Some((event, look_down)) = sculpt.next(now) {
                    if look_down {
                        self.cam_pitch = -0.75;
                    }
                    if let Ok(mut events) = self.pending_pointer_events.lock() {
                        events.push(event);
                    }
                }
            }
        }
        let pending_pointer_events = {
            profiling::profile_scope!("helio_take_pending_pointer_events");
            self.pending_pointer_events
                .lock()
                .map(|mut events| std::mem::take(&mut *events))
                .unwrap_or_default()
        };
        #[cfg(feature = "editor-ui")]
        let _pointer_events_profile = (!pending_pointer_events.is_empty()).then(|| {
            gpui::enter_span(
                gpui::SpanName::Static("pulsar: HelioRenderer::pointer_events"),
                gpui::SpanCategory::UserDefined,
                None,
            )
        });
        if !pending_pointer_events.is_empty() {
            profiling::profile_scope!("helio_pointer_events");
            for event in pending_pointer_events {
                match event {
                    PendingPointerEvent::MouseMove { norm_x, norm_y } => {
                        profiling::profile_scope!("helio_handle_mouse_move");
                        self.handle_mouse_move(norm_x, norm_y);
                    }
                    PendingPointerEvent::LeftClick { norm_x, norm_y } => {
                        profiling::profile_scope!("helio_handle_left_click");
                        self.handle_left_click(norm_x, norm_y);
                    }
                    PendingPointerEvent::LeftRelease => {
                        profiling::profile_scope!("helio_handle_left_release");
                        self.handle_left_release();
                    }
                    PendingPointerEvent::VoxelBrush {
                        norm_x,
                        norm_y,
                        request,
                        start,
                    } => {
                        self.handle_voxel_brush(norm_x, norm_y, request, start);
                    }
                }
            }
        }
        self.resolve_voxel_brushes();
        phases.mark("pointer");

        // ── Detect input activity BEFORE consuming ──────────────────────────────
        let (had_input, needs_resize) = {
            profiling::profile_scope!("helio_read_camera_input");
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
        phases.mark("camera");
        self.configure_gizmo_view();
        self.viewport_size = previous_viewport_size;

        // Before idle detection: a changed baseline renders this frame.
        let postprocess_changed = {
            profiling::profile_scope!("helio_apply_postprocess_baseline");
            self.apply_postprocess_baseline()
        };

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
        let realtime = self.viewport_realtime.load(Ordering::Acquire);
        let (scene_revision, animated) = {
            profiling::profile_scope!("helio_scene_store_read (revision)");
            let store = self.scene_store.read();
            // Re-checked only when the revision changed.
            let animated = realtime && self.animated_content.live(&store.world);
            (store.world.revision(), animated)
        };
        // A newly-created/loaded SceneDB can have revision 0. The first
        // renderer frame still steps the database so its GPU mirror is current
        // before Helio reads it.

        let needs_initial_scene_sync = !inner.has_rendered_frame;
        // Every scene change -- edits, undo/redo, opening a level -- goes
        // through the World's normal write path and advances its revision,
        // so the revision alone wakes the renderer; there is no resync.
        let has_pending_scene =
            needs_initial_scene_sync || scene_revision != inner.last_scene_revision;
        let has_pending_editor = self.pending_deselect.load(Ordering::Acquire)
            || self.pending_gizmo_mode.lock().is_ok_and(|g| g.is_some());
        let camera_stopped = self.cam_local_velocity.length_squared() <= CAMERA_IDLE_EPSILON
            && !self.had_camera_input;
        // The settling budget itself is not activity: it must eventually drain.
        let temporal_activity = !camera_stopped
            || self.native_voxel_flight.force_frames()
            || has_pending_scene
            || has_pending_editor
            || self.voxel_backends.needs_frame(&inner.renderer)
            // Brush samples wait for GPU picks, which only rendered frames
            // answer: idling mid-drag stalled the stroke (no edits until
            // something else rendered).
            || !self.voxel_brush_picks.is_empty()
            || self.gizmo_dirty
            || viewport_resized
            || postprocess_changed
            // Realtime animation: wind, particles, shader-graph `time`.
            || animated
            || self.reset_taa_next_frame;
        if self.activity_log
            && temporal_activity
            && now.duration_since(self.last_activity_log).as_secs_f64() >= 1.0
        {
            // Why the viewport keeps rendering (PULSAR_VOXEL_ACTIVITY=1).
            self.last_activity_log = now;
            tracing::info!(
                camera_moving = !camera_stopped,
                flight = self.native_voxel_flight.force_frames(),
                scene = has_pending_scene,
                scene_revision,
                editor = has_pending_editor,
                voxel = self.voxel_backends.needs_frame(&inner.renderer),
                brush_picks = self.voxel_brush_picks.len(),
                gizmo = self.gizmo_dirty,
                resized = viewport_resized,
                postprocess = postprocess_changed,
                animated,
                reset_taa = self.reset_taa_next_frame,
                "VOXEL_ACTIVITY"
            );
        }
        self.temporal_settling.observe_activity(
            inner
                .renderer
                .find_pass::<helio_pass_tsr::TsrPass>()
                .is_some(),
            temporal_activity,
        );
        let is_idle = !temporal_activity && !self.temporal_settling.needs_frame();

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

        // ── Early out when idle ─────────────────────────────────────────────────
        // No GPU work, no gizmo rebuild, no profiler reads.
        if is_idle {
            // Idle frames must still serve inspector requests.
            {
                profiling::profile_scope!("helio_idle_publish_inspector_snapshot");
                self.scene_store.read().world.publish_inspector_snapshot();
            }
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
            // Exclusive lock: contends with the UI thread (inspector, property
            // edits, hierarchy) for as long as either side holds the scene.
            let mut scene_store = {
                profiling::profile_scope!("helio_scene_store_write_lock_wait");
                self.scene_store.write()
            };
            {
                // Splines are SceneDB components drawn by Helio's editor debug
                // pass in world space, so they follow the camera like the grid.
                // Their lines are rebuilt only when a spline or its owner changed.
                profiling::profile_scope!("helio_sync_spline_lines");
                let spline_lines = inner.spline_lines.get_or_insert_with(|| {
                    helio_component::components::SplineLines::new(&scene_store.world)
                });
                if let Some(lines) = spline_lines.poll(&scene_store.world) {
                    inner.renderer.debug_set_editor_lines("splines", lines);
                }
            }
            {
                // Edits belong to the ground they were made on and skies to
                // their terrain; re-checked only when one of them changed.
                profiling::profile_scope!("helio_sync_voxel_worlds");
                let sync = inner.voxel_world_sync.get_or_insert_with(|| {
                    crate::scene::voxel_frame::VoxelWorldSync::new(&scene_store.world)
                });
                sync.poll(&mut scene_store.world);
            }
            {
                profiling::profile_scope!("helio_scene_store_step");
                scene_store.step();
            }
            sync_ms = t_sync.elapsed().as_secs_f64() * 1000.0;
            inner.last_scene_revision = scene_store.world.revision();
        }
        phases.mark("scene");

        // SceneDB Inspector bridge: throttled inside SceneDB, and a no-op unless
        // an inspector launched this process. After the GPU flush above.
        {
            profiling::profile_scope!("helio_publish_inspector_snapshot");
            self.scene_store.read().world.publish_inspector_snapshot();
        }
        phases.mark("inspector");

        // ── Camera / gizmo / render ─────────────────────────────────────────────
        let VoxelSceneRead {
            entries: voxel_entries,
            errors: mut voxel_errors,
            authored_meshes,
            sun,
            relative_camera_refusals,
        } = {
            let store = self.scene_store.read();
            let revision = store.world.revision();
            match &self.voxel_scene {
                Some((read_at, read)) if *read_at == revision => read.clone(),
                _ => {
                    let read = VoxelSceneRead::of(&store.world);
                    self.voxel_scene = Some((revision, read.clone()));
                    read
                }
            }
        };
        phases.mark("project");
        let wants_relative = self
            .voxel_backends
            .uses_camera_relative_frames(&voxel_entries);
        let camera_relative = wants_relative && {
            let store = self.scene_store.read();
            self.relative_camera_gate
                .compatible(&store.world, relative_camera_refusals.is_empty())
        };
        if self.last_camera_relative != Some(camera_relative) {
            // A switch of coordinate space discards temporal history and
            // changes how the planet and sky are placed: visible as a flash.
            // It must only follow a real scene change. A large world refused
            // camera-relative frames from the start renders in f32 world
            // coordinates far from the origin (jitter, blocks, broken view
            // rays): name what refused it.
            if self.last_camera_relative.is_some() || (wants_relative && !camera_relative) {
                let store = self.scene_store.read();
                tracing::warn!(
                    camera_relative,
                    world = ?relative_camera_refusals,
                    incompatible = ?relative_camera_incompatible_sources(&store.world),
                    "VOXEL_CAMERA_SPACE changed"
                );
            }
            self.last_camera_relative = Some(camera_relative);
        }
        self.voxel_altitude = self.voxel_backends.altitude(&voxel_entries, self.cam_pos);
        phases.mark("altitude");
        if self.native_voxel_flight.force_frames() {
            let flight_ready = inner.has_rendered_frame
                && !self.voxel_backends.needs_frame(&inner.renderer)
                && self.pending_view_direction.is_none()
                && self.voxel_altitude.is_some();
            let flight_interrupted =
                had_input || (external_camera && self.native_voxel_flight.running());
            if let Some(pose) = self.native_voxel_flight.advance(
                now,
                flight_ready,
                flight_interrupted,
                self.cam_pos,
                self.voxel_altitude,
                basis(self.cam_frame, self.cam_yaw, self.cam_pitch).0,
                self.cam_pitch,
                |direction, clearance| {
                    self.voxel_backends
                        .diagnostic_surface_point(&voxel_entries, direction, clearance)
                },
            ) {
                self.cam_pos = self
                    .voxel_backends
                    .lift_out_of_ground(pose.eye)
                    .unwrap_or(pose.eye);
                self.cam_pitch = pose.pitch;
                self.voxel_altitude = self.voxel_backends.altitude(&voxel_entries, self.cam_pos);
            }
        }
        phases.mark("flight");
        self.voxel_up = self.voxel_backends.local_up(&voxel_entries, self.cam_pos);
        phases.mark("up");
        let target = self
            .voxel_up
            .map_or(Vec3::Y, |up| up.as_vec3())
            .normalize_or(Vec3::Y);
        match self.pending_view_direction.take() {
            // A pose set from outside: its view direction within the frame at it.
            Some(forward) => {
                self.cam_frame = glam::Quat::from_rotation_arc(Vec3::Y, target);
                (self.cam_yaw, self.cam_pitch) = local_yaw_pitch(self.cam_frame, forward);
            }
            None => self.cam_frame = transported(self.cam_frame, target),
        }
        let (terrain_near, far) = self
            .voxel_backends
            .camera_clip_range(&voxel_entries, self.cam_pos)
            .unwrap_or((0.1, 10_000.0));
        // A terrain's empty-space certificate says nothing about authored
        // meshes. Preserve their close clipping plane in mixed scenes.
        let near = if authored_meshes {
            terrain_near.min(0.1)
        } else {
            terrain_near
        };
        phases.mark("clip");
        let voxel_tsr_quality = self
            .voxel_backends
            .temporal_quality(&voxel_entries, [width, height]);
        let project_tsr_quality = engine_state::settings::global_config()
            .get(
                engine_state::settings::NS_PROJECT,
                "rendering",
                "tsr_quality",
            )
            .ok()
            .and_then(|value| value.as_str().ok().map(str::to_owned))
            .and_then(|quality| match quality.as_str() {
                "performance" => Some(helio::TsrQuality::Performance),
                "balanced" => Some(helio::TsrQuality::Balanced),
                "quality" => Some(helio::TsrQuality::Quality),
                "native" => Some(helio::TsrQuality::Native),
                _ => None,
            });
        let effective_tsr_quality = voxel_tsr_quality.or(project_tsr_quality);
        inner.renderer.set_tsr_quality(effective_tsr_quality);
        if effective_tsr_quality.is_none() {
            let render_scale = engine_state::settings::global_config()
                .get(
                    engine_state::settings::NS_PROJECT,
                    "rendering",
                    "render_scale",
                )
                .ok()
                .and_then(|value| value.as_float().ok())
                .filter(|value| value.is_finite())
                .unwrap_or(0.75) as f32;
            let render_scale = render_scale.clamp(0.25, 1.0);
            if (inner.renderer.render_scale() - render_scale).abs() > f32::EPSILON {
                inner.renderer.set_render_scale(render_scale);
            }
        }
        let t_prepare = Instant::now();
        let camera = {
            #[cfg(feature = "editor-ui")]
            gpui::flamegraph_span!("pulsar: HelioRenderer::frame_prepare");
            profiling::profile_scope!("helio_frame_prepare");
            let (fwd, _, _) = basis(self.cam_frame, self.cam_yaw, self.cam_pitch);
            let frame_up = self.cam_frame * Vec3::Y;
            let aspect = width as f32 / height.max(1) as f32;
            // Canonical voxel coordinates stay global (`self.cam_pos`,
            // `VoxelView`); only the audited frame camera is origin-relative.
            inner
                .renderer
                .set_world_origin(camera_relative.then_some(self.cam_pos));
            let camera = native_frame_camera(
                self.cam_pos,
                fwd,
                frame_up,
                aspect,
                near,
                far,
                camera_relative,
            );

            // Debug geometry is transient GPU execution state. World content is
            // read by Helio passes directly from the SceneDB GPU mirror.
            {
                profiling::profile_scope!("helio_debug_clear");
                inner.renderer.debug_clear();
            }
            let store = {
                profiling::profile_scope!("helio_scene_store_read_lock_wait (gizmo)");
                self.scene_store.read()
            };
            {
                profiling::profile_scope!("helio_draw_gizmo");
                inner.interaction.draw_gizmo(
                    &mut inner.renderer,
                    &store.world,
                    self.cam_pos.as_vec3(),
                );
            }
            camera
        };

        if self.reset_taa_next_frame {
            if let Some(pass) = inner.renderer.find_pass_mut::<helio_pass_tsr::TsrPass>() {
                pass.reset_history();
            }
            self.reset_taa_next_frame = false;
        }

        let prepare_ms = t_prepare.elapsed().as_secs_f64() * 1000.0;
        if self.voxel_stats_log && self.last_voxel_stats_log.elapsed().as_secs_f32() >= 0.5 {
            self.last_voxel_stats_log = Instant::now();
            let (forward, _, up) = basis(self.cam_frame, self.cam_yaw, self.cam_pitch);
            for line in self.voxel_backends.diagnostics(&inner.renderer) {
                tracing::info!(
                    "VOXEL_STATS altitude={:.1} speed_scale={:.1} {line} eye={:?} forward={:?} up={:?} viewport={}x{} configured_render_scale={:.2} camera_relative={} graph_gpu_ms={:?}",
                    self.voxel_altitude.unwrap_or(f64::NAN),
                    self.voxel_altitude
                        .map_or(1.0, |h| (h / 20.0).clamp(1.0, 1.0e6)),
                    self.cam_pos.to_array(),
                    forward.to_array(),
                    up.to_array(),
                    width,
                    height,
                    inner.renderer.render_scale(),
                    camera_relative,
                    inner.renderer.gpu_frame_ms(),
                );
            }
        }
        let (forward, right, up) = basis(self.cam_frame, self.cam_yaw, self.cam_pitch);
        phases.mark("prepare");
        voxel_errors.extend(
            self.voxel_backends
                .configure_appearance(&mut inner.renderer, &voxel_entries),
        );
        phases.mark("appearance");
        voxel_errors.extend(self.voxel_backends.publish_frame(
            &voxel_entries,
            VoxelView {
                position: self.cam_pos.to_array(),
                right: right.to_array(),
                up: up.to_array(),
                forward: forward.to_array(),
                tan_half_fov_y: (std::f32::consts::FRAC_PI_4 * 0.5).tan(),
                aspect: width as f32 / height.max(1) as f32,
                far,
                size: [width, height],
                sun,
            },
        ));
        phases.mark("publish");
        if voxel_errors != self.last_voxel_errors {
            for error in &voxel_errors {
                tracing::warn!("Voxel terrain: {error}");
            }
            if let Ok(mut pending) = self.pending_errors.lock() {
                pending.extend(voxel_errors.iter().cloned());
            }
            self.last_voxel_errors = voxel_errors;
        }
        let t_render = Instant::now();
        let mut render_succeeded = false;
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
            {
                profiling::profile_scope!("helio_flush_gpu_mirror");
                let store = {
                    profiling::profile_scope!("helio_scene_store_read_lock_wait (flush)");
                    self.scene_store.read()
                };
                store.world.flush_gpu_mirror(&inner.queue);
                crate::scene::end_change_window(&store.world);
            }
            // Separates a wait on the scene lock (the UI thread editing)
            // from the graph's own work in VOXEL_FRAME_PHASES.
            phases.mark("flush");
            {
                profiling::profile_scope!("helio_renderer_render");
                // The editor's animation clock: wall time while realtime,
                // frozen otherwise.
                inner
                    .renderer
                    .set_frame_delta_override(self.frame_delta_override);
                inner
                    .renderer
                    .set_camera_jitter_override(self.camera_jitter_override);
                inner
                    .renderer
                    .set_frame_clock_delta(if realtime { None } else { Some(0.0) });
                match inner.renderer.render(&camera, &view) {
                    Ok(()) => render_succeeded = true,
                    Err(e) => tracing::error!("Helio render error: {:?}", e),
                }
            }
            // The graph's own submission carries the frame. Only when Helio
            // submitted nothing (an error) fall back to an empty submit, which
            // takes the queue lock and can drain pending work.
            inner.renderer.last_submission().or_else(|| {
                profiling::profile_scope!("helio_queue_submit (fence)");
                Some(
                    inner
                        .queue
                        .submit(std::iter::empty::<wgpu::CommandBuffer>()),
                )
            })
        };
        self.gizmo_dirty = false;
        inner.has_rendered_frame = true;
        let render_ms = t_render.elapsed().as_secs_f64() * 1000.0;
        phases.mark("render");
        let frame_ms = frame_start.elapsed().as_secs_f32() * 1_000.0;
        if frame_ms >= 50.0 {
            tracing::warn!(
                "VOXEL_FRAME_PHASES frame_ms={frame_ms:.1} {}",
                phases.describe()
            );
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
        let profiler_id = inner.renderer.profiling_instance_id();
        if profiling::is_profiling_enabled()
            || self.profiler_frame_counter >= 30
            || self.gpu_profiler_instance != profiler_id
        {
            profiling::profile_scope!("helio_gpu_profiler_update");
            self.profiler_frame_counter = 0;
            self.gpu_profiler
                .update_from_snapshot(inner.renderer.timing_snapshot());
            self.gpu_profiler_instance = profiler_id;
        }

        let gpu_frame = self.gpu_profiler.gpu_frame_count;
        let gpu_sample = gpu_frame.map(|frame| (profiler_id, frame));
        let new_gpu_result = gpu_sample.is_some() && gpu_sample != self.last_reported_gpu_frame;
        if new_gpu_result {
            profiling::profile_scope!("helio_emit_gpu_passes");
            emit_helio_gpu_passes(&self.gpu_profiler, profiler_id);
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
            self.last_reported_gpu_frame = gpu_sample;
        }

        {
            profiling::profile_scope!("helio_update_metrics");
            if let Ok(mut m) = self.metrics.lock() {
                m.fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                m.frame_time_ms = dt * 1000.0;
                m.frames_rendered = self.frame_count;
            }
        }

        self.temporal_settling.complete_frame(
            inner
                .renderer
                .find_pass::<helio_pass_tsr::TsrPass>()
                .is_some(),
            temporal_activity,
            render_succeeded && submission_index.is_some(),
        );
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

        // Movement is level within the camera frame: W/S along the horizontal
        // view direction, Q/E along the local vertical.
        let (sy, cy) = self.cam_yaw.sin_cos();
        let fwd = self.cam_frame * Vec3::new(sy, 0.0, -cy);
        let right = self.cam_frame * Vec3::new(cy, 0.0, sy);
        let frame_up = self.cam_frame * Vec3::Y;
        // Over voxel worlds speed grows with height above the ground: the
        // base speed within 20 m of it, 50x at 1 km, orbit in seconds.
        let altitude = self
            .voxel_altitude
            .map_or(1.0, |h| (h / 20.0).clamp(1.0, 1.0e6) as f32);
        let speed = altitude
            * if input.boost {
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

        let before = self.cam_pos;
        let steps = [
            (right * self.cam_local_velocity.x * dt).as_dvec3(),
            (frame_up * self.cam_local_velocity.y * dt).as_dvec3(),
            (fwd * self.cam_local_velocity.z * dt).as_dvec3(),
        ];
        self.cam_pos += steps[0] + steps[1] + steps[2];
        // The editor camera never enters solid voxel terrain. From air (a
        // cave, a dig, above ground) it slides along what it touches: each
        // axis of the move is kept only if it stays in air. Lifting to the
        // surface is only for a camera already buried (spawned or teleported
        // into rock): lifting on contact threw it out of caves.
        if self.voxel_backends.lift_out_of_ground(self.cam_pos).is_some() {
            if self.voxel_backends.lift_out_of_ground(before).is_none() {
                let mut pos = before;
                for step in steps {
                    if self.voxel_backends.lift_out_of_ground(pos + step).is_none() {
                        pos += step;
                    }
                }
                self.cam_pos = pos;
            } else if let Some(lifted) = self.voxel_backends.lift_out_of_ground(self.cam_pos) {
                self.cam_pos = lifted;
            }
        }

        // Middle-mouse (or right-click + Shift) view-plane pan: translate the camera
        // along its screen right/up axes for a 1:1 "grab" feel. Applied directly from
        // the accumulated pixel delta (not velocity-smoothed, not dt-scaled).
        if input.pan_delta_x != 0.0 || input.pan_delta_y != 0.0 {
            const PAN: f32 = 0.01;
            // Screen-up is right x full view forward.
            let (_, _, screen_up) = self.camera_basis();
            let pan_speed = PAN * input.move_speed.max(1.0);
            // Grab convention: dragging right moves content right (camera goes left);
            // dragging down moves content down (camera goes up).
            self.cam_pos += (right * (-input.pan_delta_x) * pan_speed).as_dvec3();
            self.cam_pos += (screen_up * input.pan_delta_y * pan_speed).as_dvec3();
        }
    }

    /// Keep the post-process resolver's baseline in step with the toolbar's
    /// Bloom toggle and the project's graphics settings: a renderer setting
    /// ([`crate::scene::EditorPostProcess`]), so nothing is written into the
    /// scene. Returns whether it changed this frame. Also drains the legacy
    /// feature commands, which carry no state.
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
        let queue = inner.queue.clone();
        let Some(resolver) = inner
            .renderer
            .find_pass_mut::<helio_pass_postprocess::PostProcessVolumeBlendPass>()
        else {
            return false;
        };
        resolver.set_defaults(&queue, &desired.settings());
        inner.applied_postprocess = Some(desired);
        tracing::info!(
            bloom = desired.bloom_enabled,
            intensity = desired.bloom_intensity,
            "Editor viewport post-process updated"
        );
        true
    }

    /// The post-process baseline the editor viewport's resolver holds, once
    /// the renderer is initialized.
    pub fn postprocess_defaults(&mut self) -> Option<helio_pass_postprocess::PostProcessSettings> {
        let inner = self.inner.as_mut()?;
        inner
            .renderer
            .find_pass_mut::<helio_pass_postprocess::PostProcessVolumeBlendPass>()
            .map(|resolver| resolver.defaults().clone())
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

    /// Helio's scene depth buffer (`Depth32Float`, `COPY_SRC`) as of the last
    /// encoded frame; `None` before the first. Read-only diagnostics: lets a
    /// test tell geometry that rasterized but shaded black from geometry
    /// that was never drawn (the SceneDB Phase 0 render baseline).
    pub fn debug_depth_texture(&self) -> Option<&wgpu::Texture> {
        self.inner
            .as_ref()
            .map(|inner| inner.renderer.debug_depth_texture())
    }

    /// The water simulation's height field as of the last encoded frame
    /// (Helio's `WaterSimPass::sim_texture`: layer `volume row * 3 +
    /// cascade`); `None` before the renderer is initialized. Read-only
    /// diagnostics: lets a test read the simulated water back.
    pub fn debug_water_sim_texture(&self) -> Option<&wgpu::Texture> {
        self.inner.as_ref().and_then(|inner| {
            inner
                .renderer
                .find_pass::<helio_pass_water_sim::WaterSimPass>()
                .map(|pass| pass.sim_texture())
        })
    }

    /// The default graph's passes, in execution order, and whether each
    /// recorded CPU work in the last frame (Helio's timing snapshot).
    /// Read-only diagnostics: lets a test show every pass of the graph runs.
    pub fn debug_pass_activity(&self) -> Vec<(String, bool)> {
        let Some(inner) = self.inner.as_ref() else {
            return Vec::new();
        };
        let timed = &inner.renderer.timing_snapshot().passes;
        inner
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

    /// A brush sample under the pointer: it asks the renderer for the
    /// terrain hit there and is applied when the answer arrives (a few
    /// frames later), searched coarse to fine around it.
    fn handle_voxel_brush(&mut self, norm_x: f32, norm_y: f32, request: VoxelBrushRequest, start: bool) {
        profiling::profile_scope!("voxel_brush");
        if start {
            self.voxel_stroke += 1;
        }
        let (width, height) = self.viewport_size;
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let (forward, right, up) = self.camera_basis();
        let tan = (std::f32::consts::FRAC_PI_4 * 0.5).tan();
        let x = norm_x.clamp(0.0, 1.0) * 2.0 - 1.0;
        let y = 1.0 - norm_y.clamp(0.0, 1.0) * 2.0;
        let direction = (forward + right * x * aspect * tan + up * y * tan)
            .normalize_or_zero()
            .as_dvec3();
        if direction == DVec3::ZERO {
            return;
        }
        let brush = PendingBrush {
            pick: 0,
            answer: None,
            requested: Instant::now(),
            stroke: self.voxel_stroke,
            origin: self.cam_pos,
            direction,
            request,
        };
        // Near the last stamp (any stroke's: the walk is exact from the eye,
        // the distance only bounds it), with no older sample still waiting
        // (it must land first): stamp now, walking from the eye to twice that
        // distance (`edit_ray` walks `cell * 3 + 1` either side of the hint,
        // so a third of the distance starts at the eye). A surface beyond it
        // takes the renderer's pick.
        let near = self
            .voxel_stroke_last
            .as_ref()
            .map(|(_, last)| last.distance)
            .filter(|&d| d <= INSTANT_STAMP_M);
        if let Some(distance) = near.filter(|_| self.voxel_brush_picks.is_empty()) {
            if self.apply_voxel_brush(&brush, Some((distance, distance / 3.0))) {
                return;
            }
        }
        match self.voxel_backends.request_pick([norm_x, norm_y]) {
            Some(pick) => self.voxel_brush_picks.push_back(PendingBrush { pick, ..brush }),
            // Nothing drawn to pick yet: a bounded exact walk.
            None => {
                self.apply_voxel_brush(&brush, None);
            }
        }
    }

    /// Apply brush samples in order as their renderer hits arrive. A sample
    /// is never dropped: one whose pick missed (sky, or the column under the
    /// previous stamp still regenerating) or got no answer in time walks the
    /// exact terrain within a bounded reach instead.
    fn resolve_voxel_brushes(&mut self) {
        if self.voxel_brush_picks.is_empty() {
            return;
        }
        profiling::profile_scope!("voxel_resolve_brushes");
        for pick in self.voxel_backends.take_picks() {
            if let Some(brush) = self.voxel_brush_picks.iter_mut().find(|b| b.pick == pick.id) {
                brush.answer = Some(pick.hit);
            }
        }
        while let Some(front) = self.voxel_brush_picks.front() {
            let near = match front.answer {
                Some(hit) => hit,
                None if front.requested.elapsed() >= PICK_TIMEOUT => None,
                None => break,
            };
            let brush = self.voxel_brush_picks.pop_front().expect("front checked above");
            self.apply_voxel_brush(&brush, near);
        }
    }

    /// Apply a brush sample where its ray first hits the terrain; `near`
    /// bounds the exact walk (around a renderer hit). Returns whether a
    /// stamp was applied.
    fn apply_voxel_brush(&mut self, brush: &PendingBrush, near: Option<(f64, f64)>) -> bool {
        profiling::profile_scope!("voxel_apply_brush");
        let started = Instant::now();
        let entries = {
            let scene = self.scene_store.read();
            crate::scene::voxel_frame::project_voxel_entries(&scene.world).0
        };
        // Flatten levels every stamp of a stroke to its first stamp's ground.
        let mut request = brush.request;
        if request.tool == VoxelBrushTool::Flatten {
            request.level = self
                .voxel_stroke_level
                .filter(|(stroke, _)| *stroke == brush.stroke)
                .map(|(_, level)| level);
        }
        let projected = started.elapsed();
        let ray = {
            profiling::profile_scope!("voxel_edit_ray");
            self.voxel_backends.edit_ray(&entries, brush.origin, brush.direction, near, request)
        };
        let walked = started.elapsed();
        let slow = |hit: Option<f64>| {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
            if started.elapsed().as_millis() >= 20 {
                tracing::warn!(
                    "VOXEL_BRUSH slow: {:.1} ms (entries {:.1}, ray {:.1}, commit {:.1}) near={near:?} hit={hit:?}",
                    ms(started.elapsed()),
                    ms(projected),
                    ms(walked - projected),
                    ms(started.elapsed() - walked)
                );
            }
        };
        match ray {
            Ok(Some(commit)) => {
                // Fill the gap from the stroke's previous stamp, so fast drags
                // stay continuous at any frame rate.
                let voxel = entries
                    .iter()
                    .find(|e| e.id == commit.id)
                    .map_or(0.1, |e| e.voxel_size);
                if request.tool == VoxelBrushTool::Flatten && request.level.is_none() {
                    self.voxel_stroke_level = Some((brush.stroke, commit.level));
                }
                // Stamps fill the gap from the stroke's previous one; flatten
                // and smooth stamps overlap by their footprint.
                let fill = self
                    .voxel_stroke_last
                    .as_ref()
                    .filter(|(stroke, last)| *stroke == brush.stroke && last.id == commit.id && request.tool == VoxelBrushTool::Stamp)
                    .map(|(_, last)| super::voxel_backend::stroke_fill(&last.edit, &commit.edit, voxel))
                    .unwrap_or_default();
                let mut scene = self.scene_store.write();
                for edit in fill {
                    let stamp = VoxelBrushCommit {
                        edit,
                        ..commit.clone()
                    };
                    self.gizmo_dirty |= apply_voxel_brush_commit(&mut scene.world, stamp);
                }
                self.gizmo_dirty |= apply_voxel_brush_commit(&mut scene.world, commit.clone());
                drop(scene);
                slow(Some(commit.distance));
                self.voxel_stroke_last = Some((brush.stroke, commit));
                true
            }
            Ok(_) => {
                slow(None);
                false
            }
            Err(error) => {
                tracing::warn!("Voxel brush: {error}");
                if let Ok(mut pending) = self.pending_errors.lock() {
                    pending.push(format!("Voxel brush: {error}"));
                }
                false
            }
        }
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
            static_drag_warning: self.static_drag_warning.clone(),
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

    /// Build a world-space ray from normalized viewport coordinates using only
    /// the renderer camera pose and local projection math.
    fn build_pick_ray(&self, norm_x: f32, norm_y: f32) -> (Vec3, Vec3) {
        let (width, height) = self.viewport_size;
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        let x = norm_x * 2.0 - 1.0;
        let y = 1.0 - norm_y * 2.0;
        let (forward, _, up) = self.camera_basis();
        let projection =
            Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, width / height, 0.1, 10_000.0);
        let camera_position = self.cam_pos.as_vec3();
        let view = Mat4::look_at_rh(camera_position, camera_position + forward, up);
        let inverse = (projection * view).inverse();
        let near = inverse.project_point3(Vec3::new(x, y, 0.0));
        let far = inverse.project_point3(Vec3::new(x, y, 1.0));
        (near, (far - near).normalize_or_zero())
    }

    pub fn handle_left_click(&mut self, norm_x: f32, norm_y: f32) {
        self.configure_gizmo_view();
        self.gizmo_dirty = true;
        let (ray_origin, ray_direction) = self.build_pick_ray(norm_x, norm_y);
        let Some(inner) = &mut self.inner else { return };
        let store = self.scene_store.read();
        if inner.interaction.try_start_drag(
            &store.world,
            ray_origin,
            ray_direction,
            self.cam_pos.as_vec3(),
        ) {
            // What the object's mesh and light instances author: the flag
            // the caches the drag would invalidate key on.
            let fixed = store.world.selected_entity().and_then(|entity| {
                let movability =
                    helio_component::components::object_movability(&store.world, entity)?;
                (!movability.can_move()).then(|| StaticDragWarning {
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
            inner.interaction.update_drag(
                &mut store.world,
                ray_origin,
                ray_direction,
                self.cam_pos.as_vec3(),
            );
            self.gizmo_dirty = true;
        } else {
            let store = self.scene_store.read();
            self.gizmo_dirty |= inner.interaction.update_hover(
                &store.world,
                ray_origin,
                ray_direction,
                self.cam_pos.as_vec3(),
            );
        }
    }

    pub fn handle_left_release(&mut self) {
        self.gizmo_dirty = true;
        if let Some(inner) = &mut self.inner {
            inner.interaction.cancel_drag();
        }
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
        let projection = Mat4::perspective_rh(
            std::f32::consts::FRAC_PI_4,
            w.max(1) as f32 / h.max(1) as f32,
            0.1,
            10_000.0,
        );
        let camera_position = self.cam_pos.as_vec3();
        let view = Mat4::look_at_rh(camera_position, camera_position + forward, up);
        if let Some(inner) = &mut self.inner {
            inner
                .interaction
                .set_view(camera_position, forward, projection * view, size, 10_000.0);
        }
    }
}

/// View direction for yaw (about +Y, 0 = -Z) and pitch.
fn direction(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(sy * cp, sp, -cy * cp)
}

/// Inverse of [`direction`].
fn yaw_pitch(forward: Vec3) -> (f32, f32) {
    let f = forward.normalize_or(Vec3::NEG_Z);
    (f.x.atan2(-f.z), f.y.clamp(-1.0, 1.0).asin())
}

/// Yaw and (clamped) pitch of a world-space direction within `frame`.
fn local_yaw_pitch(frame: glam::Quat, forward: Vec3) -> (f32, f32) {
    let (yaw, pitch) = yaw_pitch(frame.inverse() * forward);
    (yaw, pitch.clamp(-1.5, 1.5))
}

/// World-space forward, right and up for yaw and pitch within `frame`.
fn basis(frame: glam::Quat, yaw: f32, pitch: f32) -> (Vec3, Vec3, Vec3) {
    let forward = frame * direction(yaw, pitch);
    let (sy, cy) = yaw.sin_cos();
    let right = frame * Vec3::new(cy, 0.0, sy);
    let up = right.cross(forward).normalize_or_zero();
    (forward, right, up)
}

/// `frame` turned by the smallest rotation that takes its up to `up`.
fn transported(frame: glam::Quat, up: Vec3) -> glam::Quat {
    (glam::Quat::from_rotation_arc(frame * Vec3::Y, up) * frame).normalize()
}

#[cfg(test)]
mod camera_frame_tests {
    use super::*;

    #[test]
    fn world_frame_matches_the_legacy_yaw_pitch_convention() {
        let (forward, right, up) = basis(glam::Quat::IDENTITY, 0.0, 0.0);
        assert!(
            forward.abs_diff_eq(Vec3::NEG_Z, 1e-6)
                && right.abs_diff_eq(Vec3::X, 1e-6)
                && up.abs_diff_eq(Vec3::Y, 1e-6)
        );
        let d = direction(0.7, -0.3);
        let (yaw, pitch) = yaw_pitch(d);
        assert!((yaw - 0.7).abs() < 1e-5 && (pitch + 0.3).abs() < 1e-5);
    }

    #[test]
    fn the_frame_follows_the_local_vertical_with_a_level_horizon() {
        // 30 degrees from the pole of a planet centred at the origin.
        let local_up = Vec3::new(0.5, 0.866_025_4, 0.0);
        let frame = transported(glam::Quat::IDENTITY, local_up);
        assert!((frame * Vec3::Y).abs_diff_eq(local_up, 1e-5));
        for yaw in [0.0, 1.0, 2.5, -2.0] {
            let (forward, right, up) = basis(frame, yaw, 0.0);
            // Level: forward and right horizontal, up is the local vertical.
            assert!(
                forward.dot(local_up).abs() < 1e-5 && right.dot(local_up).abs() < 1e-5,
                "yaw {yaw}"
            );
            assert!(up.abs_diff_eq(local_up, 1e-5), "yaw {yaw}");
        }
        // A world-space direction survives the round trip through the frame.
        let wanted = Vec3::new(0.3, -0.4, -0.8).normalize();
        let (yaw, pitch) = local_yaw_pitch(frame, wanted);
        assert!(basis(frame, yaw, pitch).0.abs_diff_eq(wanted, 1e-5));
    }

    #[test]
    fn carrying_the_frame_over_the_planet_has_no_jumps() {
        let mut frame = glam::Quat::IDENTITY;
        let mut previous = basis(frame, 0.4, -0.2).0;
        for step in 1..=450 {
            let angle = (step as f32 * 0.1).to_radians();
            let up = Vec3::new(angle.sin(), angle.cos(), 0.0);
            frame = transported(frame, up);
            let forward = basis(frame, 0.4, -0.2).0;
            // The view turns only as much as the vertical does.
            assert!(
                forward.angle_between(previous) <= 0.1f32.to_radians() * 1.01,
                "step {step}"
            );
            previous = forward;
        }
    }
}

/// What the voxel path reads from the scene: the voxel entries, whether any
/// enabled static mesh exists, the sun direction, and what keeps the scene
/// out of camera-relative frames. Read once per world revision.
#[derive(Clone)]
struct VoxelSceneRead {
    entries: Vec<crate::scene::voxel_frame::VoxelSceneEntry>,
    errors: Vec<String>,
    authored_meshes: bool,
    sun: Option<[f32; 3]>,
    /// Empty when the scene may render camera-relative
    /// (`relative_camera_world_incompatibilities`).
    relative_camera_refusals: Vec<String>,
}

impl VoxelSceneRead {
    fn of(world: &pulsar_scenedb::World) -> Self {
        use helio_component::components::{LightComponent, LightType};
        use pulsar_scene_model::attachments::ComponentOwner;
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(world);
        let authored_meshes = world
            .query::<(
                &helio_component::components::StaticMeshComponent,
                &ComponentOwner,
            )>()
            .any(|(_, (_, owner))| owner.is_enabled());
        // Voxel terrain traces sunlight towards the scene's directional
        // light: the opposite of the direction it travels, its owner's
        // rotation of -Y. The light must be lit the way the scene join
        // lights it: enabled, and its owner visible. Any other lit light is
        // positional, which camera-relative frames do not rebase.
        let mut sun_owner = None;
        let mut positional_light = false;
        for (_, (light, owner)) in world.query::<(&LightComponent, &ComponentOwner)>() {
            if !owner.is_enabled() || !light.general.enabled {
                continue;
            }
            if light.general.light_type != LightType::Directional {
                positional_light = true;
            } else if sun_owner.is_none()
                && world
                    .get::<pulsar_scene_model::Visibility>(owner.entity())
                    .is_none_or(|visibility| visibility.visible)
            {
                sun_owner = Some(owner.entity());
            }
        }
        let sun = sun_owner
            .and_then(|owner| world.get::<crate::scene::Transform>(owner).copied())
            .map(|transform| {
                let rotation = glam::Quat::from_euler(
                    glam::EulerRot::YXZ,
                    transform.rotation[1].to_radians(),
                    transform.rotation[0].to_radians(),
                    transform.rotation[2].to_radians(),
                );
                (rotation * Vec3::Y).to_array()
            });
        Self {
            entries,
            errors,
            authored_meshes,
            sun,
            relative_camera_refusals: relative_camera_world_incompatibilities(
                world,
                positional_light,
            ),
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
mod native_relative_camera_tests {
    use super::*;
    use crate::scene::{SceneWorldExt, SpawnObject};
    use helio_component::components::{LightComponent, LightType};

    /// The example planet's objects: a terrain with its layers and air, a
    /// post-process volume and a directional sun, attached as instances.
    /// Returns the scene and the sun's light instance.
    fn planet_scene() -> (pulsar_scenedb::SceneDb, pulsar_scenedb::Entity) {
        let mut scene = pulsar_scenedb::SceneDb::new();
        let world = &mut scene.world;
        let planet = world.spawn_object(SpawnObject::new("voxel_planet")).unwrap();
        pulsar_world_registry::attach_value(world, planet, helio_component::VoxelTerrainComponent::default())
            .unwrap();
        pulsar_world_registry::attach_value(world, planet, helio_component::VoxelTerrainLayersComponent::default())
            .unwrap();
        pulsar_world_registry::attach_value(world, planet, helio_component::AtmosphereComponent::default())
            .unwrap();
        let volume = world.spawn_object(SpawnObject::new("post_process")).unwrap();
        pulsar_world_registry::attach_value(world, volume, helio_component::PostProcessVolumeComponent::default())
            .unwrap();
        let sun_object = world.spawn_object(SpawnObject::new("sun")).unwrap();
        let mut sun = LightComponent::default();
        sun.general.enabled = true;
        sun.general.light_type = LightType::Directional;
        let sun = pulsar_world_registry::attach_value(world, sun_object, sun).unwrap();
        (scene, sun)
    }

    #[test]
    fn the_example_planet_scene_renders_camera_relative() {
        let (mut scene, sun) = planet_scene();
        let read = VoxelSceneRead::of(&scene.world);
        assert!(read.relative_camera_refusals.is_empty(), "{:?}", read.relative_camera_refusals);
        assert!(read.sun.is_some(), "the directional light is the sun");
        assert_eq!(read.entries.len(), 1);
        // A new, unreviewed world-space consumer fails closed; retiring its
        // last live row restores eligibility.
        struct UnknownWorldSpaceProvider;
        let entity = scene.world.spawn();
        scene.world.insert(entity, UnknownWorldSpaceProvider);
        assert!(!VoxelSceneRead::of(&scene.world).relative_camera_refusals.is_empty());
        scene.world.despawn(entity);
        assert!(VoxelSceneRead::of(&scene.world).relative_camera_refusals.is_empty());
        // A positional light is not rebased.
        scene.world.get_mut::<LightComponent>(sun).unwrap().general.light_type = LightType::Point;
        let read = VoxelSceneRead::of(&scene.world);
        assert_eq!(read.relative_camera_refusals, ["positional light"]);
        assert!(read.sun.is_none());
    }

    /// Every buffer this engine registers in the GPU mirror is reviewed:
    /// a frame of the example planet is not refused by its own sources.
    #[test]
    fn the_engines_gpu_sources_are_camera_relative_compatible() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some(adapter) = pollster::block_on(instance.request_adapter(&Default::default())).ok() else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: helio::required_wgpu_limits(adapter.limits()),
            ..Default::default()
        })) else {
            eprintln!("skipping: no GPU device available");
            return;
        };
        let (mut scene, _) = planet_scene();
        crate::scene::ensure_gpu_mirror(&mut scene, Arc::new(device), Arc::new(queue));
        scene.step();
        assert_eq!(relative_camera_incompatible_sources(&scene.world), Vec::<String>::new());
        let read = VoxelSceneRead::of(&scene.world);
        let mut gate = RelativeCameraGate::default();
        assert!(gate.compatible(&scene.world, read.relative_camera_refusals.is_empty()));
    }

    #[test]
    fn unknown_or_raw_render_sources_do_not_activate_relative_camera() {
        use pulsar_scenedb::{gpu::BufferAccess::ReadOnly, MirrorMode};
        assert!(relative_camera_source_compatible("component_owners", "row", Some(MirrorMode::DirtyTracked), ReadOnly));
        assert!(!relative_camera_source_compatible("component_owners", "resource", None, ReadOnly));
        for key in ["reflection_captures", "portal_views", "corona_emitters", "foliage_layers", "custom_render_source"] {
            assert!(!relative_camera_source_compatible(key, "row", Some(MirrorMode::DirtyTracked), ReadOnly), "{key}");
        }
    }

    #[test]
    fn earth_scale_terrain_depth_reprojects_in_local_camera_space() {
        let eye = DVec3::new(-3426954.099417845, 4769291.620032467, -2476450.5275892294);
        let forward = Vec3::new(0.9167773, 0.29777223, -0.2661786);
        let up = Vec3::new(-0.39032218, 0.8092439, -0.43905908);
        let local = native_frame_camera(eye, forward, up, 1196.0 / 729.0, 2.4, 46_371_000.0, true);
        let global = native_frame_camera(eye, forward, up, 1196.0 / 729.0, 2.4, 46_371_000.0, false);
        assert_eq!(local.position, Vec3::ZERO);
        assert_eq!(global.position, eye.as_vec3());
        let local_vp = local.proj * local.view;
        let global_vp = global.proj * global.view;
        let point = (forward.normalize() * 10.0).extend(1.0);
        let clip = local_vp * point;
        let raster = clip / clip.w;
        let reconstruct = |vp: Mat4| {
            let h = vp.inverse() * raster;
            (h.truncate() / h.w).extend(1.0)
        };
        let local_reproject = local_vp * reconstruct(local_vp);
        let global_reproject = global_vp * reconstruct(global_vp);
        let uv_error = |q: glam::Vec4| ((q.truncate() / q.w) - raster.truncate()).truncate().length();
        assert!(uv_error(global_reproject) > 0.1, "regression trigger must expose global-f32 cancellation");
        assert!(uv_error(local_reproject) < 0.000001);
        // Express the current relative point in the previous frame's origin
        // exactly once; sub-voxel camera movement must survive Earth-scale
        // positions.
        let previous_eye = eye - DVec3::new(0.01, -0.03, 0.02);
        let shift = eye - previous_eye;
        let previous_view = (local.view.as_dmat4() * glam::DMat4::from_translation(shift)).as_mat4();
        let expected = (local.view.as_dmat4() * (point.as_dvec4() + shift.extend(0.0))).as_vec4();
        assert!((previous_view * point - expected).length() < 0.00001);
    }
}
