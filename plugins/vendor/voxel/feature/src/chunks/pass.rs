//! Chunk trees in Helio's render graph: residency, primary visibility, sun
//! and sky light, and the gbuffer, of the volumes the scene publishes
//! ([`VOXEL_FRAME`]), each placed by its transform, in one world. Lighting is
//! Helio's deferred lighting, its sun shadowed by walks through the chunk
//! trees (`directional_visibility`). It answers what each volume's owner
//! asks of the drawn world (picks, a save's chunks) into the owner's inbox.

use super::{
    residency, ChunkWorld, EditSync, FrameTimings, ResidencyConfig, UpdateStats, VolumeId,
};
use crate::engine::{PlanetFrame, TerrainAppearance, VolumeAnswer, VOXEL_FRAME};
use glam::{DVec3, Mat4, Vec2, Vec3};
use helio_core::Result as HelioResult;
use helio_core::{
    DebugViewDescriptor, DebugViewLegend, DebugViewLegendStop, PassContext, PrepareContext,
    RenderPass,
};
use std::sync::Arc;

/// A frame's volume as the world holds it: the host's id, the world's, the
/// recipe it was added for (fingerprint), the look shown, and its owner's
/// requests in hand.
struct HeldVolume {
    id: u64,
    volume: VolumeId,
    recipe: u64,
    /// The appearance and smoothing shown.
    look: (Option<TerrainAppearance>, f32),
    /// The keep request whose chunks are packing, and the last answered.
    exporting: Option<u64>,
    kept: Option<u64>,
    /// The newest pick request taken (answered here or not this camera's).
    picked: u64,
}

/// Frames to keep drawing after a volume's look changed, so temporal
/// history settles on it before an on-demand host goes idle.
const LOOK_SETTLE_FRAMES: u32 = 32;

/// The appearance a volume shows: the host's, else `generator`'s own, lit
/// `smoothing` smooth.
fn shown_appearance(
    host: Option<TerrainAppearance>,
    smoothing: f32,
    generator: impl FnOnce() -> TerrainAppearance,
) -> TerrainAppearance {
    TerrainAppearance {
        smoothing,
        ..host.unwrap_or_else(generator)
    }
}

pub struct ChunkPass {
    config: ResidencyConfig,
    debug_mode: u32,
    world: Option<ChunkWorld>,
    /// The frame's volumes the world holds.
    held: Vec<HeldVolume>,
    shown: Option<Arc<PlanetFrame>>,
    /// Frames left to draw after a look changed ([`LOOK_SETTLE_FRAMES`]).
    settling: u32,
    /// How the kept chunks of the last volume added went into it: how
    /// many, or why they were refused.
    kept_loaded: Option<Result<usize, String>>,
    last_eye: [Option<(DVec3, std::time::Instant)>; 2],
    /// Stereo draws can share a head-centred residency viewpoint while tracing
    /// from each actual eye. Inter-eye distance must not become movement velocity.
    residency_eye: Option<DVec3>,
    stats: UpdateStats,
    /// Frames whose GPU times came back, until taken.
    timings: Vec<FrameTimings>,
    /// Generation's milliseconds last frame (`ResidencyConfig::frame_ms`).
    budget_ms: Option<f64>,
    /// The last frame's CPU milliseconds: prepare, execute.
    cpu_ms: [f64; 2],
    /// Voxel materials as the host's scene materials (`set_scene_materials`).
    scene: Option<(
        super::scene::VoxelMaterialMap,
        super::scene::SceneMaterialSource,
    )>,
    /// Every hit walks its own sun ray (`set_steady_shadows`).
    steady_shadows: bool,
    /// The eye's world position this frame (`helio_gi_api::eye`), where
    /// the GI clipmap the terrain traces rays of is centred.
    gi_eye: Option<DVec3>,
    /// The last frame's projection jitter (NDC).
    last_jitter: [Vec2; 2],
    xr_mode: bool,
    /// The terrain as the ground foliage grows on (`chunks::ground`): shared with a
    /// foliage placement pass ([`Self::foliage_ground`]), its tiling and its answers.
    planet_ground: std::sync::Arc<super::ground::PlanetGround>,
    foliage_ground: std::sync::Arc<helio_foliage_api::GroundLink>,
}

impl ChunkPass {
    /// A pass drawing the volumes the scene publishes ([`VOXEL_FRAME`]),
    /// with residency settings `config` (its pixel angle follows the
    /// camera).
    pub fn new(config: ResidencyConfig) -> Self {
        let planet_ground = std::sync::Arc::new(super::ground::PlanetGround::default());
        let foliage_ground = std::sync::Arc::new(
            helio_foliage_api::GroundLink::answered_elsewhere(Box::new(planet_ground.clone())),
        );
        Self {
            config,
            debug_mode: 0,
            world: None,
            held: Vec::new(),
            shown: None,
            settling: 0,
            kept_loaded: None,
            last_eye: [None; 2],
            residency_eye: None,
            stats: UpdateStats::default(),
            timings: Vec::new(),
            budget_ms: None,
            cpu_ms: [0.0; 2],
            scene: None,
            steady_shadows: false,
            gi_eye: None,
            last_jitter: [Vec2::ZERO; 2],
            xr_mode: false,
            planet_ground,
            foliage_ground,
        }
    }

