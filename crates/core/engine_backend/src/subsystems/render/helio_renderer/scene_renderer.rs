//! A scene's renderer: Helio's renderer over a shared SceneDB scene, with
//! the scene's voxel worlds and the frame's coordinate space. The one
//! construction the editor viewport (`HelioRenderer`, which adds gizmos,
//! picking, brushes and debug views) and the game (the standalone window and
//! Play-in-Editor) render with, so a played level looks as the editor shows
//! it.
//!
//! Per frame: [`SceneRenderer::step_scene`] advances the scene,
//! [`SceneRenderer::prepare_frame`] streams virtual geometry, places the camera (camera-relative over
//! large worlds, clip planes the voxel worlds certify, the live TSR preset)
//! and publishes the voxel worlds' frames, [`SceneRenderer::render`] uploads
//! the scene and draws.

use std::sync::Arc;

use glam::{DVec3, Vec3};
use helio::{Camera, Renderer};

use super::voxel_backend::{
    PlanetVoxelBackend, VoxelBackendRegistry, VoxelChunkSaves, VoxelView,
};
use super::voxel_mesh_backend::MeshVoxelBackend;
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
    /// ([`ortho_frame_camera`]).
    Orthographic { half_height: f32 },
}

/// A frame placed by [`SceneRenderer::prepare_frame`].
pub struct PreparedFrame {
    pub camera: Camera,
    /// Whether the frame is camera-relative (its world origin near the eye).
    pub camera_relative: bool,
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
    relative_camera_gate: RelativeCameraGate,
    /// Coordinate space of the last frame (camera-relative or world).
    last_camera_relative: Option<bool>,
    /// This frame's world origin when it renders camera-relative
    /// ([`frame_world_origin`]).
    world_origin: Option<DVec3>,
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
        // Free-standing voxel objects: mesh rows in the shared scene.
        voxels
            .register(Box::new(MeshVoxelBackend::new(Some(scene.clone()))))
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

