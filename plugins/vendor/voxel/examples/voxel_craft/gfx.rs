//! The renderer: device and surface, the scene (sun, atmosphere, block
//! materials, the character), the voxel planet pass wired to the world, and
//! drawing a frame for the player's camera.

use crate::common::{
    directional_light, flush_scene_db, new_scene_db_with_gpu_mirror, scene_db_handle,
    set_camera_postprocess, spawn_light, update_light, SceneRays,
};
use crate::palette;
use crate::player::avatar::Avatar;
use glam::{DMat4, DVec3, Vec3};
use helio::{
    Camera, HelioGraphicsConfig, HelioGraphicsContext, HelioXrInput, Renderer, RendererBuilder,
    RendererConfig,
};
use helio_components::AtmosphereComponent;
use helio_components::FoliageInteractorComponent;
use helio_components::WorldRows;
use helio_presets::Preset;
use helio_pass_voxel_planet::chunks::pass::ChunkPass;
use helio_pass_voxel_planet::chunks::ResidencyConfig;
use helio_pass_voxel_planet::engine::{PlanetFrame, TerrainAppearance, VOXEL_FRAME};
use helio_pass_voxel_planet::Planet;
use pulsar_scenedb::{Entity, SceneDb};
use std::sync::Arc;
use winit::window::Window;

pub struct Gfx {
    #[cfg(not(target_arch = "wasm32"))]
    xr_input: Option<HelioXrInput>,
    xr_active: bool,
    pub failed: bool,
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_usage: wgpu::TextureUsages,
    surface_format: wgpu::TextureFormat,
    present_mode: wgpu::PresentMode,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pub renderer: Renderer,
    pub scene: SceneDb,
    pub avatar: Avatar,
    /// The scene's meshes (the character, what it holds) for ray queries:
    /// the GI probes trace them as they trace the terrain.
    rays: SceneRays,
    /// The player's feet pressing the meadow (a `FoliageInteractorComponent` row).
    feet: Entity,
    /// The blocks' look, published with the world each frame.
    appearance: TerrainAppearance,
    sun_light: Entity,
    /// The renderer's world origin of the last frame.
    pub origin: DVec3,
    size: [u32; 2],
    /// The surface's alpha mode, and the size it was configured at (none
    /// yet: the first frame configures it).
    alpha_mode: wgpu::CompositeAlphaMode,
    configured: Option<[u32; 2]>,
    /// The weather's clock at the last frame (game seconds): the renderer's
    /// frame clock (the clouds' wind) advances with it.
    clock: f64,
    /// `VOXEL_CRAFT_TIMINGS=<frames>`: GPU milliseconds a pass, summed over
    /// that many frames and logged as their mean.
    timings: Option<PassTimings>,
}

/// Per-pass GPU time over a window of frames.
struct PassTimings {
    frames: u32,
    window: u32,
    last: u64,
    total: f64,
    passes: std::collections::BTreeMap<&'static str, f64>,
    /// The voxel pass's stages over the window: frames, then draw (seed and
    /// trace), seed, sun shadows, denoiser, generation (ms summed).
    voxel: (u32, [f64; 12]),
    /// Generation by kernel over the window (`FrameTimings::stages`,
    /// `tile_stages`), summed ms.
    generation: [f64; 7],
    /// Tiles and chunks generated over the window.
    generated: [usize; 2],
    /// The render thread's CPU a frame over the window (`Renderer::render`'s
    /// scope, without the surface's acquire), ms; and its own cycles in the
    /// frame's render call (M: not the time it waited or was descheduled).
    cpu: Vec<f64>,
    cycles: Vec<f64>,
    /// The voxel pass's residency over the window: chunks generated,
    /// evicted, evicted never drawn, level switches; the last frame's.
    residency: [usize; 4],
    last_stats: Option<helio_pass_voxel_planet::chunks::UpdateStats>,
}

