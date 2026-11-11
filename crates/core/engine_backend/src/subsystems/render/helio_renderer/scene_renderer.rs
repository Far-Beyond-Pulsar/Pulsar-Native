//! A scene's renderer: Helio's renderer over a shared SceneDB scene, with
//! what the scene's content publishes and the frame's coordinate space. The one
//! construction the editor viewport (`HelioRenderer`, which adds gizmos,
//! picking, brushes and debug views) and the game (the standalone window and
//! Play-in-Editor) render with, so a played level looks as the editor shows
//! it.
//!
//! Per frame: [`SceneRenderer::step_scene`] advances the scene (and the
//! subsystems plugins add to it), [`SceneRenderer::prepare_frame`] streams
//! virtual geometry, places the camera (at its f64 eye; Helio's frames are
//! camera-relative, their origin following it), clip planes the scene's
//! ground certifies, the live TSR preset) and hands the renderer the CPU
//! payloads the scene's content publishes (`pulsar_world_registry::
//! scene_payloads`), [`SceneRenderer::render`] uploads the scene and draws.

use std::sync::Arc;

use glam::{DVec3, Vec3};
use helio::{Camera, Renderer};

use super::camera_ground::{CameraGround, GroundQueries};
use super::host_parts::RenderHostPart;
use crate::scene::{SharedScene, SpaceSync};

/// A change applied to a renderer's host parts when they are made (and to
/// the existing ones): what a test or tool sets on a plugin's part, by key
/// (a later setup with the same key replaces it).
pub type HostPartSetup = (
    &'static str,
    Arc<dyn Fn(&mut dyn RenderHostPart) + Send + Sync>,
);

/// Who owns the renderer's device.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceOwner {
    /// The renderer (the standalone game's window device).
    Renderer,
    /// A host sharing it (the editor's device, under the viewport and
    /// Play-in-Editor).
    Host,
}

/// What Helio's status screen says about assigning a pipeline.
const PIPELINE_HINT: &str = "Pick a render pipeline asset in the level's World Settings.";

/// How a [`SceneRenderer`] is built.
#[derive(Clone)]
pub struct SceneRendererOptions {
    /// Draw the editor's overlays (the editor viewport, a game launched with
    /// them).
    pub editor_mode: bool,
    pub device: DeviceOwner,
    /// The render pipeline asked for (a registered id, see
    /// [`super::pipelines`]). `None`, an id nobody registered or one that does
    /// not compile builds a renderer with no pipeline
    /// ([`super::pipelines::PipelineState`] says why): nothing is chosen for it.
    pub pipeline: Option<String>,
    /// Applied to the plugins' render-host parts when they are made
    /// (tests, tools: a plugin's settings in place of the project's).
    pub host_part_setups: Vec<HostPartSetup>,
}

impl SceneRendererOptions {
    /// A game's renderer: no overlays unless `editor_mode`, nobody saving.
    pub fn game(device: DeviceOwner, editor_mode: bool) -> Self {
        Self {
            editor_mode,
            device,
            // The level's pipeline (World Settings).
            pipeline: super::pipelines::level_pipeline(),
            host_part_setups: Vec::new(),
        }
    }
}

/// The camera of a frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameView {
    /// The eye in world space, in f64: a planet's surface is millions of
    /// metres from the origin.
    pub eye: DVec3,
    pub forward: Vec3,
    pub up: Vec3,
    /// Width over height.
    pub aspect: f32,
    pub projection: FrameProjection,
    /// Optional crop into the full output. `aspect` describes `full_size`,
    /// while the actual render target can be as small as one GPU texture.
    pub crop: Option<FrameCrop>,
}

/// One tile of a full-resolution image being rendered by `SceneRenderer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameCrop {
    pub full_size: [u32; 2],
    pub origin: [u32; 2],
    pub tile_size: [u32; 2],
}

#[derive(Clone, Copy, Debug)]
pub enum FrameProjection {
    /// `fov_y` radians vertically. `near` and `far` are the camera's own clip
    /// planes; over ground that certifies planes (a voxel world) those
    /// replace them.
    Perspective { fov_y: f32, near: f32, far: f32 },
    /// Looking at the eye from both sides, `half_height` up and down
    /// ([`ortho_camera`]).
    Orthographic { half_height: f32 },
}

/// A frame placed by [`SceneRenderer::prepare_frame`].
pub struct PreparedFrame {
    pub camera: Camera,
    /// The problems the scene's content reports (a voxel world that does
    /// not build), when they changed this frame (an empty list: they were
    /// fixed). Already logged.
    pub new_errors: Option<Vec<String>>,
}