        let scene_db = crate::scene::ensure_gpu_mirror(&mut scene.write(), device.clone(), queue.clone());
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
            .with_scene_derivation(crate::scene::virtual_geometry_asset_ready(&virtual_geometry, &device))
            .with_scene_derivation(crate::scene::virtual_geometry_instances(&mut virtual_geometry, &device))
            .with_scene_derivation(crate::scene::authored_lod_selection(&device, scene_db))
            .with_scene_derivation(crate::scene::environment_join(&device))
            // Authored portals, paired and projected each frame by `spaces`.
            .with_scene_derivation(crate::scene::portal_projection(&device, spaces.portal_input()))
            // No default ambient: all illumination comes from the scene.
            .with_ambient([0.0, 0.0, 0.0], 0.0);
        if options.device == DeviceOwner::Host {
            builder = builder.with_external_device();
        }
        // The engine's default keeps its voxel passes; any other registered
        // pipeline is built through `IntoGraph`.
        let pipeline = super::pipelines::resolve_pipeline(options.pipeline.as_deref())
            .filter(|id| *id != helio_default_passes::pipelines::DEFAULT);
        let custom = pipeline.as_deref().and_then(|id| super::pipelines::create_pipeline(Some(id)));
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
            relative_camera_gate: RelativeCameraGate::default(),
            last_camera_relative: None,
            world_origin: None,
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
    fn scene_read(&mut self) -> Arc<VoxelSceneRead> {
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
        let wants_relative = self.voxels.uses_camera_relative_frames(&read.entries);
        let camera_relative = wants_relative && {
            let store = self.scene.read();
            self.relative_camera_gate
                .compatible(&store.world, read.relative_camera_refusals.is_empty())
        };
        if self.last_camera_relative != Some(camera_relative) {
            // A switch of coordinate space discards temporal history and
            // changes how the planet and sky are placed: visible as a flash.
            // It must only follow a real scene change. A large world refused
            // camera-relative frames from the start renders in f32 world
            // coordinates far from the origin (jitter, blocks, broken view
            // rays): name what refused it.
            if self.last_camera_relative.is_some() || (wants_relative && !camera_relative) {
                let store = self.scene.read();
                tracing::warn!(
                    camera_relative,
                    world = ?read.relative_camera_refusals,
                    incompatible = ?relative_camera_incompatible_sources(&store.world),
                    "VOXEL_CAMERA_SPACE changed"
                );
            }
            self.last_camera_relative = Some(camera_relative);
        }

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

        // Canonical voxel coordinates stay global (`view.eye`, `VoxelView`);
        // only the frame camera is origin-relative.
        self.world_origin = camera_relative.then(|| frame_world_origin(self.world_origin, view.eye));
        self.renderer.set_world_origin(self.world_origin);
        let origin = self.world_origin.unwrap_or(DVec3::ZERO);
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
                native_frame_camera(view.eye, view.forward, view.up, fov_y, view.aspect, near, far, origin)
            }
            FrameProjection::Orthographic { half_height } => {
                ortho_frame_camera(view.eye, view.forward, view.up, view.aspect, half_height, origin)
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
        self.spaces.set_view(camera.proj * camera.view, camera.position);
        self.spaces.sync(&self.scene, origin, &mut self.renderer);

        let mut errors = read.errors.clone();
        errors.extend(self.voxels.configure_appearance(&mut self.renderer, &read.entries));
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
        PreparedFrame {
            camera,
            camera_relative,
            new_errors,
        }
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
        let snapshot = (store as *const _ as usize, store.buffer_registry().len());
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
        "object_transforms" => pulsar_scene_model::TransformRow::packed_gpu_component_id(),
        "light_sources" => c::LightSourceRow::packed_gpu_component_id(),
        "water_hitbox_sources" => pulsar_physics::WaterHitboxSourceRow::packed_gpu_component_id(),
        "global_fog_sources" => c::GlobalFogSourceRow::packed_gpu_component_id(),
        "local_fog_sources" => c::LocalFogSourceRow::packed_gpu_component_id(),
        "post_process_volume_sources" => c::PostProcessVolumeSourceRow::packed_gpu_component_id(),
        "camera_postprocess_sources" => c::CameraPostProcessSourceRow::packed_gpu_component_id(),
        "camera_lens_sources" | "lens_elements" => {
            return c::lens_buffer_component_id(key.as_str()).is_some_and(|id| {
                store.buffer_registry().element_type(key) == Some(Some(type_of(id)))
            });
        }
        "water_volume_sources" => c::WaterVolumeSourceRow::packed_gpu_component_id(),
        "foliage_sources" => c::FoliageSourceRow::packed_gpu_component_id(),
        "atmosphere_sources" => c::AtmosphereSourceRow::packed_gpu_component_id(),
        "decal_sources" => c::DecalSourceRow::packed_gpu_component_id(),
        "corona_emitter_sources" => c::CoronaEmitterSourceRow::packed_gpu_component_id(),
        "wind_sources" => c::GlobalWindSourceRow::packed_gpu_component_id(),
        "sprite_sources" => c::SpriteSourceRow::packed_gpu_component_id(),
        "builtin_mesh_vertex::handles"
        | "builtin_mesh_index::handles"
        | "skeletal_skin_vertices::handles"
        | "skeletal_bone_palette::handles"
        // Authored LOD thresholds and virtual-geometry overrides: no
        // positions (the LOD selection measures distance in the frame).
        | "lod_component_levels::handles"
        | "vg_instance_overrides::handles" => {
            return store.buffer_registry().element_type(key)
                == Some(Some(std::any::TypeId::of::<
                    pulsar_scenedb::gpu::VarLenHandle,
                >()));
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
        "scenedb-instances"
        | "scenedb-instance-info"
        | "builtin_generation"
        | "builtin_slot_mirror"
        | "builtin_cell_metadata"
        | "component_owners"
        | "object_hidden"
        // Parent indices, no positions; the hierarchy resolve composes the
        // (camera-relative-safe) transform rows with them.
        | "object_parent"
        | "object_frame"
        | "object_transforms"
        | "light_sources"
        | "decal_sources"
        | "water_hitbox_sources"
        | "global_fog_sources"
        | "local_fog_sources"
        | "post_process_volume_sources"
        | "camera_postprocess_sources"
        | "camera_lens_sources"
        | "lens_elements"
        | "water_volume_sources"
        | "foliage_sources"
        | "atmosphere_sources"
        | "corona_emitter_sources"
        | "wind_sources"
        | "sprite_sources"
        | "static_mesh_draw_bounds"
        | "static_mesh_draw_flags"
        | "builtin_mesh_vertex::handles"
        | "builtin_mesh_index::handles"
        | "static_mesh_draw_sections::handles"
        | "skeletal_skin_vertices::handles"
        | "skeletal_bone_palette::handles" => {
            kind == "row" && mode == Some(MirrorMode::DirtyTracked)
        }
        // Skin weights carry no positions, and the bone palette is in the
        // mesh's own space: passes apply it before the instance's model
        // matrix, which is what a moved origin rebases.
        "builtin_mesh_vertex"
        | "builtin_mesh_index"
        | "static_mesh_draw_sections"
        | "skeletal_skin_vertices"
        | "skeletal_bone_palette"
        | "lod_component_levels"
        | "vg_instance_overrides" => kind == "resource" && mode.is_none(),
        _ => false,
    }
}

/// What keeps the scene out of camera-relative frames: every component type
/// not reviewed for a moved origin. Empty when the scene is compatible.
fn relative_camera_world_incompatibilities(world: &pulsar_scenedb::World) -> Vec<String> {
    use crate::scene::{
        ComponentAttachments, Name, ObjectType, Parent, RenderProps, Selected, SiblingIndex,
        StableId, Transform, Visibility,
    };
    use pulsar_scenedb::component_id;
    let mut found = Vec::new();
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
        // The voxel world itself, traced camera-relative.
        component_id::<helio_component::VoxelTerrainComponent>(),
        component_id::<helio_component::VoxelTerrainLayersComponent>(),
        // A free-standing voxel object: its chunks are its mesh rows, which
        // the scene join places like any mesh (below).
        component_id::<helio_component::VoxelComponent>(),
        // Lights of every type: the scene join writes their positions
        // relative to the world origin, so culling and shadow matrices are
        // in the frame's space; their editor icons are billboards, which
        // subtract the origin themselves.
        component_id::<helio_component::components::LightComponent>(),
        // Meshes: the scene join writes their model matrices and bounds
        // relative to the world origin (Pulsar-Native#1127).
        component_id::<helio_component::components::StaticMeshComponent>(),
        // The atmosphere pass subtracts the world origin from the planet's
        // centre; volume blending rebases volume bounds by it; a camera's
        // post-process baseline has no position.
        component_id::<helio_component::AtmosphereComponent>(),
        component_id::<helio_component::PostProcessVolumeComponent>(),
        component_id::<helio_component::CameraPostProcessComponent>(),
        // A camera lens has no position.
        component_id::<helio_component::CameraLensComponent>(),
        // The environment join places decals, particle emitters, water
        // volumes and the water bodies push relative to the world origin;
        // the Corona pass moves live particles, and the water simulation
        // its waves and previous body bounds, with the origin.
        component_id::<helio_component::components::DecalComponent>(),
        component_id::<helio_component::components::ParticleEmitterComponent>(),
        component_id::<helio_component::WaterVolumeComponent>(),
        component_id::<pulsar_physics::PhysicsComponent>(),
        // Fog volumes stay in world space: the fog pass adds the world
        // origin to the positions it reconstructs.
        component_id::<helio_component::components::GlobalFogComponent>(),
        component_id::<helio_component::components::LocalFogVolumeComponent>(),
        // Splines draw as world-space lines in the editor overlay, which
        // subtracts the world origin; LOD settings draw nothing.
        component_id::<helio_component::components::SplineComponent>(),
        component_id::<helio_component::components::LODComponent>(),
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

/// The frame camera at `eye`, looking along `forward`, in coordinates
/// relative to `origin` (the frame's world origin; zero for a world-space
/// frame). Orientation is built before translation, and the translation in
/// f64: adding a unit direction to a large f32 world position can round
/// away the look direction.
#[allow(clippy::too_many_arguments)]
pub(super) fn native_frame_camera(
    eye: DVec3,
    forward: Vec3,
    up: Vec3,
    fov_y: f32,
    aspect: f32,
    near: f32,
    far: f32,
    origin: DVec3,
) -> Camera {
    let mut camera = Camera::perspective_look_at(
        Vec3::ZERO,
        forward,
        up,
        fov_y,
        aspect,
        near,
        far,
    );
    let local = eye - origin;
    if local != DVec3::ZERO {
        camera.position = local.as_vec3();
        camera.view = (camera.view.as_dmat4() * glam::DMat4::from_translation(-local)).as_mat4();
    }
    camera
}

/// An orthographic frame camera looking at `focus` along `forward`, in
/// coordinates relative to `origin` like [`native_frame_camera`]. The eye is
/// the focus itself and the view volume reaches [`ortho_depth_range`] to both
/// sides of it (a near plane behind the eye).
///
/// The eye must stay at the focus: depth-based effects (volumetric fog,
/// atmosphere) integrate along the distance from the eye, so an eye held far
/// back would bury the scene in haze.
pub(super) fn ortho_frame_camera(
    focus: DVec3,
    forward: Vec3,
    up: Vec3,
    aspect: f32,
    half_height: f32,
    origin: DVec3,
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
    let local = focus - origin;
    let rotation = glam::camera::rh::view::look_at_mat4(Vec3::ZERO, forward, up);
    let view = (rotation.as_dmat4() * glam::DMat4::from_translation(-local)).as_mat4();
    // `near` and `far` are for the passes that read them as distances from the
    // eye (effect ranges, shadow splits), not the clip planes above.
    Camera::from_matrices(view, proj, local.as_vec3(), 0.1, range)
}

/// How far an orthographic view reaches ahead of and behind its focus.
pub(super) fn ortho_depth_range(half_height: f32) -> f32 {
    (half_height * 200.0).max(ORTHO_STANDOFF)
}

/// How far behind its focus point an orthographic camera sits, so geometry
/// on both sides of the focus is in front of the near plane.
const ORTHO_STANDOFF: f32 = 4096.0;

/// Side of the cells camera-relative frames snap their world origin to.
/// Within one cell of the origin, an f32 position steps by under 0.2 mm.
const WORLD_ORIGIN_CELL: f64 = 1024.0;

/// The world origin for a camera-relative frame with the camera at `eye`:
/// `previous` while the camera stays within a cell of it, else the cell
/// corner nearest the camera. Positions the scene join rebases are
/// recomputed only when it moves, not every frame the camera does, so
/// static draws and shadow caches survive camera motion.
fn frame_world_origin(previous: Option<DVec3>, eye: DVec3) -> DVec3 {
    match previous {
        Some(origin) if (eye - origin).abs().max_element() <= WORLD_ORIGIN_CELL => origin,
        _ => (eye / WORLD_ORIGIN_CELL).round() * WORLD_ORIGIN_CELL,
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
            .and_then(|owner| {
                crate::scene::hierarchy_transform::world_transform_of(world, owner)
            })
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
            relative_camera_refusals: relative_camera_world_incompatibilities(world),
        }
    }
}

#[cfg(test)]
mod native_relative_camera_tests {
    use super::*;
    use crate::scene::{SceneWorldExt, SpawnObject};
    use glam::Mat4;