    /// The terrain as the ground foliage grows on: give it to a foliage placement pass
    /// (`FoliagePlacePass::set_ground`); this pass answers its queries and reports where
    /// the terrain changed under it.
    pub fn foliage_ground(&self) -> std::sync::Arc<helio_foliage_api::GroundLink> {
        self.foliage_ground.clone()
    }

    /// Voxel materials shown as the host's scene materials: `map` says which
    /// scene material rows each voxel material's faces use, `source` reads
    /// the frame's material table from the context (see
    /// `super::scene::helio_scene_materials`). Devices without
    /// texture binding arrays keep the appearance table's colours.
    pub fn set_scene_materials(
        &mut self,
        map: super::scene::VoxelMaterialMap,
        source: super::scene::SceneMaterialSource,
    ) {
        self.scene = Some((map, source));
    }

    /// Steady shadows: every hit walks its own sun ray and the sky rays hold
    /// still, instead of samples that move each frame for the denoiser to
    /// gather (about three times dearer; a reference for the denoised
    /// shadows).
    pub fn set_steady_shadows(&mut self, on: bool) {
        self.steady_shadows = on;
    }

    /// Override the position used for residency/prefetch, without changing the
    /// frame's tracing eye. Use the head centre for both draws of a stereo pair.
    pub fn set_residency_eye(&mut self, eye: Option<DVec3>) {
        self.residency_eye = eye;
    }

    /// Whether residency settles, a save's chunks pack, picks wait or a new
    /// look settles: a host rendering on demand keeps drawing meanwhile.
    fn wants_frames(&self) -> bool {
        let asked = self.shown.as_ref().is_some_and(|frame| {
            frame.volumes.iter().any(|v| {
                let held = self.held.iter().find(|h| h.id == v.volume.id);
                let keep = v
                    .requests
                    .keep
                    .is_some_and(|id| held.is_none_or(|h| h.kept != Some(id)));
                let picks = v
                    .requests
                    .picks
                    .iter()
                    .any(|r| held.is_none_or(|h| r.id > h.picked));
                keep || picks
            })
        });
        asked
            || self.settling > 0
            || self.world.as_ref().is_some_and(ChunkWorld::picks_in_flight)
            || self.world.as_ref().is_none_or(|w| !w.settled())
                && self.shown.as_ref().is_some_and(|f| !f.volumes.is_empty())
    }

    /// How the kept chunks of the last volume added went into it: how many
    /// it keeps, or why they were refused (`None`: it had none).
    pub fn kept_loaded(&self) -> Option<&Result<usize, String>> {
        self.kept_loaded.as_ref()
    }

    /// Whether volumes' chunks are held.
    pub fn has_world(&self) -> bool {
        self.world.is_some()
    }

    /// The world's volume of host volume `id`, once it holds it.
    pub fn volume_of(&self, id: u64) -> Option<VolumeId> {
        self.held.iter().find(|h| h.id == id).map(|h| h.volume)
    }

    /// The volumes' chunks, for diagnostics (trace counts, a pixel's trace).
    pub fn world(&self) -> Option<&ChunkWorld> {
        self.world.as_ref()
    }

    /// Whether nothing is left to generate for the current eye.
    pub fn settled(&self) -> bool {
        self.world.as_ref().is_some_and(ChunkWorld::settled)
    }

    /// What the sun's occlusion pages did last frame (`ChunkWorld::sun_page_stats`).
    pub fn sun_page_stats(&self) -> Option<super::SunPageStats> {
        self.world.as_ref().and_then(ChunkWorld::sun_page_stats)
    }

    /// The last frame's residency.
    pub fn stats(&self) -> &UpdateStats {
        &self.stats
    }

    /// The last frame's CPU milliseconds in the pass: prepare (edits,
    /// results), execute (residency, draw).
    pub fn cpu_ms(&self) -> [f64; 2] {
        self.cpu_ms
    }

    /// GPU times of frames that came back since the last call.
    pub fn take_timings(&mut self) -> Vec<FrameTimings> {
        std::mem::take(&mut self.timings)
    }