impl PassTimings {
    /// Adds the renderer's latest completed GPU frame, and logs the means every
    /// `window` frames.
    fn add(
        &mut self,
        snapshot: &helio::RenderTimingSnapshot,
        voxel: &[helio_pass_voxel_planet::chunks::FrameTimings],
        stats: Option<helio_pass_voxel_planet::chunks::UpdateStats>,
        size: [u32; 2],
    ) {
        if let Some(render) = profiling::collect_events()
            .iter()
            .filter(|e| e.name == "Helio::Renderer::render")
            .max_by_key(|e| e.duration_ns)
        {
            self.cpu.push(render.duration_ns as f64 * 1e-6);
        }
        profiling::clear_events();
        if let Some(s) = stats {
            for (sum, n) in self.residency.iter_mut().zip([
                s.generated_chunks,
                s.evicted_chunks,
                s.evicted_unseen,
                s.switches,
            ]) {
                *sum += n;
            }
            self.last_stats = Some(s);
        }
        for t in voxel {
            self.voxel.0 += 1;
            let [a, b, c, d, e] = t.shadow_stages;
            for (sum, ms) in self.voxel.1.iter_mut().zip([
                t.draw,
                t.seed,
                t.shadow,
                t.denoise,
                t.generate,
                a,
                b,
                c,
                d,
                e,
                t.occlusion,
                t.fallbacks,
            ]) {
                *sum += ms;
            }
            for (sum, ms) in self
                .generation
                .iter_mut()
                .zip(t.stages.iter().chain(&t.tile_stages))
            {
                *sum += ms;
            }
            self.generated[0] += t.tiles;
            self.generated[1] += t.chunks;
        }
        let Some(frame) = snapshot.gpu_frame_index.filter(|f| *f != self.last) else {
            return;
        };
        self.last = frame;
        self.frames += 1;
        self.total += f64::from(snapshot.total_gpu_ms.unwrap_or(0.0));
        for pass in &snapshot.passes {
            *self.passes.entry(pass.name).or_default() += f64::from(pass.gpu_ms.unwrap_or(0.0));
        }
        if self.frames < self.window {
            return;
        }
        let n = f64::from(self.frames);
        let mut passes: Vec<_> = self
            .passes
            .iter()
            .map(|(name, ms)| (*name, ms / n))
            .collect();
        passes.sort_by(|a, b| b.1.total_cmp(&a.1));
        let list: Vec<String> = passes
            .iter()
            .filter(|(_, ms)| *ms >= 0.02)
            .map(|(name, ms)| format!("{name} {ms:.3}"))
            .collect();
        log::info!(
            "[timings] {}x{} over {} frames: gpu {:.3} ms; {}",
            size[0],
            size[1],
            self.frames,
            self.total / n,
            list.join(", ")
        );
        if self.voxel.0 > 0 {
            let [draw, seed, shadow, denoise, generate, sun, sky, hits, local, gi, occlusion, fallbacks] =
                self.voxel.1.map(|ms| ms / f64::from(self.voxel.0));
            log::info!(
                "[timings] voxel stages over {} frames: draw {draw:.3} (seed {seed:.3}), shadow {shadow:.3} (sun samples {sun:.3}, sky samples {sky:.3}, hits {hits:.3} (fallbacks {fallbacks:.3}), local {local:.3}, GI rays {gi:.3}), denoise {denoise:.3}, generate {generate:.3}, sun occlusion {occlusion:.3}",
                self.voxel.0
            );
        }
        if self.voxel.0 > 0 {
            let [tiles, columns, builds, nodes, tile_columns, halos, features] =
                self.generation.map(|ms| ms / f64::from(self.voxel.0));
            let per = |n: usize| n as f64 / f64::from(self.voxel.0);
            log::info!(
                "[timings] generation: tiles {tiles:.3} (nodes {nodes:.3}, columns {tile_columns:.3}, halos {halos:.3}, features {features:.3}), chunk columns {columns:.3}, chunk builds {builds:.3}; a frame {:.1} tiles, {:.1} chunks",
                per(self.generated[0]),
                per(self.generated[1])
            );
        }
        self.generation = [0.0; 7];
        self.generated = [0; 2];
        if let Some(s) = self.last_stats {
            let [generated, evicted, unseen, switches] = self.residency;
            log::info!(
                "[timings] residency over the window: generated {generated} chunks, evicted {evicted} ({unseen} never drawn), switches {switches}; now requests {}, resident {}, drawn {}, fading {}",
                s.requests,
                s.resident_chunks,
                s.drawn_chunks,
                s.fading_nodes
            );
        }
        if !self.cycles.is_empty() {
            let mut cycles = std::mem::take(&mut self.cycles);
            cycles.sort_by(f64::total_cmp);
            let at = |p: f64| cycles[((cycles.len() - 1) as f64 * p).round() as usize];
            log::info!(
                "[timings] render cycles over {} frames (M): mean {:.3} p50 {:.3} p95 {:.3} max {:.3}",
                cycles.len(),
                cycles.iter().sum::<f64>() / cycles.len() as f64,
                at(0.5),
                at(0.95),
                at(1.0)
            );
        }
        if !self.cpu.is_empty() {
            let mut cpu = std::mem::take(&mut self.cpu);
            cpu.sort_by(f64::total_cmp);
            let at = |p: f64| cpu[((cpu.len() - 1) as f64 * p).round() as usize];
            log::info!(
                "[timings] render cpu over {} frames: mean {:.3} p50 {:.3} p95 {:.3} max {:.3} ms",
                cpu.len(),
                cpu.iter().sum::<f64>() / cpu.len() as f64,
                at(0.5),
                at(0.95),
                at(1.0)
            );
        }
        self.residency = [0; 4];
        self.voxel = (0, [0.0; 12]);
        self.frames = 0;
        self.total = 0.0;
        self.passes.clear();
    }
}