    const FOV: f32 = std::f32::consts::FRAC_PI_4;
    use helio_component::components::{LightComponent, LightType};

    /// The example planet's objects: a terrain with its layers and air, a
    /// post-process volume and a directional sun, attached as instances.
    /// Returns the scene and the sun's light instance.
    fn planet_scene() -> (pulsar_scenedb::SceneDb, pulsar_scenedb::Entity) {
        let mut scene = pulsar_scenedb::SceneDb::new();
        let world = &mut scene.world;
        let planet = world
            .spawn_object(SpawnObject::new("voxel_planet"))
            .unwrap();
        pulsar_world_registry::attach_value(
            world,
            planet,
            helio_component::VoxelTerrainComponent::default(),
        )
        .unwrap();
        pulsar_world_registry::attach_value(
            world,
            planet,
            helio_component::VoxelTerrainLayersComponent::default(),
        )
        .unwrap();
        pulsar_world_registry::attach_value(
            world,
            planet,
            helio_component::AtmosphereComponent::default(),
        )
        .unwrap();
        let volume = world
            .spawn_object(SpawnObject::new("post_process"))
            .unwrap();
        pulsar_world_registry::attach_value(
            world,
            volume,
            helio_component::PostProcessVolumeComponent::default(),
        )
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
        assert!(
            read.relative_camera_refusals.is_empty(),
            "{:?}",
            read.relative_camera_refusals
        );
        assert!(read.sun.is_some(), "the directional light is the sun");
        assert_eq!(read.entries.len(), 1);
        // A new, unreviewed world-space consumer fails closed; retiring its
        // last live row restores eligibility.
        struct UnknownWorldSpaceProvider;
        let entity = scene.world.spawn();
        scene.world.insert(entity, UnknownWorldSpaceProvider);
        assert!(!VoxelSceneRead::of(&scene.world)
            .relative_camera_refusals
            .is_empty());
        scene.world.despawn(entity);
        assert!(VoxelSceneRead::of(&scene.world)
            .relative_camera_refusals
            .is_empty());
        // Meshes and positional lights are rebased by the scene join
        // (Pulsar-Native#1127): a tower and a lamp on the planet keep the
        // frame camera-relative.
        let tower = scene.world.spawn_object(SpawnObject::new("tower")).unwrap();
        pulsar_world_registry::attach_value(
            &mut scene.world,
            tower,
            helio_component::components::StaticMeshComponent::default(),
        )
        .unwrap();
        scene
            .world
            .get_mut::<LightComponent>(sun)
            .unwrap()
            .general
            .light_type = LightType::Point;
        let read = VoxelSceneRead::of(&scene.world);
        assert!(
            read.relative_camera_refusals.is_empty(),
            "{:?}",
            read.relative_camera_refusals
        );
        assert!(read.authored_meshes);
        assert!(read.sun.is_none(), "a point light is not the sun");

        // So are decals, particles, water and the bodies that push it, fog,
        // splines and free-standing voxel objects.
        let props = scene.world.spawn_object(SpawnObject::new("props")).unwrap();
        let world = &mut scene.world;
        use helio_component::components as c;
        pulsar_world_registry::attach_value(world, props, c::DecalComponent::default()).unwrap();
        pulsar_world_registry::attach_value(world, props, c::ParticleEmitterComponent::default())
            .unwrap();
        pulsar_world_registry::attach_value(
            world,
            props,
            helio_component::WaterVolumeComponent::default(),
        )
        .unwrap();
        pulsar_world_registry::attach_value(
            world,
            props,
            pulsar_physics::PhysicsComponent::default(),
        )
        .unwrap();
        pulsar_world_registry::attach_value(world, props, c::GlobalFogComponent::default())
            .unwrap();
        pulsar_world_registry::attach_value(world, props, c::LocalFogVolumeComponent::default())
            .unwrap();
        pulsar_world_registry::attach_value(world, props, c::SplineComponent::default()).unwrap();
        pulsar_world_registry::attach_value(world, props, c::LODComponent::default()).unwrap();
        pulsar_world_registry::attach_value(
            world,
            props,
            helio_component::VoxelComponent::default(),
        )
        .unwrap();
        let read = VoxelSceneRead::of(&scene.world);
        assert!(
            read.relative_camera_refusals.is_empty(),
            "{:?}",
            read.relative_camera_refusals
        );

        // Foliage is grown in world tiles: it keeps frames in world space.
        pulsar_world_registry::attach_value(
            &mut scene.world,
            props,
            c::FoliageComponent::default(),
        )
        .unwrap();
        let read = VoxelSceneRead::of(&scene.world);
        assert!(
            read.relative_camera_refusals
                .iter()
                .any(|name| name.contains("Foliage")),
            "{:?}",
            read.relative_camera_refusals
        );
    }