    /// One line for a host's diagnostics ([`RenderPass::diagnostics`]).
    pub fn stats_line(&self) -> String {
        let s = &self.stats;
        let gpu = self.timings.last();
        format!(
            "chunks settled={} kept_loaded={:?} kept_chunks={} kept_bytes={} detail_pages={} detail_capacity={} chunk_slots={} leaf_pages={} leaf_page_capacity={} grow_ms={:.2} out_of_details={} drawn_nodes={} drawn_chunks={} resident_chunks={} evicted={} deferred={} fading={} waiting={} footprints={} chunks={} reports={} seen={} reached={} budget={} coarse={:.3} worst_px={:.2} worst={} plan_age={} plan_ms={:.1} switch_px={:.2} out_of_slots={} out_of_leaves={} cpu_ms={:?} draw_ms={:?} shadow_ms={:?} denoise_ms={:?} generate_ms={:?}",
            self.settled(),
            self.kept_loaded,
            s.kept_chunks,
            s.kept_bytes,
            s.detail_pages.0,
            s.detail_pages.1,
            s.chunk_slots,
            s.leaf_pages.0,
            s.leaf_pages.1,
            s.grow_ms,
            s.out_of_details,
            s.drawn_nodes,
            s.drawn_chunks,
            s.resident_chunks,
            s.evicted_chunks,
            s.evictions_deferred,
            s.fading_nodes,
            s.requests,
            s.generated_footprints,
            s.generated_chunks,
            s.reports,
            s.seen,
            s.reached,
            s.budget,
            s.coarse_share,
            s.worst_px,
            s.worst.map_or("-".to_string(), |w| format!(
                "{}/{}/{}/{}/{}/exact{}/guess{}/kids{}of{}",
                w.key.face,
                w.key.level,
                w.key.cx,
                w.key.cy,
                w.key.cz,
                u8::from(w.exact),
                w.guess,
                w.generated,
                w.children
            )),
            s.plan_age,
            s.plan_ms,
            s.switch_px,
            s.out_of_slots,
            s.out_of_leaves,
            s.cpu_ms,
            gpu.map(|t| t.draw),
            gpu.map(|t| t.shadow),
            gpu.map(|t| t.denoise),
            gpu.map(|t| t.generate),
        )
    }
}

const READS: &[&str] = &[
    "gbuffer",
    "gbuffer_lightmap_uv",
    "gbuffer_sss",
    "gbuffer_extra",
    "gbuffer_velocity",
    helio_mats::MATERIAL_TEXTURES,
    // Shared coast/travel field read by the terrain scene-material swash.
    "water_shore_fields",
    "water_shore_params",
    helio_light_api::TILE_LIGHT_LISTS,
    helio_light_api::TILE_LIGHT_COUNTS,
    helio_gi_api::GI_FRAME,
    helio_gi_api::GI_RAYS,
    helio_gi_api::GI_SUN,
    helio_gi_api::GI_CLIPMAP,
    ATMOSPHERE_FRAME,
];
/// helio-pass-sky's frame of the atmosphere (its sun's disk).
const ATMOSPHERE_FRAME: &str = "atmosphere_frame";
const WRITES: &[&str] = &[
    "gbuffer",
    "gbuffer_lightmap_uv",
    "gbuffer_sss",
    "gbuffer_extra",
    "gbuffer_velocity",
    "directional_visibility",
    "local_visibility",
    // helio-pass-tsr's STOCHASTIC_MASK.
    "tsr_stochastic",
    helio_gi_api::GI_HITS,
    helio_light_api::SUN_OCCLUSION_MAPS,
    helio_light_api::SUN_OCCLUSION_PAGES,
    helio_light_api::SUN_OCCLUSION_FRAME,
];

/// GPU times kept for a host that does not take them.
const KEEP_TIMINGS: usize = 256;

impl RenderPass for ChunkPass {
    fn set_frame_inputs(&mut self, inputs: &helio_core::RenderFrameInputs<'_>) {
        self.xr_mode = inputs.xr_mode;
    }