pub struct SceneRenderer {
    pub renderer: Renderer,
    project_settings: super::project_settings::ProjectGraphSettingsCache,
    scene: SharedScene,
    queue: Arc<wgpu::Queue>,
    /// The rigid-frame table last handed to the renderer (slot 0 is the
    /// world), so it is re-uploaded only when a frame moved.
    spaces: SpaceSync,
    /// What the frame reads from the scene, and the world revision it was
    /// read at: camera-only frames reuse it.
    read: Option<(u64, Arc<SceneRead>)>,
    last_errors: Vec<String>,
    /// The ground under cameras, as the scene's content reports it.
    ground: GroundQueries,
    /// The plugins' per-renderer state (`RenderHostPartRegistration`).
    host_parts: Vec<Box<dyn RenderHostPart>>,
    /// What the graph was built from: a registered pipeline, or why none runs.
    pipeline: super::pipelines::PipelineState,
    /// What the host publishes for this view's passes besides the scene's
    /// payloads ([`Self::set_view_payload`]).
    view_payloads: Vec<(&'static str, super::ViewPayload)>,
}

impl SceneRenderer {
    /// A renderer over `scene` with the project's settings. SceneDB's GPU
    /// mirror is attached first (idempotent: renderers on one scene share
    /// it, and on a new device it moves there); Helio's derivations make the
    /// drawn rows from it on the GPU.
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        scene: &SharedScene,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        options: SceneRendererOptions,
    ) -> Self {
        // Every renderer setting comes from the project, through Helio's
        // description of them; under "Automatic" TSR the scene content's
        // preset.
        let settings = super::project_settings::renderer();
        let scene_quality = {
            let store = scene.read();
            temporal_quality(
                pulsar_world_registry::scene_payloads(&store.world).temporal,
                [width, height],
            )
        };
        let mut host_parts = super::host_parts::new_parts();
        for part in &mut host_parts {
            for (_, setup) in &options.host_part_setups {
                setup(part.as_mut());
            }
        }
        let mut config = host_parts.iter_mut().fold(
            super::project_settings::renderer_config(width, height, format, &settings),
            |config, part| part.configure(config),
        );
        if let Some(quality) = super::project_settings::tsr_quality(&settings, scene_quality) {
            config = config.with_tsr_quality(quality);
        }
        // The plugins' features' settings: their features join whichever
        // pipeline the level runs.
        let config = super::render_features::configure(config);

        let scene_db =
            crate::scene::ensure_gpu_mirror(&mut scene.write(), device.clone(), queue.clone());
        let spaces = SpaceSync::default();
        let mut builder = helio::RendererBuilder::new(config, scene_db.clone())
            .with_editor_mode(options.editor_mode)
            // Parent-relative transforms become world-space first, under the
            // same key, so the joins below read them unchanged.
            .with_scene_derivation(crate::scene::hierarchy_resolve(&device))
            // Every plugin's object join (the meshes', the lights'): the
            // authored rows joined on the GPU.
            .with_scene_derivation(crate::scene::scene_join(&device, options.editor_mode));
        // The plugins' host parts' derivations (the meshes' virtual
        // geometry and authored LOD), between the object and row joins.
        for part in &mut host_parts {
            for derivation in part.derivations(&device, &scene_db) {
                builder = builder.with_scene_derivation(derivation);
            }
        }
        let mut builder = builder.with_scene_derivation(crate::scene::environment_join(&device));
        // What the plugins' space producers project each frame (`spaces`).
        for derivation in spaces.derivations(&device) {
            builder = builder.with_scene_derivation(derivation);
        }
        let mut builder = builder
            // No default ambient: all illumination comes from the scene.
            .with_feature(helio::AmbientLight {
                color: [0.0; 3],
                intensity: 0.0,
            });
        if options.device == DeviceOwner::Host {
            builder = builder.with_external_device();
        }
        // The renderer is built with no pipeline; the level's is assigned below.
        // There is no default: none, a missing or an invalid one shows Helio's
        // status screen with the reason.
        let renderer = builder
            .with_status_hint(PIPELINE_HINT)
            .build(device, queue.clone(), width, height, format);
        let mut this = Self {
            renderer,
            project_settings: super::project_settings::ProjectGraphSettingsCache::new(),
            scene: scene.clone(),
            queue,
            spaces,
            read: None,
            last_errors: Vec::new(),
            ground: GroundQueries::default(),
            host_parts,
            pipeline: super::pipelines::PipelineState::Unassigned,
            view_payloads: Vec::new(),
        };
        this.set_pipeline_state(super::pipelines::pipeline_state(options.pipeline.as_deref()));
        this
    }