    /// The origin follows the camera a cell at a time, so the positions the
    /// scene join rebases are recomputed when it moves rather than every
    /// frame the camera does, and the camera stays close to it.
    #[test]
    fn the_world_origin_snaps_to_cells_and_follows_the_camera() {
        let eye = DVec3::new(-3_426_954.1, 4_769_291.6, -2_476_450.5);
        let origin = frame_world_origin(None, eye);
        assert_eq!(origin, DVec3::new(-3_427_328.0, 4_769_792.0, -2_476_032.0));
        assert!((eye - origin).abs().max_element() <= WORLD_ORIGIN_CELL / 2.0);
        // Moving within a cell keeps it; leaving moves it to the new cell.
        let near = eye + DVec3::new(500.0, -400.0, 300.0);
        assert_eq!(frame_world_origin(Some(origin), near), origin);
        let far = eye + DVec3::new(5_000.0, 0.0, 0.0);
        let moved = frame_world_origin(Some(origin), far);
        assert_ne!(moved, origin);
        assert!((far - moved).abs().max_element() <= WORLD_ORIGIN_CELL / 2.0);
    }

    /// A mesh vertex on the planet's surface, in a frame relative to the
    /// snapped origin, lands where the exact (f64) projection puts it: what
    /// a world-space frame got 0.5 m out (the jagged mesh in #1127).
    #[test]
    fn surface_geometry_projects_exactly_in_a_camera_relative_frame() {
        let eye = DVec3::new(
            -3_426_954.099417845,
            4_769_291.620032467,
            -2_476_450.5275892294,
        );
        let forward = Vec3::new(0.9167773, 0.29777223, -0.2661786);
        let up = Vec3::new(-0.39032218, 0.8092439, -0.43905908);
        let origin = frame_world_origin(None, eye);
        let relative = native_frame_camera(eye, forward, up, FOV, 1.5, 0.1, 46_371_000.0, origin);
        let world = native_frame_camera(eye, forward, up, FOV, 1.5, 0.1, 46_371_000.0, DVec3::ZERO);
        // A vertex 25 m in front of the camera, 0.3 m off a 0.5 m grid line.
        let vertex = eye + forward.as_dvec3().normalize() * 25.0 + DVec3::new(0.3, 0.0, 0.0);
        let exact = {
            let view = glam::DMat4::look_to_rh(eye, forward.as_dvec3(), up.as_dvec3());
            let clip = relative.proj.as_dmat4() * view * vertex.extend(1.0);
            clip.truncate() / clip.w
        };
        let ndc = |camera: &Camera, point: Vec3| {
            let clip = camera.proj * camera.view * point.extend(1.0);
            (clip.truncate() / clip.w).as_dvec3()
        };
        let rebased = ndc(&relative, (vertex - origin).as_vec3());
        let absolute = ndc(&world, vertex.as_vec3());
        assert!(
            (rebased - exact).truncate().length() < 1e-4,
            "{rebased} vs {exact}"
        );
        assert!(
            (absolute - exact).truncate().length() > 1e-3,
            "the world-space frame snaps it"
        );
    }

