//! A scene's renderer: Helio's renderer over a shared SceneDB scene, with
//! the scene's voxel worlds and the frame's coordinate space. The one
//! construction the editor viewport (`HelioRenderer`, which adds gizmos,
//! picking, brushes and debug views) and the game (the standalone window and
//! Play-in-Editor) render with, so a played level looks as the editor shows
//! it.
//!
//! Per frame: [`SceneRenderer::step_scene`] advances the scene,
//! [`SceneRenderer::prepare_frame`] streams virtual geometry, places the
//! camera (at its f64 eye; Helio's frames are camera-relative, their origin
//! following it), clip planes the voxel worlds certify, the live TSR preset)
//! and publishes the voxel worlds' frames, [`SceneRenderer::render`] uploads
//! the scene and draws.

use std::sync::Arc;

use glam::{DVec3, Vec3};
use helio::{Camera, Renderer};

use super::voxel_backend::{PlanetVoxelBackend, VoxelBackendRegistry, VoxelChunkSaves, VoxelView};
use crate::scene::voxel_frame::{project_voxel_entries, VoxelWorldSync};
use crate::scene::{SharedScene, SpaceSync, VirtualGeometryHost};
use pulsar_settings::project::virtual_geometry::VirtualGeometrySettings;

/// Who owns the renderer's device.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceOwner {
    /// The renderer (the standalone game's window device).
    Renderer,
    /// A host sharing it (the editor's device, under the viewport and
    /// Play-in-Editor).
    Host,
}

/// How a [`SceneRenderer`] is built.
#[derive(Clone)]
pub struct SceneRendererOptions {
    /// Draw the editor's overlays (the editor viewport, a game launched with
    /// them).
    pub editor_mode: bool,
    pub device: DeviceOwner,
    /// Where saves ask the voxel planet's renderer for its edited chunks.
    pub chunk_saves: VoxelChunkSaves,
    /// The render pipeline asked for (a registered id, see
    /// [`super::pipelines`]); `None`, an unregistered id or the default build
    /// the engine's default graph with its voxel passes.
    pub pipeline: Option<String>,
    /// Virtual-geometry settings in place of the project's (tests, tools).
    pub virtual_geometry: Option<VirtualGeometrySettings>,
}