/// The calling thread's CPU cycles so far (Windows `QueryThreadCycleTime`:
/// the cycles it ran, not the time it waited or was descheduled by other
/// processes); 0 elsewhere.
fn thread_cycles() -> u64 {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> isize;
            fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
        }
        let mut cycles = 0u64;
        // SAFETY: the pseudo-handle of the calling thread, and a u64 out.
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
        cycles
    }
    #[cfg(not(windows))]
    0
}

/// Residency settings for chunk trees at a window of height `h`.
fn chunk_config(h: u32) -> ResidencyConfig {
    let number = |name: &str, default: f64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let mut config = ResidencyConfig::default();
    config.plan.pixel_angle = 2.0 * std::f64::consts::FRAC_PI_8.tan() / f64::from(h);
    // Cells a pixel wide at most: one-block detail wherever it shows.
    config.plan.lod_pixels = number("VOXEL_CRAFT_LOD", 0.6);
    // Far enough for a real horizon.
    config.plan.max_distance = number("VOXEL_CRAFT_DISTANCE", 12_000.0);
    config
}

/// The sun along `direction` (from it), of `color`: it lights the haze too,
/// shadowed by everything (shafts).
fn sun_light(direction: [f32; 3], color: [f32; 3]) -> helio::GpuLight {
    let mut light = directional_light(direction, color, 3.0);
    light.god_rays_enabled = 1;
    light.god_rays_density = 1.0;
    light.god_rays_weight = 1.0;
    light.god_rays_exposure = 1.0;
    // Fully shadowed in the haze.
    light.god_rays_decay = 1.0;
    light
}

/// TSR's quality (`fxaa`: no TSR, FXAA at native resolution, for A/B
/// measurements of what is temporal).
fn tsr_quality() -> Option<helio_pass_tsr::TsrQuality> {
    match std::env::var("VOXEL_CRAFT_QUALITY").as_deref() {
        Ok("performance") => Some(helio_pass_tsr::TsrQuality::Performance),
        Ok("quality") => Some(helio_pass_tsr::TsrQuality::Quality),
        Ok("native") => Some(helio_pass_tsr::TsrQuality::Native),
        Ok("fxaa") => None,
        _ => Some(helio_pass_tsr::TsrQuality::Balanced),
    }
}