    /// Runs the pipeline `state` names, at once (the caller is at a frame
    /// boundary): the old graph and its GPU resources are dropped and the new
    /// one built, or Helio's status screen shows why none runs. Nothing is
    /// substituted for a pipeline that is missing or does not build.
    pub fn set_pipeline_state(&mut self, state: super::pipelines::PipelineState) {
        use super::pipelines;
        match &state {
            pipelines::PipelineState::Assigned { id, .. } => {
                match pipelines::create_pipeline(Some(id)) {
                    Some(graph) => self
                        .renderer
                        .set_pipeline(Some(helio::NamedPipeline::boxed(id.clone(), graph))),
                    None => self.renderer.fail_pipeline(
                        id.clone(),
                        helio::PipelineFailure::new(format!(
                            "Render pipeline `{id}` is no longer registered"
                        )),
                    ),
                }
            }
            pipelines::PipelineState::Unassigned => self.renderer.set_pipeline(None),
            missing => {
                if let Some((id, failure)) = missing.failure() {
                    self.renderer.fail_pipeline(id, failure);
                }
            }
        }
        self.renderer.apply_pending_pipeline();
        match (state.reason(), self.renderer.pipeline_status()) {
            (Some(reason), _) => tracing::error!("{reason}; the renderer runs no pipeline"),
            (None, helio::PipelineStatus::Invalid { id, reason, .. }) => {
                tracing::error!("Render pipeline `{id}` does not build: {reason}")
            }
            (None, _) => {
                if let Some(id) = state.id() {
                    pipelines::log_routing(id, &self.renderer.routing_issues());
                }
            }
        }
        self.pipeline = state;
    }

    /// Publish `payload` for this view's passes under a feature's payload
    /// contract key, every frame until replaced (`None` withdraws it):
    /// editor-only data a plugin's overlay draws, beside what the scene
    /// publishes.
    pub fn set_view_payload(&mut self, key: &'static str, payload: Option<super::ViewPayload>) {
        self.view_payloads.retain(|(existing, _)| *existing != key);
        if let Some(payload) = payload {
            self.view_payloads.push((key, payload));
        }
    }

    /// What the renderer was told to run; [`PipelineState::reason`] says why
    /// none does. Whether the pipeline built is `renderer.pipeline_status()`.
    ///
    /// [`PipelineState::reason`]: super::pipelines::PipelineState::reason
    pub fn pipeline(&self) -> &super::pipelines::PipelineState {
        &self.pipeline
    }

    /// This renderer's host part of type `T` (a plugin's), if one was made.
    pub fn host_part<T: RenderHostPart>(&self) -> Option<&T> {
        self.host_parts
            .iter()
            .find_map(|part| part.as_any().downcast_ref::<T>())
    }

    /// This renderer's host part of type `T` (a plugin's), if one was made.
    pub fn host_part_mut<T: RenderHostPart>(&mut self) -> Option<&mut T> {
        self.host_parts
            .iter_mut()
            .find_map(|part| part.as_any_mut().downcast_mut::<T>())
    }

    /// Apply `setup` to every host part now.
    pub fn set_up_host_parts(&mut self, setup: &(dyn Fn(&mut dyn RenderHostPart) + Send + Sync)) {
        for part in &mut self.host_parts {
            setup(part.as_mut());
        }
    }

    /// Whether the renderer or the scene's content needs more frames to
    /// finish what it shows (content streaming in or building, a request
    /// the renderer answers a few frames later).
    pub fn needs_frame(&self) -> bool {
        self.content_settling() || self.renderer.needs_frame()
    }

    /// Whether the scene's content said it is still settling (a world
    /// building, requests waiting on the renderer): its scene subsystems
    /// need steps to take what finished.
    pub fn content_settling(&self) -> bool {
        pulsar_world_registry::scene_settling(&self.scene.read().world)
    }

    /// The renderer's diagnostics lines (`PULSAR_VOXEL_STATS`).
    pub fn diagnostics(&self) -> Vec<String> {
        self.renderer.diagnostics()
    }