impl SceneRendererOptions {
    /// A game's renderer: no overlays unless `editor_mode`, nobody saving.
    pub fn game(device: DeviceOwner, editor_mode: bool) -> Self {
        Self {
            editor_mode,
            device,
            chunk_saves: VoxelChunkSaves::default(),
            // The level's pipeline (World Settings).
            pipeline: super::pipelines::level_pipeline(),
            virtual_geometry: None,
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
}

#[derive(Clone, Copy, Debug)]
pub enum FrameProjection {
    /// `fov_y` radians vertically. `near` and `far` are the camera's own clip
    /// planes; over a voxel world the planes it certifies replace them.
    Perspective { fov_y: f32, near: f32, far: f32 },
    /// Looking at the eye from both sides, `half_height` up and down
    /// ([`ortho_camera`]).
    Orthographic { half_height: f32 },
}

/// A frame placed by [`SceneRenderer::prepare_frame`].
pub struct PreparedFrame {
    pub camera: Camera,
    /// The voxel worlds' problems, when they changed this frame (an empty
    /// list: they were fixed). Already logged.
    pub new_errors: Option<Vec<String>>,
}

/// The voxel ground under an eye.
#[derive(Clone, Copy, Debug, Default)]
pub struct VoxelGround {
    /// The local vertical (a planet's radial).
    pub up: Option<DVec3>,
    /// Height above the ground directly below.
    pub altitude: Option<f64>,
}

pub struct SceneRenderer {
    pub renderer: Renderer,
    scene: SharedScene,
    queue: Arc<wgpu::Queue>,
    voxels: VoxelBackendRegistry,
    /// The rigid-frame table last handed to the renderer (slot 0 is the
    /// world), so it is re-uploaded only when a frame moved.
    spaces: SpaceSync,
    /// What the frame reads from the scene, and the world revision it was
    /// read at: camera-only frames reuse it.
    read: Option<(u64, Arc<VoxelSceneRead>)>,
    last_errors: Vec<String>,
    /// Drops voxel edits made on ground a terrain no longer has and keeps
    /// skies with their terrain; created with the first step.
    world_sync: Option<VoxelWorldSync>,
    /// Builds, caches and streams the cluster assets of meshes whose owner
    /// has a `VirtualGeometryComponent`; its runtime feeds the graph.
    virtual_geometry: VirtualGeometryHost,
    virtual_geometry_override: Option<VirtualGeometrySettings>,
    /// The registered pipeline id the graph was built from (`None`: the
    /// default).
    pipeline: Option<String>,
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
        let mut voxels = VoxelBackendRegistry::new();
        voxels
            .register(Box::new(PlanetVoxelBackend::new(options.chunk_saves)))
            .expect("built-in voxel renderer ID must be unique");

        // Every renderer setting comes from the project, through Helio's
        // description of them; under "Automatic" TSR the voxel scene's
        // preset.
        let settings = super::project_settings::renderer();
        let voxel_quality = {
            let store = scene.read();
            let (entries, _) = project_voxel_entries(&store.world);
            voxels.temporal_quality(&entries, [width, height])
        };
        let mut virtual_geometry = VirtualGeometryHost::new();
        let mut config = super::project_settings::renderer_config(width, height, format, &settings)
            .with_virtual_geometry(virtual_geometry.runtime());
        if let Some(quality) = super::project_settings::tsr_quality(&settings, voxel_quality) {
            config = config.with_tsr_quality(quality);
        }

        let scene_db =
            crate::scene::ensure_gpu_mirror(&mut scene.write(), device.clone(), queue.clone());
        let spaces = SpaceSync::default();
        let mut builder = helio::RendererBuilder::new(config, scene_db.clone())
            .with_editor_mode(options.editor_mode)
            // Parent-relative transforms become world-space first, under the
            // same key, so the joins below read them unchanged.
            .with_scene_derivation(crate::scene::hierarchy_resolve(&device))
            // Meshes and lights: Helio joins the authored rows on the GPU.
            .with_scene_derivation(crate::scene::scene_join(&device, options.editor_mode))
            // Virtual geometry: which assets are ready, then which instances
            // are VG-active (both before the authored LOD, which they
            // suppress).
            .with_scene_derivation(crate::scene::virtual_geometry_asset_ready(
                &virtual_geometry,
                &device,
            ))
            .with_scene_derivation(crate::scene::virtual_geometry_instances(
                &mut virtual_geometry,
                &device,
            ))
            .with_scene_derivation(crate::scene::authored_lod_selection(
                &device,
                scene_db,
                // Pulsar's derivations record into one encoder: they share
                // one staging pool.
                crate::gpu::Uploads::new(&device),
            ))
            .with_scene_derivation(crate::scene::environment_join(&device))
            // Authored portals, paired and projected each frame by `spaces`.
            .with_scene_derivation(crate::scene::portal_projection(
                &device,
                spaces.portal_input(),
            ))
            // No default ambient: all illumination comes from the scene.
            .with_ambient([0.0, 0.0, 0.0], 0.0);
        if options.device == DeviceOwner::Host {
            builder = builder.with_external_device();
        }
        // The engine's default keeps its voxel passes; any other registered
        // pipeline is built through `IntoGraph`.
        let pipeline = super::pipelines::resolve_pipeline(options.pipeline.as_deref())
            .filter(|id| *id != helio_default_passes::pipelines::DEFAULT);
        let custom = pipeline
            .as_deref()
            .and_then(|id| super::pipelines::create_pipeline(Some(id)));
        let pipeline = custom.is_some().then_some(pipeline).flatten();
        let builder = match custom {
            Some(graph) => builder.with_boxed_graph(graph),
            None => {
                let voxel_passes = voxels.pass_factories();
                builder.with_pass_build_context(Box::new(move |ctx| {
                    helio_default_passes::build_default_graph_external_with_voxel_passes(
                        ctx,
                        voxel_passes,
                    )
                }))
            }
        };
        let mut renderer = builder.build(device, queue.clone(), width, height, format);
        // Scene materials read through trilinear, anisotropic, repeating
        // filtering (project `streaming.texture_*` settings), not wgpu's
        // nearest/clamp default.
        renderer.set_material_sampler(
            &super::material_sampler::SamplerSettings::from_project().descriptor(),
        );
        Self {
            renderer,
            scene: scene.clone(),
            queue,
            voxels,
            spaces,
            read: None,
            last_errors: Vec::new(),
            world_sync: None,
            virtual_geometry,
            virtual_geometry_override: options.virtual_geometry,
            pipeline,
        }
    }

    /// The registered pipeline the graph was built from (`None`: the
    /// engine's default).
    pub fn pipeline(&self) -> Option<&str> {
        self.pipeline.as_deref()
    }

    /// The virtual-geometry streaming runtime (readiness, settings).
    pub fn virtual_geometry_runtime(&self) -> &helio_vg_stream::VgRuntime {
        self.virtual_geometry.runtime()
    }

    /// Use `settings` instead of the project's virtual-geometry settings
    /// (`None` restores them); takes effect next frame.
    pub fn set_virtual_geometry_settings(&mut self, settings: Option<VirtualGeometrySettings>) {
        self.virtual_geometry_override = settings;
    }

    /// The voxel worlds' renderers (picks, brush rays, the camera's ground).
    pub fn voxels(&self) -> &VoxelBackendRegistry {
        &self.voxels
    }

    /// Whether the voxel worlds need more frames to finish what they show
    /// (a world building, chunks streaming in, a save waiting).
    pub fn needs_frame(&self) -> bool {
        self.voxels.needs_frame(&self.renderer)
    }

    /// The voxel renderers' diagnostics lines (`PULSAR_VOXEL_STATS`).
    pub fn voxel_diagnostics(&self) -> Vec<String> {
        self.voxels.diagnostics(&self.renderer)
    }

    /// Advance `scene` (this renderer's) a frame: voxel worlds' dependents
    /// follow them, then SceneDB steps and flushes its GPU mirror.
    pub fn step_scene(&mut self, scene: &mut pulsar_scenedb::SceneDb) {
        {
            profiling::profile_scope!("helio_sync_voxel_worlds");
            self.world_sync
                .get_or_insert_with(|| VoxelWorldSync::new(&scene.world))
                .poll(&mut scene.world);
        }
        profiling::profile_scope!("helio_scene_store_step");
        scene.step();
    }

    /// What the frame reads from the scene, read once per world revision.
    /// The voxel entries (and what else the frame reads with them) of the
    /// scene's current revision: projected once per revision, shared by the
    /// frame and every brush sample until the scene changes.
    pub(super) fn scene_read(&mut self) -> Arc<VoxelSceneRead> {
        let store = self.scene.read();
        let revision = store.world.revision();
        match &self.read {
            Some((read_at, read)) if *read_at == revision => read.clone(),
            _ => {
                let read = Arc::new(VoxelSceneRead::of(&store.world));
                self.read = Some((revision, read.clone()));
                read
            }
        }
    }

    /// The voxel ground under `eye`, as of the last published frame.
    pub fn voxel_ground(&mut self, eye: DVec3) -> VoxelGround {
        let read = self.scene_read();
        VoxelGround {
            up: self.voxels.local_up(&read.entries, eye),
            altitude: self.voxels.altitude(&read.entries, eye),
        }
    }

    /// The direction towards the scene's sun as the voxel worlds trace it,
    /// if it has one (the last read of the scene).
    pub fn sun_direction(&self) -> Option<[f32; 3]> {
        self.read.as_ref().and_then(|(_, read)| read.sun)
    }

    /// Place this frame's camera and publish the voxel worlds' frames for
    /// it. `size` is the frame's in pixels.
    pub fn prepare_frame(&mut self, view: &FrameView, size: [u32; 2]) -> PreparedFrame {
        profiling::profile_scope!("helio_frame_prepare");
        // Build and stream the virtual-geometry assets the scene asks for.
        crate::scene::sync_virtual_geometry(
            &mut self.virtual_geometry,
            &self.scene,
            self.virtual_geometry_override,
        );
        let read = self.scene_read();

        // The live settings: the TSR preset (the voxel scene's under
        // "Automatic") and the render scale while TSR is off.
        let settings = super::project_settings::renderer();
        let tsr_quality = super::project_settings::tsr_quality(
            &settings,
            self.voxels.temporal_quality(&read.entries, size),
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
        // relative to it. Voxel coordinates stay global (`view.eye`,
        // `VoxelView`).
        let origin = self.renderer.origin_for(view.eye);
        let camera = match view.projection {
            FrameProjection::Perspective { fov_y, near, far } => {
                // Over a voxel world the planes it certifies: its empty space
                // around the eye says nothing about authored meshes, whose
                // close plane is kept.
                let (near, far) = match self.voxels.camera_clip_range(&read.entries, view.eye) {
                    Some((terrain_near, far)) if read.authored_meshes => {
                        (terrain_near.min(near), far)
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
        // A lens that maps past the frame (fisheye, barrel distortion) needs
        // a wider render than the frame, which it then crops back; the camera
        // lens component owns how much, and tells the lens pass the same
        // factor. TODO(review): this names one component's helper from the
        // generic renderer, like the lens GPU column registration in
        // `helio_bridge`; it belongs behind a view-projection hook that helio
        // components register, once one exists.
        let lens_overscan = helio_component::components::camera_lens_view_overscan(
            &self.scene.read().world,
            camera.view_id,
            view.aspect,
        );
        let camera = camera.with_overscan(lens_overscan);
        // Rigid frames: one matrix each, relative to the same origin. Their
        // members' rows never change when a frame moves; this table does.
        // Portal chains are built for the portals this camera can see.
        self.spaces
            .set_view(camera.view_proj_in(origin), camera.position_in(origin));
        self.spaces.sync(&self.scene, origin, &mut self.renderer);

        let mut errors = read.errors.clone();
        errors.extend(
            self.voxels
                .configure_appearance(&mut self.renderer, &read.entries),
        );
        errors.extend(self.voxels.publish_frame(
            &read.entries,
            VoxelView {
                position: view.eye.to_array(),
                sun: read.sun,
            },
        ));
        let new_errors = (errors != self.last_errors).then(|| {
            for error in &errors {
                tracing::warn!("Voxel terrain: {error}");
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

/// What the voxel path reads from the scene: the voxel entries, whether any
/// enabled static mesh exists and the sun direction. Read once per world
/// revision.
#[derive(Clone)]
pub(super) struct VoxelSceneRead {
    pub(super) entries: Vec<crate::scene::voxel_frame::VoxelSceneEntry>,
    errors: Vec<String>,
    authored_meshes: bool,
    sun: Option<[f32; 3]>,
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
        // lights it: enabled, and its owner visible.
        let mut sun_owner = None;
        for (_, (light, owner)) in world.query::<(&LightComponent, &ComponentOwner)>() {
            if !owner.is_enabled() || !light.general.enabled {
                continue;
            }
            if light.general.light_type == LightType::Directional
                && sun_owner.is_none()
                && world
                    .get::<pulsar_scene_model::Visibility>(owner.entity())
                    .is_none_or(|visibility| visibility.visible)
            {
                sun_owner = Some(owner.entity());
            }
        }
        let sun = sun_owner
            .and_then(|owner| crate::scene::hierarchy_transform::world_transform_of(world, owner))
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
        }
    }
}