/// VR renders two eye views every frame. Start at half linear resolution and
/// reconstruct to the runtime's recommended per-eye size; users can opt into
/// balanced, quality, or native rendering through `VOXEL_CRAFT_VR_QUALITY`.
fn vr_tsr_quality() -> Option<helio_pass_tsr::TsrQuality> {
    match std::env::var("VOXEL_CRAFT_VR_QUALITY").as_deref() {
        Ok("native" | "fxaa") => None,
        Ok("balanced") => Some(helio_pass_tsr::TsrQuality::Balanced),
        Ok("quality") => Some(helio_pass_tsr::TsrQuality::Quality),
        _ => Some(helio_pass_tsr::TsrQuality::Performance),
    }
}

impl Gfx {
    pub fn new(window: Arc<Window>, planet: &Planet) -> Self {
        let size = window.inner_size();
        let size = [size.width.max(1), size.height.max(1)];
        let mut graphics = pollster::block_on(HelioGraphicsContext::new(
            window.clone(),
            size[0],
            size[1],
            HelioGraphicsConfig {
                vr_support: std::env::var_os("VOXEL_CRAFT_SHOTS").is_none()
                    && !matches!(
                        std::env::var("VOXEL_CRAFT_VR").as_deref(),
                        Ok("off" | "0" | "false")
                    ),
                application_name: "Voxel Craft".into(),
                present_mode: wgpu::PresentMode::AutoNoVsync,
                surface_usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC,
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let surface_config = graphics.surface_config.clone();
        let (device, queue) = (graphics.device.clone(), graphics.queue.clone());
        let render_target = graphics.render_target();
        let render_size = [render_target.0, render_target.1];
        let render_format = render_target.2;
        let vr = graphics.is_xr();

        // The scene: SceneDB rows for the sun, the air, the blocks' materials
        // and the character.
        let mut scene = new_scene_db_with_gpu_mirror(&device, &queue);
        let palette = palette::load(&mut scene.world);
        // Grass and flowers on the world's grass: the engine's voxel meadow, with stands
        // of tall grass in places and a fresh breeze through it.
        let meadow = helio_pass_voxel_planet::meadow::meadow()
            .with_tall_stands()
            .with_wind([1.0, 0.35], 4.5, 0.8);
        for row in meadow.types {
            let entity = scene.world.spawn();
            scene.world.insert_row(entity, row);
        }
        let wind = scene.world.spawn();
        scene.world.insert_row(wind, meadow.wind);
        // The player's feet push it aside, and leave a track that rises behind them.
        let feet = scene.world.spawn();
        scene.world.insert(
            feet,
            FoliageInteractorComponent::from(helio_pass_foliage_place::FoliageInteractorRow::new(
                DVec3::ZERO,
                0.7,
                1.0,
                Vec3::ZERO,
                1.2,
            )),
        );
        let sun_light = spawn_light(
            &mut scene.world,
            sun_light([-0.3, -0.8, -0.4], [1.0, 0.96, 0.9]),
        );
        let air = scene.world.spawn();
        // Fair-weather cumulus, sized for the small planet: a layer from 0.9
        // to 2.2 km over the ground, drifting with a light breeze
        // (`VOXEL_CRAFT_NO_CLOUDS`: a clear sky).
        let clouds = helio_pass_sky::CloudLayer {
            bottom_km: 0.9,
            top_km: 2.2,
            coverage: 0.45,
            weather_km: 7.0,
            shape_km: 1.8,
            detail_km: 0.3,
            wind: [6.0, 0.0, 2.5],
            ..helio_pass_sky::CloudLayer::cumulus()
        };
        let sky = AtmosphereComponent::from(
            helio_pass_sky::AtmosphereRow::earth().around_planet([0.0; 3], planet.grid().radius()),
        );
        let sky = if std::env::var_os("VOXEL_CRAFT_NO_CLOUDS").is_some() {
            sky
        } else {
            sky.with_clouds(clouds).into()
        };
        scene.world.insert(air, sky);
        // A light ground haze, thinning with altitude over the planet, the
        // sun scatters forward through: shafts past trees and ridges (the
        // terrain's sun occlusion shadows it).
        let haze = scene.world.spawn();
        scene.world.insert(
            haze,
            helio_components::GlobalFogComponent {
                mode: 1,
                extinction: 0.001,
                height_falloff: 0.01,
                anisotropy: 0.6,
                ..Default::default()
            }
            .around_planet([0.0; 3], planet.grid().radius()),
        );
        let mut post = helio_pass_postprocess::PostProcessSettings::default();
        // AgX with its punchy look: sunlit leaves and grass roll toward
        // white, not lime, and the sky stays blue.
        post.tonemap_operator = helio::TonemapOperator::AgxPunchy;
        post.tonemap_exposure = 2.0;
        post.color_contrast = [1.06; 3];
        post.exposure_mode = helio_pass_postprocess::ExposureMode::Manual;
        set_camera_postprocess(&mut scene.world, 0, &post);
        let avatar = Avatar::spawn(&mut scene.world, |block| palette.hand_material(block));
        scene.world.flush_gpu_mirror(&queue);

        // The planet pass draws the world's latest snapshot, published each
        // frame (`VOXEL_FRAME`).
        let appearance = palette.appearance;
        let terrain = helio_pass_voxel_planet::VoxelSettings {
            residency: chunk_config(render_size[1]),
        };
        let renderer_config = RendererConfig::new(render_size[0], render_size[1], render_format);
        // Render VR eyes at Performance scale by default, then reconstruct to
        // the headset's native swapchain dimensions. This cuts pixel shading
        // by 75% across both eyes while keeping the compositor target unchanged.
        let quality = if vr { vr_tsr_quality() } else { tsr_quality() };
        let mut renderer_config = match quality {
            Some(quality) => renderer_config.with_tsr_quality(quality),
            None => renderer_config.without_tsr().with_render_scale(1.0),
        };
        // The meadow, grown on the terrain the planet pass draws
        // (`VOXEL_CRAFT_NO_FOLIAGE`: bare turf, for A/B measurements).
        renderer_config.features.edit::<helio_pass_foliage_place::FoliageSettings>().enabled = std::env::var_os("VOXEL_CRAFT_NO_FOLIAGE").is_none();
        renderer_config.features.edit::<helio_pass_foliage_place::FoliageSettings>().tufts_per_m2 = Some(meadow.tufts_per_m2);
        renderer_config.features.edit::<helio_pass_foliage_place::FoliageSettings>().blades_per_tuft = Some(meadow.blades_per_tuft);
        renderer_config.features.edit::<helio_pass_foliage_place::FoliageSettings>().range = Some(meadow.range);
        let renderer_config = renderer_config.with_feature(terrain);
        let handle = scene_db_handle(&scene);
        let builder =
            RendererBuilder::new(renderer_config.clone(), handle).with_graph(
                // The voxel world, the camera lens and its flare are features
            // Helio's presets do not list (the voxel and lens plugins').
                Preset::Deferred.with_features(
                    &helio_feature_api::FeatureSet::new()
                        .with(helio_pass_voxel_planet::VoxelFeature)
                        .with(helio_pass_lens::LensFeature)
                        .with(helio_pass_flare::LensFlareFeature),
                ),
            );
        let (mut renderer, xr_input) = graphics
            .build_renderer(builder, renderer_config)
            .expect("Helio renderer");
        let surface = graphics.surface;
        let surface_usage = surface_config.usage;
        let surface_format = surface_config.format;
        let alpha_mode = surface_config.alpha_mode;
        let present_mode = surface_config.present_mode;
        let xr_input = xr_input;
        if vr {
            renderer.set_jitter_enabled(false);
        }
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.45, 0.58, 0.8],
            intensity: 0.35,
        });
        // `VOXEL_CRAFT_DEBUG=<mode>`: one of the renderer's debug views.
        if let Some(mode) = std::env::var("VOXEL_CRAFT_DEBUG")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            renderer.set_debug_mode(mode);
        }
        if std::env::var_os("VOXEL_CRAFT_TIMINGS").is_some() {
            profiling::enable_profiling();
        }
        let timings = std::env::var("VOXEL_CRAFT_TIMINGS")
            .ok()
            .and_then(|v| v.parse().ok())
            .map(|window: u32| PassTimings {
                frames: 0,
                window: window.max(1),
                last: u64::MAX,
                total: 0.0,
                passes: Default::default(),
                voxel: (0, [0.0; 12]),
                cpu: Vec::new(),
                cycles: Vec::new(),
                residency: [0; 4],
                last_stats: None,
                generation: [0.0; 7],
                generated: [0; 2],
            });
        let rays = SceneRays::new(&device, &queue);
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            xr_input,
            xr_active: vr,
            failed: false,
            window,
            surface,
            surface_usage,
            surface_format,
            present_mode,
            device,
            queue,
            renderer,
            scene,
            avatar,
            rays,
            feet,
            appearance,
            sun_light,
            origin: DVec3::ZERO,
            size,
            alpha_mode,
            configured: Some([surface_config.width, surface_config.height]),
            clock: 0.0,
            timings,
        }
    }

    pub fn vr_active(&self) -> bool {
        self.xr_active
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn xr_frame_input(&mut self) -> helio_xr::Result<Option<crate::xr::rig::FrameInput>> {
        let Some(input) = &mut self.xr_input else {
            return Ok(None);
        };
        let state = input.frame_state(&self.renderer, &glam::Mat4::IDENTITY)?;
        let Some(tracking) = state.tracking else {
            return Ok(None);
        };
        let Some(head) = tracking.head else {
            return Ok(None);
        };
        let controls = state.controllers;
        Ok(Some(crate::xr::rig::FrameInput {
            focused: state.focused,
            head,
            grips: tracking.grips,
            aim: tracking.aims[1],
            movement: controls.left_stick,
            turn: controls.right_stick,
            buttons: crate::xr::rig::Buttons {
                mine: controls.trigger[1],
                place: controls.trigger[0],
                jump: controls.primary[1],
                pick: controls.primary[0],
                fly: controls.secondary[1],
                recenter: controls.secondary[0],
                sneak: controls.squeeze[0],
                sprint: controls.squeeze[1],
            },
        }))
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.configure([width, height]);
        }
    }

    /// The surface and the renderer at `size` pixels, unless already.
    fn configure(&mut self, size: [u32; 2]) {
        if self.configured == Some(size) {
            return;
        }
        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: self.surface_usage,
                format: self.surface_format,
                width: size[0],
                height: size[1],
                present_mode: self.present_mode,
                alpha_mode: self.alpha_mode,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
                color_space: wgpu::SurfaceColorSpace::Auto,
            },
        );
        if size != self.size && !self.vr_active() {
            self.renderer.set_render_size(size[0], size[1]);
        }
        self.size = size;
        self.configured = Some(size);
    }

    /// Whether the planet pass is still generating terrain for this view.
    pub fn settled(&self) -> bool {
        self.renderer
            .find_pass::<ChunkPass>()
            .is_none_or(ChunkPass::settled)
    }

    /// Draws a frame of `view`.
    pub fn draw(&mut self, view: &View) {
        let (planet, eye, forward) = (view.planet, view.eye, view.look);
        // The planet pass draws from the camera's eye and streams around the
        // residency eye. In VR both must be the tracked head position, not
        // the player's body eye: otherwise the camera follows vertical
        // room-scale head motion while the voxel pass's geometry origin stays
        // fixed, making the world appear to move toward the player. Keep
        // `eye` for gameplay/avatar placement and use `render_eye` only for
        // rendering and residency.
        #[cfg(not(target_arch = "wasm32"))]
        let render_eye = view.vr.map(|vr| vr.head.w_axis.truncate()).unwrap_or(eye);
        #[cfg(target_arch = "wasm32")]
        let render_eye = eye;
        let sun = view.sun.normalize();
        let mut frame = PlanetFrame::single(planet.clone(), sun.as_vec3());
        frame.volumes[0].appearance = Some(self.appearance);
        self.renderer
            .set_scene_payload(&VOXEL_FRAME, Arc::new(frame));
        // The sun's light follows the sky.
        update_light(
            &mut self.scene.world,
            self.sun_light,
            sun_light(
                (-sun).as_vec3().to_array(),
                // Warm at the horizon, white overhead.
                {
                    let up = render_eye.normalize();
                    let height = sun.dot(up).clamp(-0.2, 1.0) as f32;
                    let warm = (1.0 - height * 2.2).clamp(0.0, 1.0);
                    [1.0, 0.97 - 0.25 * warm, 0.92 - 0.5 * warm]
                },
            ),
        );
        let weather_delta = (view.weather - self.clock).clamp(0.0, 0.1) as f32;
        self.renderer.set_frame_clock_delta(Some(weather_delta));
        self.clock = view.weather;
        self.origin = self.renderer.origin_for(render_eye);
        let origin = self.origin;
        // The character and what it holds, and the targeted block's outline.
        #[cfg(not(target_arch = "wasm32"))]
        let hand = view
            .vr
            .map(|vr| vr.grips[1].map(|pose| (DMat4::from_translation(-origin) * pose).as_mat4()));
        #[cfg(target_arch = "wasm32")]
        let hand = None;
        self.avatar.pose(
            &mut self.scene.world,
            &mut self.renderer,
            view.player,
            (eye, forward),
            origin,
            view.time,
            hand,
        );
        if let Some(mut feet) = self
            .scene
            .world
            .get_mut::<FoliageInteractorComponent>(self.feet)
        {
            feet.set_motion(view.player.position, view.player.velocity.as_vec3());
        }
        self.renderer.debug_clear();
        for (a, b) in view.outline {
            self.renderer.debug_line(*a, *b, [0.05, 0.05, 0.05, 1.0]);
        }
        // A crosshair fixed in front of the eye.
        if !view.player.third_person && !self.vr_active() {
            let look = forward.normalize();
            let right = look.cross(eye.normalize()).normalize_or_zero();
            let above = right.cross(look);
            let centre = eye + look * 0.6;
            let reach = 0.0045;
            for (a, b) in [(-right, right), (-above, above)] {
                self.renderer.debug_line(
                    centre + a * reach,
                    centre + b * reach,
                    [1.0, 1.0, 1.0, 0.9],
                );
            }
        }

        // The window's size now (the first frame configures the surface).
        let now = self.window.inner_size();
        if now.width > 0 && now.height > 0 {
            self.configure([now.width, now.height]);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(vr) = view.vr {
            let (start, direction) = vr.aim;
            let end = view
                .target
                .unwrap_or(start + direction * crate::config::REACH);
            self.renderer.debug_line(start, end, [0.3, 0.85, 1.0, 0.8]);
            for grip in vr.grips.iter().flatten() {
                let a = grip.transform_point3(DVec3::new(0.0, 0.0, 0.04));
                let b = grip.transform_point3(DVec3::new(0.0, 0.0, -0.12));
                self.renderer.debug_line(a, b, [0.9, 0.9, 0.95, 1.0]);
            }
            if let Some(input) = &self.xr_input {
                let world_from_stage =
                    (DMat4::from_translation(-origin) * vr.world_from_stage).as_mat4();
                input.set_locomotion_transform(&mut self.renderer, world_from_stage);
            }
        }
        if self.vr_active() {
            if let Some(pass) = self.renderer.find_pass_mut::<ChunkPass>() {
                pass.set_residency_eye(Some(render_eye));
            }
        }
        let up = eye.normalize().as_vec3();
        let forward = forward.normalize().as_vec3();
        let up = if forward.dot(up).abs() > 0.999 {
            up.any_orthonormal_vector()
        } else {
            up
        };
        let near = (planet.air_clearance(render_eye) * 0.25).clamp(0.05, 50_000.0) as f32;
        let aspect = self.size[0] as f32 / self.size[1] as f32;
        let camera = Camera::perspective_look_to(
            render_eye,
            forward,
            up,
            70f32.to_radians(),
            aspect,
            near,
            40_000_000.0,
        );

        // Let Helio own the ordinary window frame and its XR mirror. Screenshot
        // capture is the sole path that needs the acquired surface texture.
        let output = if view.screenshot.is_some() {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(texture)
                | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
                _ => return,
            }
        } else {
            None
        };
        flush_scene_db(&self.scene, &self.queue);
        self.rays.publish(&self.scene.world, &mut self.renderer);
        let cycles = thread_cycles();
        if let Some(output) = &output {
            let target = output.texture.create_view(&Default::default());
            if let Err(error) = self.renderer.render(&camera, &target) {
                log::error!("render: {error:?}");
            }
        } else if let Err(error) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("render: {error:?}");
        }
        let cycles = thread_cycles() - cycles;
        if let Some(timings) = &mut self.timings {
            timings.cycles.push(cycles as f64 * 1e-6);
            let voxel = self
                .renderer
                .find_pass_mut::<ChunkPass>()
                .map(ChunkPass::take_timings)
                .unwrap_or_default();
            let stats = self
                .renderer
                .find_pass::<ChunkPass>()
                .map(|pass| *pass.stats());
            timings.add(self.renderer.timing_snapshot(), &voxel, stats, self.size);
        }
        if let (Some(path), Some(output)) = (view.screenshot, output.as_ref()) {
            self.save_screenshot(&output.texture, path);
        }
        if let Some(output) = output {
            self.queue.present(output);
        }
    }

    /// Writes the frame just drawn to `path` (PNG).
    fn save_screenshot(&self, texture: &wgpu::Texture, path: &str) {
        let [w, h] = self.size;
        let pixels = self.read_pixels(texture);
        let _ = image::save_buffer(path, &pixels, w, h, image::ColorType::Rgba8);
    }

    /// The frame just drawn as RGBA8 rows.
    fn read_pixels(&self, texture: &wgpu::Texture) -> Vec<u8> {
        let [w, h] = self.size;
        let row = (w * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        let bgra = matches!(
            self.surface_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for line in data.chunks(row as usize).take(h as usize) {
            for p in line[..(w * 4) as usize].chunks(4) {
                pixels.extend_from_slice(&if bgra {
                    [p[2], p[1], p[0], 255]
                } else {
                    [p[0], p[1], p[2], 255]
                });
            }
        }
        drop(data);
        pixels
    }
}

/// What a frame shows.
pub struct View<'a> {
    #[cfg(not(target_arch = "wasm32"))]
    pub vr: Option<&'a crate::xr::rig::RigView>,
    pub target: Option<DVec3>,
    pub dt: f32,
    pub planet: &'a Arc<Planet>,
    pub player: &'a crate::player::Player,
    /// The camera: position and look direction (planet-centred).
    pub eye: DVec3,
    pub look: DVec3,
    /// Direction to the sun.
    pub sun: DVec3,
    /// Edges to outline (the targeted block), planet-centred.
    pub outline: &'a [(DVec3, DVec3)],
    /// Seconds since the game started (the character's animation).
    pub time: f64,
    /// Game seconds (they hold still in the tour and race with `T`): the
    /// clouds drift with them.
    pub weather: f64,
    /// Save this frame to a PNG at this path.
    pub screenshot: Option<&'a str>,
}