    /// Advance `scene` (this renderer's) a frame: SceneDB steps the
    /// subsystems plugins added to every scene, and flushes its GPU mirror.
    pub fn step_scene(&mut self, scene: &mut pulsar_scenedb::SceneDb) {
        profiling::profile_scope!("helio_scene_store_step");
        pulsar_world_registry::install_scene_subsystems(scene);
        scene.step();
    }

    /// What the frame reads from the scene, read once per world revision.
    fn scene_read(&mut self) -> Arc<SceneRead> {
        let store = self.scene.read();
        let revision = store.world.revision();
        match &self.read {
            Some((read_at, read)) if *read_at == revision => read.clone(),
            _ => {
                let read = Arc::new(SceneRead::of(&store.world, &self.host_parts));
                self.read = Some((revision, read.clone()));
                read
            }
        }
    }

    /// The ground under `eye`, as the scene's content reports it
    /// ([`super::camera_ground`]): a camera's local vertical and altitude.
    pub fn camera_ground(&mut self, eye: DVec3) -> CameraGround {
        let mut scene = self.scene.write();
        self.ground.ground(&mut scene.world, eye)
    }

    /// Where a camera at `eye` should be instead when `eye` is inside solid
    /// ground: just above it.
    pub fn lift_out_of_ground(&mut self, eye: DVec3) -> Option<DVec3> {
        let mut scene = self.scene.write();
        self.ground.lift_out_of_ground(&mut scene.world, eye)
    }

    /// Place this frame's camera and hand the renderer what the scene's
    /// content publishes. `size` is the frame's in pixels.
    pub fn prepare_frame(&mut self, view: &FrameView, size: [u32; 2]) -> PreparedFrame {
        profiling::profile_scope!("helio_frame_prepare");
        // The plugins' per-renderer state (the virtual-geometry assets the
        // scene asks for, built and streamed).
        for part in &mut self.host_parts {
            part.prepare_frame(&self.scene);
        }
        let read = self.scene_read();
        // The CPU payloads the scene's content publishes for the features
        // (a voxel world's volumes), by contract key.
        let published = {
            let store = self.scene.read();
            pulsar_world_registry::scene_payloads(&store.world)
        };
        let mut payloads = helio_core::ScenePayloads::default();
        for (key, payload) in published.payloads {
            payloads.insert_any(key, payload);
        }
        for (key, payload) in &self.view_payloads {
            payloads.insert_any(key, payload.clone());
        }

        // The live settings: the TSR preset (the scene content's under
        // "Automatic") and the render scale while TSR is off.
        let project_settings = self.project_settings.load();
        let settings = &project_settings.renderer;
        let tsr_quality = super::project_settings::tsr_quality(
            settings,
            temporal_quality(published.temporal, size),
        );
        self.renderer.set_tsr_quality(tsr_quality);
        if tsr_quality.is_none() {
            let render_scale = (settings.number("render_scale") as f32).clamp(0.25, 1.0);
            if (self.renderer.render_scale() - render_scale).abs() > f32::EPSILON {
                self.renderer.set_render_scale(render_scale);
            }
        }

        // Every frame is camera-relative: its world origin follows the eye
        // (Helio's `Renderer::origin_for`), and the rows the joins derive are
        // relative to it. Voxel coordinates stay global.
        let origin = self.renderer.origin_for(view.eye);
        let camera = match view.projection {
            FrameProjection::Perspective { fov_y, near, far } => {
                // The planes the scene's ground certifies (a voxel world's
                // empty space around the eye): they say nothing about other
                // content (authored meshes), whose close plane is kept.
                let certified = {
                    let mut scene = self.scene.write();
                    self.ground.clip_range(&mut scene.world, view.eye)
                };
                let (near, far) = match certified {
                    Some((ground_near, far)) if read.keeps_near_plane => {
                        (ground_near.min(near), far)
                    }
                    Some(range) => range,
                    None => (near, far),
                };
                Camera::perspective_look_to(
                    view.eye,
                    view.forward,
                    view.up,
                    fov_y,
                    view.aspect,
                    near,
                    far,
                )
            }
            FrameProjection::Orthographic { half_height } => {
                ortho_camera(view.eye, view.forward, view.up, view.aspect, half_height)
            }
        };
        // A view that maps past the frame (a fisheye or barrel-distorted
        // lens) needs a wider render than the frame, which it then crops
        // back; whatever owns the camera's optics says how much
        // (`ViewOverscanRegistration`).
        let overscan = super::render_features::view_overscan(
            &self.scene.read().world,
            camera.view_id,
            view.aspect,
        );
        let camera = camera.with_overscan(overscan);
        let camera = view.crop.map_or(camera.clone(), |crop| {
            camera.with_pixel_crop(crop.full_size, crop.origin, crop.tile_size)
        });
        // Rigid frames: one matrix each, relative to the same origin. Their
        // members' rows never change when a frame moves; this table does.
        // Producers (portal projections) build only what this camera sees,
        // and publish what their passes read about the frame.
        self.spaces
            .set_view(camera.view_proj_in(origin), camera.position_in(origin));
        self.spaces
            .sync(&self.scene, origin, &mut self.renderer, &mut payloads);
        self.renderer.set_scene_payloads(payloads);

        let errors = published.problems;
        let new_errors = (errors != self.last_errors).then(|| {
            for error in &errors {
                tracing::warn!("Scene: {error}");
            }
            self.last_errors = errors.clone();
            errors
        });
        PreparedFrame { camera, new_errors }
    }