    fn name(&self) -> &'static str {
        "VoxelChunks"
    }
    fn inherit_persistent_state(&mut self, previous: &mut dyn RenderPass) -> bool {
        let Some(previous) = previous.as_any_mut().downcast_mut::<Self>() else {
            return false;
        };
        self.world = previous.world.take();
        self.debug_mode = previous.debug_mode;
        if let Some(world) = &mut self.world {
            world.set_debug_mode(self.debug_mode);
        }
        self.held = std::mem::take(&mut previous.held);
        self.shown = previous.shown.take();
        self.kept_loaded = previous.kept_loaded.take();
        self.settling = previous.settling;
        if self.scene.is_none() {
            self.scene = previous.scene.take();
        }
        self.steady_shadows |= previous.steady_shadows;
        self.residency_eye = previous.residency_eye;
        self.world.is_some()
    }
    fn reads(&self) -> &'static [&'static str] {
        READS
    }
    fn needs_frame(&self) -> bool {
        self.wants_frames()
    }
    fn diagnostics(&self) -> Option<String> {
        self.shown.as_ref()?;
        Some(self.stats_line())
    }
    fn writes(&self) -> &'static [&'static str] {
        WRITES
    }
    fn declare_resources(&self, builder: &mut helio_core::graph::ResourceBuilder) {
        for name in READS {
            builder.read(name);
        }
    }
    fn render_pass_descriptor<'a>(
        &'a self,
        _: &'a wgpu::TextureView,
        _: &'a wgpu::TextureView,
        _: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }
    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let started = std::time::Instant::now();
        self.gi_eye = Some(helio_gi_api::eye(ctx));
        let result = self.prepare_frame(ctx);
        self.cpu_ms[0] = started.elapsed().as_secs_f64() * 1e3;
        result
    }
    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        if let Some(view) = self.world.as_ref().and_then(ChunkWorld::stochastic_view) {
            frame.write_texture_binding("tsr_stochastic", view, self.name());
        }
        if let Some(view) = self.world.as_ref().and_then(ChunkWorld::sun_view) {
            frame.write_texture_binding("directional_visibility", view, self.name());
        }
        if let Some(buffer) = self.world.as_ref().and_then(ChunkWorld::local_visibility) {
            frame.write_buffer_binding("local_visibility", buffer, self.name());
        }
        if let Some((maps, pages, sun_frame)) =
            self.world.as_ref().and_then(ChunkWorld::sun_occlusion)
        {
            frame.write_texture_binding(helio_light_api::SUN_OCCLUSION_MAPS, maps, self.name());
            frame.write_buffer_binding(helio_light_api::SUN_OCCLUSION_PAGES, pages, self.name());
            frame.write_buffer_binding(
                helio_light_api::SUN_OCCLUSION_FRAME,
                sun_frame,
                self.name(),
            );
        }
    }
    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let started = std::time::Instant::now();
        let result = self.execute_frame(ctx);
        self.cpu_ms[1] = started.elapsed().as_secs_f64() * 1e3;
        result
    }

    fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode;
        if let Some(world) = &mut self.world {
            world.set_debug_mode(mode);
        }
    }

    fn debug_views(&self) -> &'static [DebugViewDescriptor] {
        static VIEWS: &[DebugViewDescriptor] = &[
            DebugViewDescriptor {
                name: "Voxel Trace Cost",
                debug_mode: 32,
                description: "Voxel surfaces shaded by primary-ray traversal work; red is cheaper, yellow and white are more expensive",
            },
            DebugViewDescriptor {
                name: "Voxel Levels",
                debug_mode: 34,
                description: "Voxel surfaces coloured by the octree level drawing them (level 0 finest)",
            },
            DebugViewDescriptor {
                name: "Voxel Levels (Left Half)",
                debug_mode: 35,
                description: "Voxel levels over the left half of the view, shading on the right",
            },
        ];
        VIEWS
    }

    fn debug_view_legend(&self, debug_mode: u32) -> Option<DebugViewLegend> {
        static STOPS: &[DebugViewLegendStop] = &[
            DebugViewLegendStop {
                label: "0",
                color: [0, 0, 0],
            },
            DebugViewLegendStop {
                label: "64",
                color: [255, 0, 0],
            },
            DebugViewLegendStop {
                label: "128",
                color: [255, 255, 0],
            },
            DebugViewLegendStop {
                label: "192+",
                color: [255, 255, 255],
            },
        ];

        (debug_mode == 32).then_some(DebugViewLegend {
            title: "Voxel trace cost",
            unit: "ray steps",
            stops: STOPS,
        })
    }
}

impl ChunkPass {
    fn prepare_frame(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // The last frame was submitted: its results can be mapped.
        if let Some(world) = &mut self.world {
            world.set_plain_colour(false);
            helio_core::cpu_scope!("VoxelChunks: submitted");
            world.submitted();
            self.timings.extend(world.take_timings());
            let excess = self.timings.len().saturating_sub(KEEP_TIMINGS);
            self.timings.drain(..excess);
        }
        self.settling = self.settling.saturating_sub(1);
        // No volumes published: the chunks and storage go.
        let Some(frame) = ctx.scene_buffers.payload(&VOXEL_FRAME) else {
            self.shown = None;
            self.world = None;
            self.held.clear();
            return Ok(());
        };
        let world = self.world.get_or_insert_with(|| {
            let mut world = ChunkWorld::new(ctx.device, ctx.queue);
            world.set_plain_colour(false);
            world.set_debug_mode(self.debug_mode);
            world
        });
        // The frame's volumes: each followed by its edits (any history, in
        // place), placed by its transform; another recipe, the volume added
        // again.
        let mut held = Vec::with_capacity(frame.volumes.len());
        for v in &frame.volumes {
            let planet = &v.volume.planet;
            let recipe = planet.recipe_fingerprint();
            let at = self.held.iter().position(|h| h.id == v.volume.id);
            let synced = at
                .filter(|&n| self.held[n].recipe == recipe)
                .and_then(|n| world.sync_edits(self.held[n].volume, planet));
            if let Some(sync) = synced {
                let mut h = self.held.swap_remove(at.unwrap());
                world.set_transform(h.volume, v.volume.transform);
                let look = (v.appearance, clean_smoothing(v.smoothing));
                if h.look != look {
                    world.set_appearance(
                        h.volume,
                        &shown_appearance(look.0, look.1, || planet.field().appearance()),
                    );
                    h.look = look;
                    self.settling = LOOK_SETTLE_FRAMES;
                }
                // Another history (a level loaded): its saved chunks too.
                if sync == EditSync::Resynced {
                    self.kept_loaded = v
                        .kept
                        .as_ref()
                        .map(|kept| world.load_kept(h.volume, planet, kept));
                }
                held.push(h);
                continue;
            }
            if let Some(n) = at {
                world.remove_volume(self.held.swap_remove(n).volume);
            }
            let volume = world.add_volume(v.volume.id, planet, v.volume.transform);
            let look = (v.appearance, clean_smoothing(v.smoothing));
            if look.0.is_some() || look.1 != 0.0 {
                world.set_appearance(
                    volume,
                    &shown_appearance(look.0, look.1, || planet.field().appearance()),
                );
            }
            self.kept_loaded = v
                .kept
                .as_ref()
                .map(|kept| world.load_kept(volume, planet, kept));
            self.last_eye = [None; 2];
            held.push(HeldVolume {
                id: v.volume.id,
                volume,
                recipe,
                look,
                exporting: None,
                kept: None,
                picked: 0,
            });
        }
        // Volumes the frame no longer shows go.
        for gone in std::mem::replace(&mut self.held, held) {
            world.remove_volume(gone.volume);
        }
        // Foliage tiles the volume the world follows the eye on (its
        // primary; a rebuilt graph's new pass too).
        if let Some(v) = primary_volume(world, &self.held, &frame) {
            self.planet_ground
                .set_volume(*v.volume.planet.grid(), v.volume.transform);
        }
        // A save's keep request: a volume's resident edited chunks packed
        // (over the next updates), then answered for its world.
        for (h, v) in self.held.iter_mut().zip(&frame.volumes) {
            let wanted = v.requests.keep.filter(|id| h.kept != Some(*id));
            let (Some(id), Some(answers)) = (wanted, &v.requests.answers) else {
                h.exporting = None;
                continue;
            };
            if h.exporting != Some(id) {
                world.keep_resident();
                h.exporting = Some(id);
            } else if world.keeping() == 0 {
                answers.push(VolumeAnswer::Kept {
                    id,
                    planet: v.volume.planet.clone(),
                    chunks: world.kept_chunks(h.volume, &v.volume.planet),
                });
                h.kept = Some(id);
                h.exporting = None;
            }
        }
        self.shown = Some(frame);
        Ok(())
    }