    /// Every buffer this engine registers in the GPU mirror is reviewed:
    /// a frame of the example planet is not refused by its own sources.
    #[test]
    fn the_engines_gpu_sources_are_camera_relative_compatible() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some(adapter) = pollster::block_on(instance.request_adapter(&Default::default())).ok()
        else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: helio::required_wgpu_limits(adapter.limits()),
                ..Default::default()
            }))
        else {
            eprintln!("skipping: no GPU device available");
            return;
        };
        let (mut scene, _) = planet_scene();
        crate::scene::ensure_gpu_mirror(&mut scene, Arc::new(device), Arc::new(queue));
        scene.step();
        assert_eq!(
            relative_camera_incompatible_sources(&scene.world),
            Vec::<String>::new()
        );
        let read = VoxelSceneRead::of(&scene.world);
        let mut gate = RelativeCameraGate::default();
        assert!(gate.compatible(&scene.world, read.relative_camera_refusals.is_empty()));
    }

    #[test]
    fn unknown_or_raw_render_sources_do_not_activate_relative_camera() {
        use pulsar_scenedb::{gpu::BufferAccess::ReadOnly, MirrorMode};
        assert!(relative_camera_source_compatible(
            "component_owners",
            "row",
            Some(MirrorMode::DirtyTracked),
            ReadOnly
        ));
        assert!(!relative_camera_source_compatible(
            "component_owners",
            "resource",
            None,
            ReadOnly
        ));
        for key in [
            "reflection_captures",
            "portal_views",
            "corona_emitters",
            "foliage_layers",
            "custom_render_source",
        ] {
            assert!(
                !relative_camera_source_compatible(
                    key,
                    "row",
                    Some(MirrorMode::DirtyTracked),
                    ReadOnly
                ),
                "{key}"
            );
        }
    }

    #[test]
    fn earth_scale_terrain_depth_reprojects_in_local_camera_space() {
        let eye = DVec3::new(-3426954.099417845, 4769291.620032467, -2476450.5275892294);
        let forward = Vec3::new(0.9167773, 0.29777223, -0.2661786);
        let up = Vec3::new(-0.39032218, 0.8092439, -0.43905908);
        let local = native_frame_camera(eye, forward, up, FOV, 1196.0 / 729.0, 2.4, 46_371_000.0, eye);
        let global = native_frame_camera(
            eye,
            forward,
            up,
            FOV,
            1196.0 / 729.0,
            2.4,
            46_371_000.0,
            DVec3::ZERO,
        );
        assert_eq!(local.position, Vec3::ZERO);
        assert_eq!(global.position, eye.as_vec3());
        let local_vp = local.proj * local.view;
        let global_vp = global.proj * global.view;
        // A surface 10 m ahead, reconstructed from its depth texel (as TSR
        // and the deferred pass do) in either frame.
        let point = (forward.normalize() * 10.0).extend(1.0);
        let clip = local_vp * point;
        let raster = clip / clip.w;
        let reconstruct = |vp: Mat4| {
            let h = vp.inverse() * raster;
            (h.truncate() / h.w).as_dvec3()
        };
        let surface = eye + point.truncate().as_dvec3();
        // Global f32 frames at Earth radius step by half a metre: the
        // surface lands metres away. The camera's own frame is exact.
        let global_error = (reconstruct(global_vp) - surface).length();
        let local_error = (reconstruct(local_vp) + eye - surface).length();
        eprintln!("reconstruction error: global {global_error} m, local {local_error} m");
        assert!(
            global_error > 0.1,
            "regression trigger must expose global-f32 cancellation"
        );
        assert!(local_error < 1.0e-4, "{local_error} m");
        // Express the current relative point in the previous frame's origin
        // exactly once; sub-voxel camera movement must survive Earth-scale
        // positions.
        let previous_eye = eye - DVec3::new(0.01, -0.03, 0.02);
        let shift = eye - previous_eye;
        let previous_view =
            (local.view.as_dmat4() * glam::DMat4::from_translation(shift)).as_mat4();
        let expected = (local.view.as_dmat4() * (point.as_dvec4() + shift.extend(0.0))).as_vec4();
        assert!((previous_view * point - expected).length() < 0.00001);
    }
}