    /// Bring the GPU up to date with the scene: bind the assets that
    /// finished loading, flush the GPU mirror and close the frame's change
    /// window. [`Self::render`] does it first.
    pub fn upload(&self) {
        // `SceneDb::step` flushes the mirror too, but writes queued after it
        // this frame (a gizmo drag, a script, a tool) must reach the GPU
        // before the frame reads them.
        crate::scene::pump_assets(&self.scene);
        profiling::profile_scope!("helio_flush_gpu_mirror");
        let store = self.scene.read();
        store.world.flush_gpu_mirror(&self.queue);
        crate::scene::end_change_window(&store.world);
    }

    /// Upload the scene and draw `camera`'s frame into `target`.
    pub fn render(&mut self, camera: &Camera, target: &wgpu::TextureView) -> helio::Result<()> {
        self.upload();
        profiling::profile_scope!("helio_renderer_render");
        self.renderer.render(camera, target)
    }
}

/// An orthographic camera looking at `focus` along `forward`. The eye is the
/// focus itself and the view volume reaches [`ortho_depth_range`] to both
/// sides of it (a near plane behind the eye).
///
/// The eye must stay at the focus: depth-based effects (volumetric fog,
/// atmosphere) integrate along the distance from the eye, so an eye held far
/// back would bury the scene in haze.
pub(super) fn ortho_camera(
    focus: DVec3,
    forward: Vec3,
    up: Vec3,
    aspect: f32,
    half_height: f32,
) -> Camera {
    let range = ortho_depth_range(half_height);
    let half_width = half_height * aspect;
    // The engine's reversed-Z convention (`helio_core::depth::orthographic`):
    // the near plane at depth 1, the far plane at depth 0.
    let proj = glam::camera::rh::proj::directx::orthographic(
        -half_width,
        half_width,
        -half_height,
        half_height,
        range,
        -range,
    );
    let rotation = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, forward, up);
    // `near` and `far` are for the passes that read them as distances from the
    // eye (effect ranges, shadow splits), not the clip planes above.
    Camera::new(focus, rotation, proj, 0.1, range)
}

/// How far an orthographic view reaches ahead of and behind its focus.
pub(super) fn ortho_depth_range(half_height: f32) -> f32 {
    (half_height * 200.0).max(ORTHO_STANDOFF)
}

/// How far behind its focus point an orthographic camera sits, so geometry
/// on both sides of the focus is in front of the near plane.
const ORTHO_STANDOFF: f32 = 4096.0;

/// What the frame reads from the scene: whether it holds content the
/// ground's certified planes say nothing about (a host part's:
/// [`RenderHostPart::keeps_near_plane`]). Read once per world revision.
#[derive(Clone)]
struct SceneRead {
    keeps_near_plane: bool,
}

impl SceneRead {
    fn of(world: &pulsar_scenedb::World, parts: &[Box<dyn RenderHostPart>]) -> Self {
        Self {
            keeps_near_plane: parts.iter().any(|part| part.keeps_near_plane(world)),
        }
    }
}

/// The TSR preset a scene prefers under "Automatic": content that samples
/// stochastically (a voxel world's sun) wants temporal resolve, at a lower
/// render scale above 1.5 megapixels.
fn temporal_quality(content_wants_it: bool, size: [u32; 2]) -> Option<helio::TsrQuality> {
    let pixels = u64::from(size[0]) * u64::from(size[1]);
    content_wants_it.then_some(if pixels > 1_500_000 {
        helio::TsrQuality::Quality
    } else {
        helio::TsrQuality::Native
    })
}