    fn execute_frame(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let (Some(world), Some(frame)) = (&mut self.world, self.shown.clone()) else {
            return Ok(());
        };
        let missing = |name: &str| helio_core::Error::ResourceNotFound(name.into());
        let g = ctx
            .registry
            .get::<helio_core::ViewGroup<'_, 4>>(helio_core::ResourceKey::new("gbuffer"))
            .ok_or_else(|| missing("gbuffer"))?;
        macro_rules! view {
            ($name:literal) => {
                ctx.registry
                    .get::<&wgpu::TextureView>(helio_core::ResourceKey::new($name))
                    .ok_or_else(|| missing($name))?
            };
        }
        let gbuffer = [
            g.views[0],
            g.views[1],
            g.views[2],
            g.views[3],
            view!("gbuffer_lightmap_uv"),
            view!("gbuffer_sss"),
            view!("gbuffer_extra"),
            view!("gbuffer_velocity"),
        ];
        let camera = ctx.camera_data;
        let eye_index = if self.xr_mode && camera.jitter_frame[3].to_bits() == 2 {
            1
        } else {
            0
        };
        // The tracer starts every perspective ray at zero. Its matrices must
        // therefore be eye-relative, and its absolute ray origin must be the
        // SAME eye as the renderer camera (including OpenXR's predicted pose
        // and each eye's offset). Residency follows it too, unless the host
        // set a head-centred eye for a stereo pair.
        let at = Vec3::from_slice(&camera.position_near[..3]);
        let (trace_eye, view_proj, prev_view_proj) = trace_camera(camera, ctx.world_origin);
        let proj = Mat4::from_cols_array(&camera.proj);
        let jitter = Vec2::from_slice(&camera.jitter_frame[..2]);
        let jitter = [
            jitter,
            std::mem::replace(&mut self.last_jitter[eye_index], jitter),
        ];
        let now = std::time::Instant::now();
        let residency_eye = self.residency_eye.unwrap_or(trace_eye);
        let interval =
            self.last_eye[eye_index].map(|(_, at)| now.duration_since(at).as_secs_f64().max(1e-3));
        let velocity = match (self.last_eye[eye_index], interval) {
            (Some((eye, _)), Some(dt)) => (residency_eye - eye) / dt,
            _ => DVec3::ZERO,
        };
        self.last_eye[eye_index] = Some((residency_eye, now));
        // In view: the cone around the camera's forward reaching the
        // frustum's corners. A pixel's angle sets the cut's detail.
        let tan_y = 1.0 / f64::from(proj.y_axis.y);
        let view = residency::View {
            forward: glam::Vec3::from_slice(&camera.forward_far[..3])
                .as_dvec3()
                .normalize(),
            cos_half: frustum_cos_half(proj, Vec2::from_slice(&camera.jitter_frame[..2])),
        };
        self.config.plan.pixel_angle = 2.0 * tan_y / f64::from(ctx.height.max(1));
        let near = camera.position_near[3];
        // The scene's lights and their tile lists (light culling's), for
        // the terrain's shadows of them.
        let lights = ctx
            .scene_buffers
            .get(helio_core::BufferKey::of(helio_light_api::LIGHT_LIST))
            .map(|h| h.buffer.clone());
        let tile_lists = ctx
            .registry
            .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(
                helio_light_api::TILE_LIGHT_LISTS,
            ))
            .map(|b| (*b).clone());
        let tile_counts = ctx
            .registry
            .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(
                helio_light_api::TILE_LIGHT_COUNTS,
            ))
            .map(|b| (*b).clone());
        // The GI probes' rays, when a GI pass runs before this one.
        let binding = |name: &str| ctx.registry.buffer_binding(name).cloned();
        let clipmap = ctx
            .registry
            .get::<helio_gi_api::Clipmap>(helio_core::ResourceKey::new(helio_gi_api::GI_CLIPMAP));
        let gi = match (
            binding(helio_gi_api::GI_FRAME),
            binding(helio_gi_api::GI_RAYS),
            binding(helio_gi_api::GI_HITS),
            binding(helio_gi_api::GI_SUN),
            clipmap,
            self.gi_eye,
        ) {
            (Some(frame_gi), Some(rays), Some(hits), Some(sun), Some(clipmap), Some(world_eye)) => {
                world.follow_gi(clipmap, world_eye, trace_eye);
                Some((frame_gi, rays, hits, sun))
            }
            _ => None,
        };
        let mut cmds = ctx.graphics_cmds();
        let encoder = &mut cmds;
        // Generation keeps its pace per second at any frame rate: a frame
        // slower than a 60th of a second generates for longer, up to twice.
        let mut config = self.config;
        config.generate_ms = config
            .generate_ms
            .map(|ms| ms * interval.map_or(1.0, |dt| (dt * 60.0).clamp(1.0, 2.0)));
        // It grows into what the last measured frame left of `frame_ms` on
        // the GPU and the CPU (its own generation counted in), by half a
        // millisecond a frame at most (measurements lag), up to three times
        // that; it falls at once.
        if let (Some(base), Some(frame_ms)) = (config.generate_ms, config.frame_ms) {
            let busy = match (ctx.last_frame_gpu_ms(), ctx.last_frame_cpu_ms()) {
                (Some(g), Some(c)) => Some(f64::from(g.max(c))),
                (g, c) => g.or(c).map(f64::from),
            };
            let room = busy.map_or(base, |busy| {
                (self.timings.last().map_or(0.0, |t| t.generate) + (frame_ms - busy) * 0.8)
                    .clamp(base, 3.0 * base)
            });
            let budget = room.min(self.budget_ms.unwrap_or(base) + 0.5);
            self.budget_ms = Some(budget);
            config.generate_ms = Some(budget);
        }
        self.stats = {
            helio_core::cpu_scope!("VoxelChunks: update");
            world.update(
                encoder,
                residency_eye,
                velocity,
                Some(view),
                ctx.frame_num,
                &config,
            )
        };
        if let Some((map, _)) = &self.scene {
            world.set_material_map(map);
        }
        world.set_steady_shadows(self.steady_shadows);
        // The atmosphere's sun disk sizes the shadows' penumbrae.
        world.set_atmosphere_frame(
            ctx.registry
                .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(ATMOSPHERE_FRAME))
                .map(|b| (*b).clone()),
        );
        // The sun's occlusion by the terrain, for what else the sun lights
        // (fine cells and meshes in this draw, the GI probes' hits,
        // volumetric fog): the pages its readers asked for and lack.
        {
            helio_core::cpu_scope!("VoxelChunks: sun occlusion");
            world.draw_sun_occlusion(encoder, ctx.frame_num, trace_eye, frame.sun);
        }
        helio_core::cpu_scope!("VoxelChunks: draw");
        world.draw_for_eye(
            encoder,
            ctx.frame_num,
            trace_eye,
            view_proj,
            prev_view_proj,
            jitter,
            near,
            frame.sun,
            [ctx.width, ctx.height],
            Some(ctx.depth),
            match (&lights, &tile_lists, &tile_counts) {
                (Some(lights), Some(tile_lists), Some(tile_counts)) => Some(super::SceneLights {
                    lights,
                    tile_lists,
                    tile_counts,
                    eye: at,
                }),
                _ => None,
            }
            .as_ref(),
            gi.as_ref()
                .map(|(frame, rays, hits, sun)| super::GiRays {
                    frame,
                    rays,
                    hits,
                    sun,
                })
                .as_ref(),
            eye_index,
        );
        // Foliage's ground: where the drawn terrain changed, and this frame's
        // questions (`chunks::ground`) answered against the cut just drawn
        // (the primary volume's, and the others' near the eye).
        self.foliage_ground
            .push_changes(world.take_ground_changes());
        if let Some(question) = self.foliage_ground.take_question() {
            let exchange = (
                binding(helio_foliage_api::GROUND_QUERIES),
                binding(helio_foliage_api::GROUND_HITS),
                primary_volume(world, &self.held, &frame).map(|v| &v.volume.planet),
            );
            if let (Some(queries), Some(hits), Some(planet)) = exchange {
                world.probe_ground(
                    encoder,
                    &queries,
                    &hits,
                    &question.tiles,
                    planet,
                    trace_eye,
                    at,
                );
                self.foliage_ground.answer(question.batch);
            }
        }
        let scene_inputs = self.scene.as_ref().and_then(|(_, source)| source(ctx));
        world.publish_scene(encoder, gbuffer, ctx.depth, scene_inputs);
        // Picks along rays from this camera's eye: each volume's, at the
        // view points they cross. Others' are another camera's.
        for (h, v) in self.held.iter_mut().zip(&frame.volumes) {
            let Some(answers) = &v.requests.answers else {
                continue;
            };
            let fresh: Vec<_> = v
                .requests
                .picks
                .iter()
                .filter(|r| r.id > h.picked)
                .collect();
            let mine: Vec<(u64, [f32; 2])> = fresh
                .iter()
                .filter_map(|r| Some((r.id, pick_uv(r, trace_eye, view_proj)?)))
                .collect();
            let taken = world.copy_picks(encoder, &mine, answers);
            // Taken, or not this camera's: done here. Untaken ones wait.
            let first_waiting = mine.get(taken).map(|(id, _)| *id);
            for r in fresh {
                if first_waiting.is_none_or(|w| r.id < w) {
                    h.picked = h.picked.max(r.id);
                }
            }
        }
        Ok(())
    }
}

/// Surface smoothing within 0 to 1.
fn clean_smoothing(smoothing: f32) -> f32 {
    if smoothing.is_finite() {
        smoothing.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Where `request`'s ray crosses this camera's view (0..1 from the top
/// left), when it leaves this camera's eye (`trace_eye`, eye-relative
/// `view_proj`) and is in view.
fn pick_uv(
    request: &crate::engine::PickRequest,
    trace_eye: DVec3,
    view_proj: Mat4,
) -> Option<[f32; 2]> {
    let off = request.origin.distance(trace_eye);
    if !(off <= 0.05 + trace_eye.length() * 1.0e-7) {
        return None;
    }
    let d = request.direction.try_normalize()?.as_vec3();
    let clip = view_proj * d.extend(1.0);
    if !(clip.w > 0.0) {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    let uv = [ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5];
    uv.iter().all(|c| (0.0..=1.0).contains(c)).then_some(uv)
}

/// The frame's volume the world takes as its primary (`ChunkWorld::primary`:
/// the one the eye is nearest the surface of), `held` in the frame's order.
fn primary_volume<'a>(
    world: &ChunkWorld,
    held: &[HeldVolume],
    frame: &'a PlanetFrame,
) -> Option<&'a crate::engine::VolumeFrame> {
    let primary = world.primary()?;
    let at = held.iter().position(|h| h.volume == primary)?;
    frame.volumes.get(at)
}

/// The voxel shader unprojects a direction from zero (`pixel_ray`), unlike
/// mesh projection which accepts geometry relative to any origin. Pair that
/// eye-relative matrix with the active camera's absolute position: the
/// frame's world origin plus the camera's position in the frame.
fn trace_camera(
    camera: &helio_core::GpuCameraUniforms,
    world_origin: DVec3,
) -> (DVec3, Mat4, Mat4) {
    let at = Vec3::from_slice(&camera.position_near[..3]);
    let eye = world_origin + at.as_dvec3();
    let from_eye = Mat4::from_translation(at);
    (
        eye,
        Mat4::from_cols_array(&camera.view_proj) * from_eye,
        // Previous VP is rebased into the CURRENT frame coordinates by Helio.
        Mat4::from_cols_array(&camera.prev_view_proj) * from_eye,
    )
}

/// XR frusta are asymmetric: include the offset on each axis so chunks at the
/// wider edge of either eye are not rejected by the residency cone. The
/// projection's per-frame jitter (an NDC translation, `jitter`) is not part of
/// the frustum: it would change the view every frame, and a view that changes
/// is planned again and never settles.
fn frustum_cos_half(proj: Mat4, jitter: Vec2) -> f64 {
    let proj = Mat4::from_translation(-jitter.extend(0.0)) * proj;
    let tan_x = (1.0 + f64::from(proj.z_axis.x).abs()) / f64::from(proj.x_axis.x).abs();
    let tan_y = (1.0 + f64::from(proj.z_axis.y).abs()) / f64::from(proj.y_axis.y).abs();
    (tan_x * tan_x + tan_y * tan_y).sqrt().atan().cos()
}

#[cfg(test)]
mod stereo_tests {
    use super::*;

    #[test]
    fn voxel_rays_and_mesh_projection_agree_for_moving_stereo_eyes() {
        let origin = DVec3::new(6_371_200.0, -1024.0, 2048.0);
        let old_head = Vec3::new(7.0, 2.0, -3.0);
        // Keep the app's earlier input pose unchanged while rendering newer
        // translated/rotated eyes, as OpenXR's predicted display poses do.
        let landmarks = [
            Vec3::new(8.0, 1.0, -10.0),
            Vec3::new(4.0, 4.0, -12.0),
            Vec3::new(10.0, -1.0, -20.0),
        ];
        for motion in [Vec3::ZERO, Vec3::new(0.3, -0.6, 0.2)] {
            for yaw in [0.0, 0.6, -0.4] {
                let orientation = glam::Quat::from_rotation_y(yaw)
                    * glam::Quat::from_rotation_x(-0.2)
                    * glam::Quat::from_rotation_z(0.15);
                for side in [-1.0, 1.0] {
                    let at = old_head + motion + orientation * Vec3::X * (side * 0.032);
                    let pose = Mat4::from_rotation_translation(orientation, at);
                    let mut proj = helio_core::depth::perspective(1.4, 1.1, 0.05);
                    proj.z_axis.x = side * 0.15;
                    let view = pose.inverse();
                    let camera = helio_core::GpuCameraUniforms::new(
                        view,
                        proj,
                        at,
                        0.05,
                        1000.0,
                        0,
                        [0.0; 2],
                        proj * view,
                    );
                    let (eye, vp, _) = trace_camera(&camera, origin);
                    assert!(eye.distance(origin + at.as_dvec3()) < 1e-9);
                    let rays = super::super::ray_plane(vp, [128, 128]);
                    for point in landmarks {
                        // Rasterized foliage sees this fixed world landmark.
                        let ndc = (proj * view).project_point3(point);
                        let expected = (point - at).normalize();
                        // Mirror chunk_draw.wgsl::pixel_ray: it unprojects a
                        // POINT and normalizes it, assuming the eye is zero.
                        let ray = vp
                            .inverse()
                            .project_point3(Vec3::new(ndc.x, ndc.y, 0.5))
                            .normalize();
                        assert!(
                            ray.distance(expected) < 2e-4,
                            "shader ray drift: {ray:?} != {expected:?}"
                        );
                        let pixel =
                            Vec2::new((ndc.x + 1.0) * 64.0 - 0.5, (1.0 - ndc.y) * 64.0 - 0.5);
                        let plane_ray = (Vec3::from_slice(&rays[0])
                            + Vec3::from_slice(&rays[1]) * pixel.x
                            + Vec3::from_slice(&rays[2]) * pixel.y)
                            .normalize();
                        assert!(plane_ray.distance(expected) < 5e-4, "ray plane drift");
                        let world_point = origin + point.as_dvec3();
                        let distance = world_point.distance(eye);
                        assert!((eye + ray.as_dvec3() * distance).distance(world_point) < 0.005);
                    }
                }
            }
        }
    }

    #[test]
    fn voxel_previous_projection_uses_current_eye_after_origin_rebase() {
        let origin = DVec3::new(6_371_200.0, 0.0, 0.0);
        let old_origin = origin - DVec3::new(128.0, 0.0, 0.0);
        let at = Vec3::new(0.1, 1.4, 0.0);
        let old_at = Vec3::new(127.9, 1.8, 0.1);
        let proj = helio_core::depth::perspective(1.4, 1.1, 0.05);
        let old_vp = proj * Mat4::from_translation(-old_at);
        let rebased_previous = old_vp * Mat4::from_translation((origin - old_origin).as_vec3());
        let camera = helio_core::GpuCameraUniforms::new(
            Mat4::from_translation(-at),
            proj,
            at,
            0.05,
            1000.0,
            0,
            [0.0; 2],
            rebased_previous,
        );
        let (eye, _, previous) = trace_camera(&camera, origin);
        let landmark = origin + DVec3::new(1.0, 2.0, -8.0);
        let expected = old_vp.project_point3((landmark - old_origin).as_vec3());
        let actual = previous.project_point3((landmark - eye).as_vec3());
        assert!(actual.distance(expected) < 1e-5);
    }

    #[test]
    fn residency_cone_contains_all_asymmetric_frustum_corners() {
        for offset in [-0.45, 0.0, 0.45] {
            let mut proj = helio_core::depth::perspective(1.4, 1.1, 0.05);
            proj.z_axis.x = offset;
            proj.z_axis.y = 0.2;
            let inverse = proj.inverse();
            let cos_half = frustum_cos_half(proj, Vec2::ZERO);
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    let ray = inverse.project_point3(Vec3::new(x, y, 1.0)).normalize();
                    assert!(
                        f64::from(-ray.z) + 1e-7 >= cos_half,
                        "corner {ray:?} was culled"
                    );
                }
            }
        }
    }

    #[test]
    fn residency_cone_ignores_projection_jitter() {
        let proj = helio_core::depth::perspective(1.2, 16.0 / 9.0, 0.05);
        let still = frustum_cos_half(proj, Vec2::ZERO);
        for frame in 0..8 {
            let [x, y] = helio_core::temporal::r1_r2_jitter(frame);
            let jitter = Vec2::new(x * 2.0 / 1600.0, y * 2.0 / 900.0);
            let jittered = Mat4::from_translation(jitter.extend(0.0)) * proj;
            assert_eq!(frustum_cos_half(jittered, jitter), still);
        }
    }
}
