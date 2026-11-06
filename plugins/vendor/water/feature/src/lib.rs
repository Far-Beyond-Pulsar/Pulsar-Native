pub mod feature;
#[cfg(feature = "host-rows")]
pub mod host_rows;
pub mod scene_rows;
pub use feature::WaterFeature;
pub use scene_rows::WaterRowSources;

pub mod pipeline;
pub mod simulation;

use bytemuck::Zeroable;
use helio_core::graph::{ResourceBuilder, ResourceFormat, ResourceSize};
use helio_core::{CommandRecorder, PassContext, PrepareContext, RenderPass, Result as HelioResult};
use std::f32::consts::PI;
use wgpu::util::DeviceExt;

pub mod components;
pub mod gpu_types;
pub mod ocean_fft;
mod reflection_capture;
pub mod shoreline;
pub mod surface_query;
pub mod reactivity;
pub mod transmission;
pub use components::WATER_HITBOXES;
pub use reactivity::WaterReactivityPass;
pub use components::WATER_VOLUMES;
pub use components::{WaterHitboxRow, WaterVolumeRow};
pub use gpu_types::*;
pub use ocean_fft::{OceanSpectrum, CASCADE_LENGTHS as OCEAN_CASCADE_LENGTHS};
pub use shoreline::{WaterShoreMap, WaterTerrainProvider, SHORE_MAP_SIZE};
pub use surface_query::{
    WaterSurfaceQueries, WaterSurfaceQueryConsumer, WaterSurfaceQueryPoint,
    WaterSurfaceQueryResult, MAX_WATER_SURFACE_QUERIES,
};
pub use transmission::WaterTransmissionPass;

use pulsar_scenedb::gpu::BufferKey;

/// Simple fullscreen blit: copies a texture to the render target as-is.
const BLIT_WGSL: &str = "
@group(0) @binding(0) var blit_tex:  texture_2d<f32>;
@group(0) @binding(1) var blit_samp: sampler;
struct V { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@builtin(vertex_index) vi: u32) -> V {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return V(vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0), vec2<f32>(x, y));
}
@fragment fn fs(in: V) -> @location(0) vec4<f32> {
    return textureSample(blit_tex, blit_samp, in.uv);
}
";

const SIM_SIZE: u32 = 256;
const CAUSTICS_SIZE: u32 = 512;
const CAUSTICS_DEEP_SIZE: u32 = 256;
const CAUSTICS_NEAR_MIPS: u32 = 10;
const CAUSTICS_DEEP_MIPS: u32 = 9;
const FOAM_TEXTURE_SIZE: u32 = 512;
const FOAM_MIP_LEVELS: u32 = 10;
const MAX_DROPS_BUFFERED: usize = 16;
const MAX_WATER_SPRAY_IMPACTS: usize = 16;
const MAX_WATER_SPRAY_IMPACT_AGE: f32 = 1.6;
const MAX_WATER_SPRAY_EMITTERS: u32 = 150_000;
const WATER_SPRAY_EMITTER_STRIDE: u64 = 16;
pub(crate) const CASCADE_COUNT: usize = 3;
pub const MAX_SIM_VOLUMES: u32 = 8;
/// Fixed capacity for the `"water_hitboxes"` SceneDB buffer, mirroring
/// `MAX_SIM_VOLUMES` above: this pass always reads exactly this many rows
/// (zeroed/absent hitboxes are inert -- a zero strength displaces
/// nothing), so no per-frame CPU count of live hitboxes is ever needed to
/// drive this pass's dispatch/draw bounds.
pub const MAX_WATER_HITBOXES: u32 = 32;
pub(crate) const CASCADE_PATCH_SIZES: [f32; 3] = [30.0, 90.0, 270.0];
/// Legacy interaction/caustics phase period. Ultra-Water's FFT surface uses
/// independent f64-reduced phases for each patch instead of this shared period.
pub const WATER_WORLD_PERIOD: f64 = 270.0;
/// One simulation step, in seconds of the renderer's frame clock: the
/// simulation steps at a fixed 120 Hz, so a 60 Hz frame takes two steps and
/// a frame whose clock did not advance (a paused game, a viewport whose
/// Realtime is off) takes none.
pub(crate) const SIM_STEP_SECONDS: f32 = 1.0 / 120.0;
/// Steps one frame takes at most, so a long frame does not stall the next.
pub(crate) const MAX_SIM_STEPS_PER_FRAME: u32 = 8;

// ---- Mesh helpers ----------------------------------------------------------------

/// Tessellated source-water grid for one area-ratio caustics splat. The grid
/// extends beyond the 12 m tile by `margin` source cells so refracted triangles
/// from outside the tile can land inside its edges without drawing 3x3 tiles.
pub fn make_caustics_grid(
    device: &wgpu::Device,
    cells: u32,
    margin: u32,
) -> (wgpu::Buffer, wgpu::Buffer, u32) {
    let span = cells + margin * 2;
    let n = span + 1;
    let total_verts = (n * n) as usize;
    let total_indices = (span * span * 6) as usize;

    let mut verts: Vec<[f32; 4]> = Vec::with_capacity(total_verts);
    let mut indices: Vec<u32> = Vec::with_capacity(total_indices);

    for j in 0..n {
        for i in 0..n {
            let x = i as f32 - span as f32 * 0.5;
            let y = j as f32 - span as f32 * 0.5;
            verts.push([x, y, 0.0, 0.0]);
        }
    }
    for j in 0..span {
        for i in 0..span {
            let tl = j * n + i;
            let tr = j * n + (i + 1);
            let bl = (j + 1) * n + i;
            let br = (j + 1) * n + (i + 1);
            indices.extend_from_slice(&[tl, bl, tr, tr, bl, br]);
        }
    }

    let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Water Caustics VB"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Water Caustics IB"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    (vbuf, ibuf, indices.len() as u32)
}

/// Static camera-centred polar grid. Rings grow geometrically so their cells
/// stay close to square; the last ring is a flat far-field skirt. Vertex z
/// carries the local cell diameter for per-cascade FFT band limiting.
fn make_top_grid(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer, u32) {
    const ANGULAR: u32 = 384;
    const INNER: f32 = 1.0;
    const OUTER: f32 = 3_000.0;
    const SKIRT: f32 = 60_000.0;

    let ratio = 1.0 + (2.0 * PI) / ANGULAR as f32;
    let mut radii = Vec::new();
    let mut r = INNER / 16.0;
    while r < INNER * 0.99 {
        radii.push(r);
        r *= 1.12;
    }
    r = INNER;
    while r < OUTER {
        radii.push(r);
        r *= ratio;
    }
    radii.push(SKIRT);

    let mut verts = Vec::<[f32; 4]>::with_capacity(1 + radii.len() * ANGULAR as usize);
    verts.push([0.0, 0.0, INNER / 16.0, 0.0]);
    let mut previous_radius = 0.0;
    for radius in &radii {
        let radial_step = *radius - previous_radius;
        let cell = radial_step.max(*radius * (2.0 * PI / ANGULAR as f32));
        for a in 0..ANGULAR {
            let angle = a as f32 * (2.0 * PI / ANGULAR as f32);
            verts.push([radius * angle.cos(), radius * angle.sin(), cell, 0.0]);
        }
        previous_radius = *radius;
    }

    let mut indices =
        Vec::<u32>::with_capacity((radii.len() * ANGULAR as usize * 6) + (ANGULAR as usize * 3));
    for a in 0..ANGULAR {
        indices.extend_from_slice(&[0, 1 + (a + 1) % ANGULAR, 1 + a]);
    }
    for ring in 0..radii.len().saturating_sub(1) {
        let inner = 1 + ring as u32 * ANGULAR;
        let outer = inner + ANGULAR;
        for a in 0..ANGULAR {
            let next = (a + 1) % ANGULAR;
            indices.extend_from_slice(&[
                inner + a,
                inner + next,
                outer + a,
                inner + next,
                outer + next,
                outer + a,
            ]);
        }
    }
    let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Water Top Grid VB"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Water Top Grid IB"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    (vbuf, ibuf, indices.len() as u32)
}

fn vec4_vbl() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 16,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 0,
            shader_location: 0,
        }],
    }
}

// ---- Pass struct ----------------------------------------------------------------

pub struct WaterSimPass {
    /// Ultra-Water's four-cascade spectral ocean. Its output is published for
    /// the surface stage and remains independent of volume-local hitbox data.
    pub(crate) ocean_fft: ocean_fft::OceanFft,
    /// Sparse asynchronous queries sample this pass's current rendered
    /// spectrum, shoreline and localized interaction fields.
    pub(crate) surface_queries: WaterSurfaceQueries,
    /// Secondary mirrored opaque view, rendered before the surface shade and
    /// retained across frames so its HDR target and graph survive resize.
    pub(crate) reflection_capture: Option<reflection_capture::PlanarReflectionCapture>,
    /// Absolute-world rest plane for planar capture, in sync with the authored
    /// water volume's `bounds_max.w` value.
    pub(crate) ocean_rest_height: f64,
    /// Explicit terrain-derived shore inputs; absent by default because generic
    /// Helio scenes do not expose a terrain height query to render passes.
    pub(crate) shore_map: Option<WaterShoreMap>,
    pub(crate) shore_map_dirty: bool,
    pub(crate) _shore_texture: wgpu::Texture,
    pub(crate) shore_view: wgpu::TextureView,
    pub(crate) shore_params: wgpu::Buffer,
    pub(crate) sim_bgl: wgpu::BindGroupLayout,
    pub(crate) update_bgl: wgpu::BindGroupLayout,
    pub(crate) hitbox_bgl: wgpu::BindGroupLayout,

    pub(crate) drop_pipeline: wgpu::RenderPipeline,
    pub(crate) update_pipeline: wgpu::RenderPipeline,
    pub(crate) normal_pipeline: wgpu::RenderPipeline,
    pub(crate) hitbox_pipeline: wgpu::RenderPipeline,

    pub(crate) sim_tex_a: wgpu::Texture,
    pub(crate) sim_tex_b: wgpu::Texture,
    pub(crate) sim_array_view_a: wgpu::TextureView,
    pub(crate) sim_array_view_b: wgpu::TextureView,
    pub(crate) sim_layer_views_a: Vec<wgpu::TextureView>,
    pub(crate) sim_layer_views_b: Vec<wgpu::TextureView>,
    pub(crate) front_per_layer: Vec<bool>,

    pub(crate) sampler: wgpu::Sampler,
    pub(crate) output_sampler: wgpu::Sampler,
    pub(crate) depth_sampler: wgpu::Sampler,

    pub(crate) drop_buf: wgpu::Buffer,
    pub(crate) update_bufs: [wgpu::Buffer; 3],
    pub(crate) normal_bufs: [wgpu::Buffer; 3],
    pub(crate) hitbox_count_buf: wgpu::Buffer,
    /// The hitbox rows as of the last frame that applied them: a row that
    /// still holds the same body displaces from where it was then, so a
    /// body that stops moving stops displacing water.
    pub(crate) hitbox_prev_buf: wgpu::Buffer,
    /// The world origin the hitbox rows are relative to this frame, and how
    /// far it moved since `hitbox_prev_buf` was captured (`xyz`), which the
    /// shader subtracts from those rows.
    pub(crate) hitbox_origin: [f64; 3],
    pub(crate) hitbox_prev_shift: [f64; 3],

    pub(crate) pending_drops: std::collections::VecDeque<simulation::DropUniform>,
    pub(crate) drop_staged: bool,

    /// Static grid top face (normalized [-1,1] mapped to world by vertex shader)
    pub(crate) top_vbuf: wgpu::Buffer,
    pub(crate) top_ibuf: wgpu::Buffer,
    pub(crate) top_index_count: u32,

    /// Three tessellated source grids at the Ultra-Water cell budgets.
    pub(crate) caustics_vbufs: [wgpu::Buffer; 3],
    pub(crate) caustics_ibufs: [wgpu::Buffer; 3],
    pub(crate) caustics_index_counts: [u32; 3],
    /// Static near/mid/deep splat parameters, one uniform per depth slice.
    pub(crate) caustics_slice_params: [wgpu::Buffer; 3],
    /// 4x raster targets resolve into independent near and deep tile textures.
    pub(crate) _caustics_near_msaa_tex: wgpu::Texture,
    pub(crate) caustics_near_msaa_view: wgpu::TextureView,
    pub(crate) _caustics_deep_msaa_tex: wgpu::Texture,
    pub(crate) caustics_deep_msaa_view: wgpu::TextureView,
    pub(crate) _caustics_near_tex: wgpu::Texture,
    pub(crate) caustics_near_view: wgpu::TextureView,
    pub(crate) caustics_near_mip_views: Vec<wgpu::TextureView>,
    pub(crate) _caustics_deep_tex: wgpu::Texture,
    pub(crate) caustics_deep_view: wgpu::TextureView,
    pub(crate) caustics_deep_mip_views: Vec<wgpu::TextureView>,
    pub(crate) caustics_mip_pipeline: wgpu::RenderPipeline,

    pub(crate) caustics_sampler: wgpu::Sampler,
    /// Zeroed atmosphere frame / sky table, bound until a sky publishes.
    pub(crate) fallback_atmosphere: wgpu::Buffer,
    pub(crate) fallback_lights: wgpu::Buffer,
    pub(crate) fallback_sky_table: wgpu::Buffer,
    pub(crate) _fallback_sky_view_tex: wgpu::Texture,
    pub(crate) fallback_sky_view: wgpu::TextureView,

    /// Startup-baked, tileable RGBA foam material and its mip chain.
    pub(crate) _foam_texture: wgpu::Texture,
    pub(crate) foam_view: wgpu::TextureView,
    pub(crate) foam_mip_views: Vec<wgpu::TextureView>,
    pub(crate) foam_sampler: wgpu::Sampler,
    pub(crate) foam_bake_pipeline: wgpu::RenderPipeline,
    pub(crate) foam_mip_bgl: wgpu::BindGroupLayout,
    pub(crate) foam_mip_pipeline: wgpu::RenderPipeline,
    pub(crate) foam_baked: bool,

    pub(crate) caustics_render_bgl: wgpu::BindGroupLayout,
    pub(crate) render_bgl: wgpu::BindGroupLayout,
    pub(crate) render_bg: helio_core::CachedBindGroup,
    pub(crate) normal_bgs: Vec<Option<wgpu::BindGroup>>,
    pub(crate) normal_bg_keys: Vec<Option<wgpu::TextureView>>,

    /// Indexed like `update_bgs`; only each volume's cascade-0 layers are used.
    pub(crate) hitbox_bgs: Vec<Option<wgpu::BindGroup>>,
    pub(crate) hitbox_bg_keys: Vec<Option<(wgpu::TextureView, wgpu::Buffer, wgpu::Buffer)>>,
    pub(crate) drop_bg: Option<wgpu::BindGroup>,
    pub(crate) drop_bg_key: Option<wgpu::TextureView>,
    /// Two per layer, indexed `layer * 2 + front`: one for each ping-pong source.
    pub(crate) update_bgs: Vec<Option<wgpu::BindGroup>>,
    pub(crate) update_bg_keys: Vec<Option<(wgpu::TextureView, wgpu::Buffer)>>,
    pub(crate) underwater_tint_bg: helio_core::CachedBindGroup,

    pub(crate) caustics_pipeline: wgpu::RenderPipeline,
    pub(crate) surface_pipeline: wgpu::RenderPipeline,

    pub(crate) _pre_aa_fallback_tex: wgpu::Texture,
    pub(crate) pre_aa_fallback_view: wgpu::TextureView,

    pub(crate) _gbuffer_fallback_tex: wgpu::Texture,
    pub(crate) gbuffer_fallback_view: wgpu::TextureView,

    pub(crate) internal_width: u32,
    pub(crate) internal_height: u32,
    pub(crate) surface_format: wgpu::TextureFormat,
    pub(crate) viewport_buf: wgpu::Buffer,

    pub(crate) blit_bgl: wgpu::BindGroupLayout,
    pub(crate) blit_pipeline: wgpu::RenderPipeline,
    pub(crate) blit_bg: Option<wgpu::BindGroup>,
    pub(crate) blit_bg_key: Option<wgpu::TextureView>,

    pub(crate) water_output_view: Option<wgpu::TextureView>,
    /// Whether the simulation and tint targets are full size. The pass is a
    /// graph member only while some `"water_volumes"` row is live; until it
    /// joins they are 1-texel placeholders, and it frees them when it leaves.
    pub(crate) content_allocated: bool,
    /// This frame produced `water_output` and republished it as `pre_aa`.
    /// False when the scene has no volume buffer: the screen-space passes
    /// (scene blit, surface, underwater tint) would only copy `pre_aa`
    /// unchanged, so they are skipped and downstream passes keep reading the
    /// original `pre_aa`.
    pub(crate) screen_active: bool,
    /// Whether last frame simulated. The simulation restarts from the zeroed
    /// startup state when water appears.
    pub(crate) sim_live: bool,

    pub(crate) caustics_bg_keys: [Option<(wgpu::Buffer, wgpu::TextureView)>; 3],
    pub(crate) caustics_bgs: [Option<wgpu::BindGroup>; 3],

    pub(crate) _tint_scratch_tex: wgpu::Texture,
    /// The surface's own depth (see `pipeline::water_depth_target`).
    pub(crate) _water_depth_tex: wgpu::Texture,
    pub(crate) water_depth_view: wgpu::TextureView,
    pub(crate) tint_scratch_view: wgpu::TextureView,
    /// Samples `tint_scratch_view` for the blit back; rebuilt with it on resize.
    pub(crate) tint_blit_bg: Option<wgpu::BindGroup>,
    pub(crate) underwater_tint_bgl: wgpu::BindGroupLayout,
    pub(crate) underwater_tint_pipeline: wgpu::RenderPipeline,
    pub(crate) _shafts_scratch_tex: wgpu::Texture,
    pub(crate) shafts_view: wgpu::TextureView,
    pub(crate) shafts_bgl: wgpu::BindGroupLayout,
    pub(crate) shafts_pipeline: wgpu::RenderPipeline,
    pub(crate) shafts_bg: helio_core::CachedBindGroup,

    /// Shore-breaker spray and bounded impact-crown droplets.
    pub(crate) spray_bgl: wgpu::BindGroupLayout,
    pub(crate) spray_breaker_pipeline: wgpu::RenderPipeline,
    pub(crate) spray_impact_pipeline: wgpu::RenderPipeline,
    pub(crate) spray_bg: helio_core::CachedBindGroup,
    pub(crate) spray_impacts_buf: wgpu::Buffer,
    pub(crate) spray_params_buf: wgpu::Buffer,
    pub(crate) spray_emitters_buf: wgpu::Buffer,
    pub(crate) spray_emitters_pending: Vec<[f32; 4]>,
    pub(crate) spray_emitters_dirty: bool,
    pub(crate) spray_live_emitter_count: u32,
    pub(crate) spray_settings: WaterSpraySettings,
    pub(crate) spray_impacts: std::collections::VecDeque<WaterSprayImpact>,
    pub(crate) spray_live_impact_count: u32,

    /// Pass-wide dynamics, for rows that leave their own at zero (see
    /// [`GpuWaterVolume::sim_dynamics`]).
    pub(crate) wave_spring: f32,
    pub(crate) wave_damping: f32,

    pub(crate) wind_direction: [f32; 2],
    pub(crate) wind_strength: f32,
    pub(crate) wave_scale: f32,
    pub(crate) wave_speed: f32,
    /// Simulated seconds: the frame clock's advance, in whole steps.
    pub(crate) sim_time: f32,
    /// Frame clock time not yet simulated (less than one step).
    pub(crate) step_clock: f32,
    /// Simulation steps this frame takes ([`SIM_STEP_SECONDS`] each).
    pub(crate) steps_this_frame: u32,
}

/// Controls the water spray port. Breaker spray is active only when an
/// authored [`WaterShoreMap`] is installed. The default grid evaluates 128²
/// candidate emitters; each is culled in the vertex shader before rasterization.
#[derive(Debug, Clone, Copy)]
pub struct WaterSpraySettings {
    pub enabled: bool,
    /// Scales alpha for both breaker spray and impact droplets.
    pub amount: f32,
    /// Scales billboard size in metres.
    pub size: f32,
    /// Maximum camera distance for breaker emitters in metres.
    pub max_distance: f32,
    /// Requested valid surf-zone emitter count, clamped to 0..150,000.
    /// Device buffer limits can reduce the effective count.
    pub emitter_count: u32,
}

impl Default for WaterSpraySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            amount: 1.0,
            size: 1.0,
            max_distance: 350.0,
            emitter_count: MAX_WATER_SPRAY_EMITTERS,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WaterSprayImpact {
    absolute_position: glam::DVec3,
    age: f32,
    radius: f32,
    strength: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GpuWaterSprayImpact {
    pub(crate) position_age: [f32; 4],
    pub(crate) radius_strength: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GpuWaterSprayParams {
    pub(crate) amount: f32,
    pub(crate) size: f32,
    pub(crate) max_distance: f32,
    pub(crate) emitter_count: u32,
}

/// Pack shore-zone candidates using Ultra-Water's valid emitter bounds. The
/// valid-texel scan happens once per authored map/count change; deterministic
/// uniform sampling with replacement keeps a sparse beach as dense as the
/// requested target without a potentially unbounded rejection loop.
fn pack_shore_spray_emitters(map: &WaterShoreMap, target_count: u32) -> Vec<[f32; 4]> {
    if target_count == 0 {
        return Vec::new();
    }
    let size = SHORE_MAP_SIZE as usize;
    let mut eligible = Vec::<u32>::new();
    eligible.reserve(map.shore.len() / 16);
    for (idx, field) in map.shore.iter().enumerate() {
        let depth = -field[0];
        if (0.3..=3.2).contains(&depth) && field[1] <= 150.0 {
            eligible.push(idx as u32);
        }
    }
    if eligible.is_empty() {
        return Vec::new();
    }

    struct Lcg(u32);
    impl Lcg {
        fn next_f64(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            f64::from(self.0) / 4_294_967_296.0
        }
    }
    let mut rng = Lcg(11);
    let mut packed = Vec::with_capacity(target_count as usize);
    for _ in 0..target_count {
        let selected = (rng.next_f64() * eligible.len() as f64) as usize;
        let idx = eligible[selected.min(eligible.len() - 1)] as usize;
        let (i, j) = (idx % size, idx / size);
        let jitter_x = rng.next_f64();
        let jitter_z = rng.next_f64();
        let x = ((i as f64 + jitter_x) / size as f64 - 0.5) * f64::from(map.extent_xz[0]);
        let z = ((j as f64 + jitter_z) / size as f64 - 0.5) * f64::from(map.extent_xz[1]);
        packed.push([
            x as f32,
            z as f32,
            rng.next_f64() as f32,
            rng.next_f64() as f32,
        ]);
    }
    packed
}

// ---- Public API ----------------------------------------------------------------

impl WaterSimPass {
    /// Replace the bounded request set for the next rendered frame. Each
    /// point supplies its current world position and the water-volume row
    /// whose rest plane/local interaction layer the rendered surface uses.
    pub fn set_surface_queries(
        &mut self,
        points: impl IntoIterator<Item = WaterSurfaceQueryPoint>,
    ) {
        self.surface_queries.set_requests(points);
    }

    /// Drain results whose staged GPU readback has completed. This never
    /// blocks and each result carries its dispatch frame/time for staleness
    /// handling in physics.
    pub fn take_surface_query_results(&mut self) -> Vec<WaterSurfaceQueryResult> {
        self.surface_queries.take_completed()
    }

    /// Drain completed results into an engine-side adapter without coupling
    /// Helio to a particular physics crate or stepping backend.
    pub fn publish_surface_query_results(&mut self, consumer: &mut impl WaterSurfaceQueryConsumer) {
        let mut results = self.take_surface_query_results();
        results.sort_by_key(|result| result.sampled_frame);
        let mut start = 0;
        while start < results.len() {
            let frame = results[start].sampled_frame;
            let time = results[start].sampled_time;
            let end = results[start..]
                .iter()
                .position(|result| result.sampled_frame != frame)
                .map_or(results.len(), |relative| start + relative);
            consumer.publish_water_surface_queries(frame, time, &results[start..end]);
            start = end;
        }
    }

    /// Release a body's pending request and velocity history on teardown.
    pub fn remove_surface_query_body(&mut self, body_key: u64) {
        self.surface_queries.remove_body(body_key);
    }

    /// Set or clear the authored terrain-derived shoreline field. Setting a
    /// map is opt-in; this pass never guesses coastlines from water-volume AABBs.
    pub fn set_shore_map(&mut self, map: Option<WaterShoreMap>) {
        if let Some(map) = map.as_ref() {
            self.ocean_rest_height = map.rest_y;
        }
        self.spray_emitters_pending = map
            .as_ref()
            .map(|map| pack_shore_spray_emitters(map, self.spray_settings.emitter_count))
            .unwrap_or_default();
        self.spray_emitters_dirty = true;
        self.spray_bg.clear();
        self.shore_map = map;
        self.shore_map_dirty = self.shore_map.is_some();
        self.render_bg.clear();
    }

    /// Set the absolute-world ocean rest height used by mirrored captures.
    /// Keep this equal to `bounds_max.w` on the live `WaterVolumeRow`
    /// rows; Ultra-Water's global ocean has one rest plane.
    pub fn set_ocean_rest_height(&mut self, rest_y: f64) -> Result<(), &'static str> {
        if !rest_y.is_finite() {
            return Err("ocean rest height must be finite");
        }
        self.ocean_rest_height = rest_y;
        Ok(())
    }

    /// Set the authored ocean spectrum; defaults match Ultra-Water's README
    /// scene and parameter panel.
    pub fn set_ocean_spectrum(&mut self, spectrum: OceanSpectrum) {
        self.ocean_fft.set_spectrum(spectrum);
    }

    pub fn ocean_spectrum(&self) -> OceanSpectrum {
        self.ocean_fft.spectrum()
    }

    /// Pass-wide spring and damping, for volumes whose row leaves its own
    /// dynamics at zero (hosts that write pass rows themselves). A volume
    /// with its own (`sim_dynamics.x` > 0) simulates with those.
    pub fn set_sim_dynamics(&mut self, spring: f32, damping: f32) {
        self.wave_spring = spring.clamp(0.1, 2.0);
        self.wave_damping = damping.clamp(0.0, 1.0);
    }

    /// Pass-wide wind; see [`Self::set_sim_dynamics`].
    pub fn set_wind(&mut self, direction: [f32; 2], strength: f32) {
        let len = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt();
        self.wind_direction = if len > 1e-6 {
            [direction[0] / len, direction[1] / len]
        } else {
            [0.0, 0.0]
        };
        self.wind_strength = strength.max(0.0);
    }

    /// Pass-wide wave scale; see [`Self::set_sim_dynamics`].
    pub fn set_wave_scale(&mut self, scale: f32) {
        self.wave_scale = scale.max(0.01);
    }

    /// Pass-wide wave speed; see [`Self::set_sim_dynamics`].
    pub fn set_wave_speed(&mut self, speed: f32) {
        self.wave_speed = speed.max(0.0);
    }

    /// The simulation's height field as of the last frame: one array layer
    /// per volume row and cascade (layer `volume * 3 + cascade`), RGBA16F
    /// (R = height, G = velocity, B/A = normal XZ), `COPY_SRC`. Cascade `c`
    /// tiles world XZ every `[30, 90, 270][c]` metres. Read-only
    /// diagnostics, for tests that read the simulation back.
    pub fn sim_texture(&self) -> &wgpu::Texture {
        &self.sim_tex_a
    }

    pub fn add_drop(&mut self, center_x: f32, center_z: f32, radius: f32, strength: f32) {
        if self.pending_drops.len() < MAX_DROPS_BUFFERED {
            self.pending_drops.push_back(simulation::DropUniform {
                center: [center_x, center_z],
                radius,
                strength,
            });
        }
    }

    /// Configure shoreline breaker spray and impact droplets. Invalid values
    /// are replaced with defaults. The requested emitter count is bounded to
    /// `0..=150_000`; the effective count may be lower for device limits.
    pub fn set_spray_settings(&mut self, mut settings: WaterSpraySettings) {
        let defaults = WaterSpraySettings::default();
        if !settings.amount.is_finite() {
            settings.amount = defaults.amount;
        }
        if !settings.size.is_finite() {
            settings.size = defaults.size;
        }
        if !settings.max_distance.is_finite() {
            settings.max_distance = defaults.max_distance;
        }
        settings.amount = settings.amount.clamp(0.0, 4.0);
        settings.size = settings.size.clamp(0.05, 8.0);
        settings.max_distance = settings.max_distance.clamp(10.0, 2000.0);
        settings.emitter_count = settings.emitter_count.min(MAX_WATER_SPRAY_EMITTERS);
        let emitter_count_changed = settings.emitter_count != self.spray_settings.emitter_count;
        self.spray_settings = settings;
        if emitter_count_changed {
            self.spray_emitters_pending = self
                .shore_map
                .as_ref()
                .map(|map| pack_shore_spray_emitters(map, settings.emitter_count))
                .unwrap_or_default();
            self.spray_emitters_dirty = true;
        }
        self.spray_bg.clear();
    }

    /// Number of valid shore emitters currently bound for rendering, after
    /// requested count and device storage limits are applied.
    pub fn effective_spray_emitter_count(&self) -> u32 {
        self.spray_live_emitter_count
    }

    /// Enqueue a short-lived impact splash at an absolute-world point on the
    /// water surface. Up to 16 events are retained; when full, the oldest
    /// event is dropped so recent impacts remain visible. Radius is metres and
    /// strength is relative event energy. Invalid positions or nonpositive
    /// values return an error without changing the queue.
    pub fn add_spray_impact(
        &mut self,
        absolute_position: glam::DVec3,
        radius: f32,
        strength: f32,
    ) -> Result<(), &'static str> {
        if !absolute_position.is_finite() || !radius.is_finite() || !strength.is_finite() {
            return Err("spray impact position, radius, and strength must be finite");
        }
        if radius <= 0.0 || strength <= 0.0 {
            return Err("spray impact radius and strength must be positive");
        }
        if !self.spray_settings.enabled {
            return Ok(());
        }
        if self.spray_impacts.len() == MAX_WATER_SPRAY_IMPACTS {
            self.spray_impacts.pop_front();
        }
        self.spray_impacts.push_back(WaterSprayImpact {
            absolute_position,
            age: 0.0,
            radius: radius.clamp(0.05, 8.0),
            strength: strength.clamp(0.001, 10.0),
        });
        Ok(())
    }

    /// Clears every simulation layer back to the zeroed startup state.
    fn reset_simulation(&mut self, encoder: &mut CommandRecorder<'_>) {
        for view in self.sim_layer_views_a.iter().chain(&self.sim_layer_views_b) {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("WaterSim Reset"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.front_per_layer
            .iter_mut()
            .for_each(|front| *front = true);
        // Bodies already in the water push it once as it starts.
        encoder.clear_buffer(&self.hitbox_prev_buf, 0, None);
        self.hitbox_prev_shift = [0.0; 3];
    }

    /// Gives the simulation and tint targets their full size (`true`) or
    /// 1-texel placeholders (`false`), and drops what was bound to the old
    /// ones. The simulation restarts from its startup state either way.
    fn set_content_allocated(&mut self, device: &wgpu::Device, allocated: bool) {
        let sim = pipeline::SimTextures::new(device, if allocated { SIM_SIZE } else { 1 });
        self.sim_tex_a = sim.tex_a;
        self.sim_tex_b = sim.tex_b;
        self.sim_array_view_a = sim.array_a;
        self.sim_array_view_b = sim.array_b;
        self.sim_layer_views_a = sim.layers_a;
        self.sim_layer_views_b = sim.layers_b;
        self.front_per_layer
            .iter_mut()
            .for_each(|front| *front = true);
        self.sim_live = false;
        self.content_allocated = allocated;
        self.resize_internal(device, self.internal_width, self.internal_height);
    }

    pub fn resize_internal(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (tint_w, tint_h) = if self.content_allocated {
            (width, height)
        } else {
            (1, 1)
        };
        let (tint_scratch_tex, tint_scratch_view) =
            pipeline::tint_scratch(device, tint_w, tint_h, self.surface_format);
        self.tint_scratch_view = tint_scratch_view;
        self._tint_scratch_tex = tint_scratch_tex;
        let (water_depth_tex, water_depth_view) =
            pipeline::water_depth_target(device, tint_w, tint_h);
        self._water_depth_tex = water_depth_tex;
        self.water_depth_view = water_depth_view;
        self.tint_blit_bg = None;
        let (shafts_tex, shafts_view) = pipeline::shafts_scratch(
            device,
            if self.content_allocated {
                width.div_ceil(2)
            } else {
                1
            },
            if self.content_allocated {
                height.div_ceil(2)
            } else {
                1
            },
        );
        self._shafts_scratch_tex = shafts_tex;
        self.shafts_view = shafts_view;
        self.shafts_bg.clear();
        self.spray_bg.clear();

        self.viewport_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Water Viewport"),
            contents: bytemuck::cast_slice(&[
                width as f32,
                height as f32,
                1.0 / width as f32,
                1.0 / height as f32,
            ]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        self.internal_width = width;
        self.internal_height = height;

        self.water_output_view = None;
        self.render_bg.clear();
        self.blit_bg = None;
        self.blit_bg_key = None;
    }
}

// ---- RenderPass impl ----------------------------------------------------------------

impl RenderPass for WaterSimPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &[
            "water_hitboxes",
            "water_volumes",
            helio_light_api::LIGHT_LIST,
        ]
    }

    fn name(&self) -> &'static str {
        "WaterSim"
    }

    /// A member while the scene has water: some `"water_volumes"` row is live.
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        scene.holds_rows("water_volumes")
    }

    fn on_leave(&mut self, device: &wgpu::Device) {
        // Re-created at full size when water returns.
        self.set_content_allocated(device, false);
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn on_resize(&mut self, device: &wgpu::Device, size: helio_core::RenderSize) {
        let [width, height] = size.internal;
        self.resize_internal(device, width, height);
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read(helio_frame_api::color::SCENE_COLOR);
        builder.write_color(
            "water_output",
            ResourceFormat::from(self.surface_format),
            ResourceSize::MatchSurface,
        );
        builder.publish_alias("water_output", helio_frame_api::color::SCENE_COLOR);
        // The pass owns these mipmapped outputs; the graph tracks producer/
        // consumer ordering without allocating one-mip placeholder textures.
        builder.write_buffer("water_caustics");
        builder.write_buffer("water_caustics_deep");
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            helio_frame_api::gbuffer::GROUP,
            helio_frame_api::gbuffer::DEPTH,
            helio_frame_api::color::SCENE_COLOR,
            // Min-reduced depth pyramid, marched for water reflections. Built
            // by HiZBuildPass from `depth` alone, so it is available here.
            helio_frame_api::depth::HIZ_NEAREST,
            // The sky the sea reflects and scatters.
            helio_frame_api::lighting::sky::ATMOSPHERE_FRAME,
            helio_frame_api::lighting::sky::ATMOSPHERE_SKY_TABLE,
            helio_frame_api::lighting::sky::ATMOSPHERE_SKY_VIEW,
        ]
    }
    fn writes(&self) -> &'static [&'static str] {
        &[
            "water_sim_texture",
            "water_sim_sampler",
            "water_ocean_displacement",
            "water_ocean_slopes",
            "water_shore_fields",
            "water_shore_params",
            "water_caustics",
            "water_caustics_deep",
            "water_planar_reflection",
            "water_planar_reflection_camera",
            reactivity::WATER_SURFACE_DEPTH,
            helio_frame_api::color::SCENE_COLOR,
        ]
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        // SAFETY: every borrow below is extended out of `self`, which is
        // pass-lifetime (owned by the RenderGraph across many frames), never
        // frame-scoped -- `'a` is always shorter than these fields' real
        // lifetime. `publish(&self, ..)` (not `&'a self`) cannot express that
        // relationship, so the borrow checker sees an unrelated, shorter
        // lifetime here instead; matches `ResourceRegistry::
        // write_texture_binding`'s own transmute for the same reason.
        let view: &'a wgpu::TextureView = unsafe {
            std::mem::transmute(if self.front_per_layer[0] {
                &self.sim_layer_views_a[0]
            } else {
                &self.sim_layer_views_b[0]
            })
        };
        frame.write(
            helio_core::ResourceKey::new("water_sim_texture"),
            view,
            "WaterSim",
        );
        let sampler: &'a wgpu::Sampler = unsafe { std::mem::transmute(&self.output_sampler) };
        frame.write(
            helio_core::ResourceKey::new("water_sim_sampler"),
            sampler,
            "WaterSim",
        );
        // The surface's own depth: where it drew this frame. TSR's reactivity
        // pass (`WaterReactivityPass`) reads it to say where the water is.
        let surface_depth: &'a wgpu::TextureView =
            unsafe { std::mem::transmute(&self.water_depth_view) };
        frame.write(
            helio_core::ResourceKey::new(reactivity::WATER_SURFACE_DEPTH),
            surface_depth,
            "WaterSim",
        );
        if let Some(capture) = self.reflection_capture.as_ref() {
            let reflection: &'a wgpu::TextureView =
                unsafe { std::mem::transmute(capture.reflection_view()) };
            frame.write(
                reflection_capture::REFLECTION_RESOURCE,
                reflection,
                "WaterSim",
            );
            let camera: &'a wgpu::Buffer = unsafe { std::mem::transmute(capture.camera_buffer()) };
            frame.write(
                helio_core::ResourceKey::new("water_planar_reflection_camera"),
                camera,
                "WaterSim",
            );
        }
        let displacement: &'a wgpu::TextureView =
            unsafe { std::mem::transmute(self.ocean_fft.displacement_view()) };
        frame.write(
            helio_core::ResourceKey::new("water_ocean_displacement"),
            displacement,
            "WaterSim",
        );
        let slopes: &'a wgpu::TextureView =
            unsafe { std::mem::transmute(self.ocean_fft.slopes_view()) };
        frame.write(
            helio_core::ResourceKey::new("water_ocean_slopes"),
            slopes,
            "WaterSim",
        );
        let caustics_near: &'a wgpu::TextureView =
            unsafe { std::mem::transmute(&self.caustics_near_view) };
        frame.write(
            helio_core::ResourceKey::new("water_caustics"),
            caustics_near,
            "WaterSim",
        );
        let caustics_deep: &'a wgpu::TextureView =
            unsafe { std::mem::transmute(&self.caustics_deep_view) };
        frame.write(
            helio_core::ResourceKey::new("water_caustics_deep"),
            caustics_deep,
            "WaterSim",
        );
        // Shared with voxel terrain's optional scene-material path: the terrain
        // evaluates the same travel-time field for beach run-up and wet sand.
        let shore_fields: &'a wgpu::TextureView = unsafe { std::mem::transmute(&self.shore_view) };
        frame.write_texture_binding("water_shore_fields", shore_fields, "WaterSim");
        let shore_params: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.shore_params) };
        frame.write_buffer_binding("water_shore_params", shore_params, "WaterSim");
        if let Some(view) = self
            .water_output_view
            .as_ref()
            .filter(|_| self.screen_active)
        {
            let view: &'a wgpu::TextureView = unsafe { std::mem::transmute(view) };
            frame.write(
                helio_core::ResourceKey::new(helio_frame_api::color::SCENE_COLOR),
                view,
                "WaterSim",
            );
        }
    }

    /// Surface reflections march the min-depth pyramid, but only on frames
    /// where the screen-space water path runs (some volume row may be live).
    fn declare_frame_demands(
        &mut self,
        ctx: &PrepareContext,
        demands: &mut helio_core::FrameDemands,
    ) {
        if ctx
            .scene_buffers
            .get(BufferKey::of("water_volumes"))
            .is_some()
        {
            demands.demand(helio_frame_api::depth::HIZ_NEAREST);
        }
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        self.surface_queries.prepare();
        let reflection_size = (ctx.width.max(1).div_ceil(2), ctx.height.max(1).div_ceil(2));
        if let Some(capture) = self.reflection_capture.as_mut() {
            capture.resize(reflection_size.0, reflection_size.1);
        } else {
            self.reflection_capture = Some(reflection_capture::PlanarReflectionCapture::new(
                ctx.device,
                ctx.queue,
                reflection_size.0,
                reflection_size.1,
            ));
        }
        if !self.content_allocated {
            self.set_content_allocated(ctx.device, true);
        }
        // Fixed steps on the host's frame clock: none while it is frozen.
        self.step_clock += ctx.time_delta.max(0.0);
        let steps = ((self.step_clock / SIM_STEP_SECONDS) + 1e-3)
            .floor()
            .max(0.0) as u32;
        self.steps_this_frame = steps.min(MAX_SIM_STEPS_PER_FRAME);
        self.step_clock =
            (self.step_clock - steps as f32 * SIM_STEP_SECONDS).clamp(0.0, SIM_STEP_SECONDS);
        self.sim_time += self.steps_this_frame as f32 * SIM_STEP_SECONDS;
        self.ocean_fft
            .prepare(ctx.queue, ctx.time_delta.max(0.0), ctx.world_origin);
        if self.spray_emitters_dirty {
            let limits = ctx.device.limits();
            let binding_limit =
                u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
            let capacity = (binding_limit / WATER_SPRAY_EMITTER_STRIDE) as usize;
            let count = self.spray_emitters_pending.len().min(capacity);
            let packed = &self.spray_emitters_pending[..count];
            let dummy = [[0.0f32; 4]; 1];
            let contents = if packed.is_empty() {
                bytemuck::cast_slice(&dummy)
            } else {
                bytemuck::cast_slice(packed)
            };
            self.spray_emitters_buf =
                ctx.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Packed Water Breaker Emitters"),
                        contents,
                        usage: wgpu::BufferUsages::STORAGE,
                    });
            self.spray_live_emitter_count = count as u32;
            self.spray_emitters_dirty = false;
            self.spray_bg.clear();
        }
        let spray_dt = ctx.time_delta.max(0.0);
        for impact in &mut self.spray_impacts {
            impact.age += spray_dt;
        }
        self.spray_impacts
            .retain(|impact| impact.age <= MAX_WATER_SPRAY_IMPACT_AGE);
        self.spray_live_impact_count = self.spray_impacts.len() as u32;
        let spray_origin =
            glam::DVec3::new(ctx.world_origin.x, ctx.world_origin.y, ctx.world_origin.z);
        let mut gpu_impacts = vec![GpuWaterSprayImpact::zeroed(); MAX_WATER_SPRAY_IMPACTS];
        for (gpu, impact) in gpu_impacts.iter_mut().zip(self.spray_impacts.iter()) {
            let p = impact.absolute_position - spray_origin;
            gpu.position_age = [p.x as f32, p.y as f32, p.z as f32, impact.age];
            gpu.radius_strength = [impact.radius, impact.strength, 0.0, 0.0];
        }
        ctx.write_buffer(
            &self.spray_impacts_buf,
            0,
            bytemuck::cast_slice(&gpu_impacts),
        );
        let spray = &self.spray_settings;
        ctx.write_buffer(
            &self.spray_params_buf,
            0,
            bytemuck::bytes_of(&GpuWaterSprayParams {
                amount: if spray.enabled { spray.amount } else { 0.0 },
                size: spray.size,
                max_distance: spray.max_distance,
                emitter_count: self.spray_live_emitter_count,
            }),
        );
        let (center, extent, enabled) = if let Some(map) = self.shore_map.as_ref() {
            if self.shore_map_dirty {
                let row_bytes = SHORE_MAP_SIZE * 4 * 4;
                for (layer, data) in [&map.shore, &map.travel].into_iter().enumerate() {
                    ctx.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self._shore_texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: 0,
                                y: 0,
                                z: layer as u32,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        bytemuck::cast_slice(data),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row_bytes),
                            rows_per_image: Some(SHORE_MAP_SIZE),
                        },
                        wgpu::Extent3d {
                            width: SHORE_MAP_SIZE,
                            height: SHORE_MAP_SIZE,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                self.shore_map_dirty = false;
            }
            (
                [
                    (map.center_xz[0] - ctx.world_origin.x) as f32,
                    (map.center_xz[1] - ctx.world_origin.z) as f32,
                ],
                map.extent_xz,
                1.0f32,
            )
        } else {
            ([0.0; 2], [1.0; 2], 0.0f32)
        };
        let center_eye = self.shore_map.as_ref().map_or([0.0; 2], |map| {
            [
                (map.center_xz[0]
                    - ctx.world_origin.x
                    - f64::from(ctx.camera_data.position_near[0])) as f32,
                (map.center_xz[1]
                    - ctx.world_origin.z
                    - f64::from(ctx.camera_data.position_near[2])) as f32,
            ]
        });
        let shore_params = [
            center[0],
            center[1],
            extent[0],
            extent[1],
            enabled,
            self.ocean_fft.time(),
            // Terrain hit positions are eye-relative, while surface shading
            // uses frame-relative coordinates. This spare pair carries the
            // same map center relative to the current eye for terrain swash.
            center_eye[0],
            center_eye[1],
        ];
        ctx.write_buffer(&self.shore_params, 0, bytemuck::cast_slice(&shore_params));
        let step_dt = SIM_STEP_SECONDS;
        for ci in 0..CASCADE_COUNT {
            let delta = simulation::DeltaUniform {
                delta: [1.0 / SIM_SIZE as f32, 1.0 / SIM_SIZE as f32],
                spring: self.wave_spring,
                damping: self.wave_damping,
                wind_dir: self.wind_direction,
                wind_strength: self.wind_strength,
                time: self.sim_time,
                wave_scale: self.wave_scale,
                time_step: step_dt,
                cascade_patch_size: CASCADE_PATCH_SIZES[ci],
                cascade_id: ci as u32,
                wave_speed: self.wave_speed,
                _pad: [0.0; 3],
            };
            ctx.write_buffer(&self.update_bufs[ci], 0, bytemuck::bytes_of(&delta));
            ctx.write_buffer(&self.normal_bufs[ci], 0, bytemuck::bytes_of(&delta));
        }

        // Hitbox rows are relative to the frame's world origin; the previous
        // rows are relative to the one they were captured at.
        let origin = ctx.world_origin.to_array();
        for axis in 0..3 {
            self.hitbox_prev_shift[axis] += origin[axis] - self.hitbox_origin[axis];
        }
        self.hitbox_origin = origin;

        // Fixed capacity, not a live count: `"water_hitboxes"` is a SceneDB
        // buffer read directly by `execute()`/the hitbox shader (see
        // `MAX_WATER_HITBOXES`'s doc) -- no per-frame CPU scan of the World
        // is ever needed to produce this number.
        ctx.write_buffer(
            &self.hitbox_count_buf,
            0,
            bytemuck::bytes_of(&simulation::HitboxCountUniform {
                count: MAX_WATER_HITBOXES,
                _pad: [0; 3],
                previous_shift: [
                    self.hitbox_prev_shift[0] as f32,
                    self.hitbox_prev_shift[1] as f32,
                    self.hitbox_prev_shift[2] as f32,
                    0.0,
                ],
            }),
        );

        self.drop_staged = false;
        if let Some(drop) = self.pending_drops.pop_front() {
            ctx.write_buffer(&self.drop_buf, 0, bytemuck::bytes_of(&drop));
            self.drop_staged = true;
        }

        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if let Some(capture) = self.reflection_capture.as_mut() {
            capture.render_from_context(ctx, self.ocean_rest_height)?;
        }
        // Spectral ocean is the primary wave source. The legacy simulation
        // remains a localized hitbox/drop interaction adapter; it no longer
        // supplies wind-sea waves.
        {
            let mut encoder = ctx.graphics_cmds();
            self.ocean_fft.execute(&mut encoder);
        }
        // `"water_volumes"`/`"water_hitboxes"` are resolved fresh from the
        // SceneDB mirror by key every frame -- no Renderer method, no
        // Scene-owned arena, no CPU-tracked dirty range. Both buffers are
        // fixed-capacity (`MAX_SIM_VOLUMES`/`MAX_WATER_HITBOXES` rows): a row
        // with no live entity behind it is `Zeroable`-default, which every
        // consuming shader stage below treats as inert (see each constant's
        // doc and `surface.wgsl`'s `1e-4`-guarded extent/`hitbox.frag.wgsl`'s
        // `strength == 0` short-circuit).
        let water_volumes_buf = ctx
            .scene_buffers
            .get(BufferKey::of("water_volumes"))
            .map(|handle| &handle.buffer);
        let water_hitboxes_buf = ctx
            .scene_buffers
            .get(BufferKey::of("water_hitboxes"))
            .map(|handle| &handle.buffer);
        // The pass runs only while some row is live (`is_member`); empty rows
        // are inert everywhere (no surface, no caustics, no tint). A volume
        // that appears starts from the startup state.
        let sim_live = water_volumes_buf.is_some();
        if sim_live && !self.sim_live {
            self.reset_simulation(&mut ctx.graphics_cmds());
        }
        self.sim_live = sim_live;
        let volume_count = if sim_live { MAX_SIM_VOLUMES } else { 0 };
        let hitbox_count = if water_hitboxes_buf.is_some() {
            MAX_WATER_HITBOXES
        } else {
            0
        };
        let total_layers = volume_count * CASCADE_COUNT as u32;

        let layer_view = |layer: usize, front: bool| -> &wgpu::TextureView {
            if front {
                &self.sim_layer_views_a[layer]
            } else {
                &self.sim_layer_views_b[layer]
            }
        };

        // ---- 1. Hitbox displacement (cascade 0 of each volume) -------------
        // World-space boxes, mapped into each volume's sim space by the
        // shader (the draw's instance is the volume). Applied once per frame
        // that steps the simulation, then remembered as the previous rows.
        if let (true, Some(hitboxes_buf), Some(vols_buf)) = (
            hitbox_count > 0 && volume_count > 0 && self.steps_this_frame > 0,
            water_hitboxes_buf,
            water_volumes_buf,
        ) {
            {
                for vol_idx in 0..volume_count {
                    let layer = (vol_idx * CASCADE_COUNT as u32) as usize;
                    let src = layer_view(layer, self.front_per_layer[layer]);
                    let dst = if self.front_per_layer[layer] {
                        &self.sim_layer_views_b[layer]
                    } else {
                        &self.sim_layer_views_a[layer]
                    };

                    let new_key = (src.clone(), hitboxes_buf.clone(), vols_buf.clone());
                    let slot = layer * 2 + self.front_per_layer[layer] as usize;
                    if self.hitbox_bg_keys[slot].as_ref() != Some(&new_key) {
                        self.hitbox_bgs[slot] =
                            Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some("WaterSim Hitbox BG"),
                                layout: &self.hitbox_bgl,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(src),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 2,
                                        resource: self.hitbox_count_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 3,
                                        resource: hitboxes_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 4,
                                        resource: self.hitbox_prev_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 5,
                                        resource: vols_buf.as_entire_binding(),
                                    },
                                ],
                            }));
                        self.hitbox_bg_keys[slot] = Some(new_key);
                    }
                    let bg = self.hitbox_bgs[slot].as_ref().unwrap();

                    let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                        view: dst,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })];
                    let desc = wgpu::RenderPassDescriptor {
                        label: Some("WaterSim Hitbox"),
                        color_attachments: &color_attachments,
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    };
                    let mut pass = ctx.begin_render_pass(&desc);
                    pass.set_pipeline(&self.hitbox_pipeline);
                    pass.set_bind_group(0, bg, &[]);
                    pass.draw(0..6, vol_idx..vol_idx + 1);
                    drop(pass);
                    self.front_per_layer[layer] = !self.front_per_layer[layer];
                }
            }
            // Remember where every body was applied.
            if hitboxes_buf.usage().contains(wgpu::BufferUsages::COPY_SRC) {
                let size = hitboxes_buf.size().min(self.hitbox_prev_buf.size());
                ctx.graphics_cmds().copy_buffer_to_buffer(
                    hitboxes_buf,
                    0,
                    &self.hitbox_prev_buf,
                    0,
                    size,
                );
                self.hitbox_prev_shift = [0.0; 3];
            }
        }

        // ---- 2. Drop ripple (cascade 0 for all volumes) --------------------
        if self.drop_staged {
            for vol_idx in 0..volume_count {
                let layer = (vol_idx * CASCADE_COUNT as u32) as usize;
                let src = layer_view(layer, self.front_per_layer[layer]);
                let dst = if self.front_per_layer[layer] {
                    &self.sim_layer_views_b[layer]
                } else {
                    &self.sim_layer_views_a[layer]
                };

                if self.drop_bg_key.as_ref() != Some(src) {
                    self.drop_bg = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("WaterSim Drop BG"),
                        layout: &self.sim_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(src),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: self.drop_buf.as_entire_binding(),
                            },
                        ],
                    }));
                    self.drop_bg_key = Some(src.clone());
                }
                let bg = self.drop_bg.as_ref().unwrap();

                let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                    view: dst,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })];
                let desc = wgpu::RenderPassDescriptor {
                    label: Some("WaterSim Drop"),
                    color_attachments: &color_attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                };
                let mut pass = ctx.begin_render_pass(&desc);
                pass.set_pipeline(&self.drop_pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..6, 0..1);
                drop(pass);
                self.front_per_layer[layer] = !self.front_per_layer[layer];
            }
        }

        // ---- 3 & 4. Cascade wave propagation + normal recomputation --------
        // One propagation step per simulation step this frame; none (and no
        // normal pass, which would only recompute the same normals) while
        // the frame clock is frozen.
        let steps = self.steps_this_frame;
        if let (Some(vols_buf), true) = (water_volumes_buf, steps > 0) {
            for vol_idx in 0..volume_count {
                let base = (vol_idx * CASCADE_COUNT as u32) as usize;
                for ci in 0..CASCADE_COUNT {
                    let layer = base + ci;

                    // ---- 3. Wave propagation (`steps` per layer) ---------------------
                    for i in 0..steps {
                        let src = layer_view(layer, self.front_per_layer[layer]);
                        let dst = if self.front_per_layer[layer] {
                            &self.sim_layer_views_b[layer]
                        } else {
                            &self.sim_layer_views_a[layer]
                        };

                        let update_key = (src.clone(), vols_buf.clone());
                        // One slot per ping-pong side: with several steps a
                        // frame the source alternates, and a single slot
                        // would rebuild on every step.
                        let slot = layer * 2 + self.front_per_layer[layer] as usize;
                        if self.update_bg_keys[slot].as_ref() != Some(&update_key) {
                            let bg_label = format!("WaterSim Update BG V{} C{}", vol_idx, ci);
                            self.update_bgs[slot] =
                                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: Some(&bg_label),
                                    layout: &self.update_bgl,
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: wgpu::BindingResource::TextureView(src),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 2,
                                            resource: self.update_bufs[ci].as_entire_binding(),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 3,
                                            resource: vols_buf.as_entire_binding(),
                                        },
                                    ],
                                }));
                            self.update_bg_keys[slot] = Some(update_key);
                        }
                        let bg = self.update_bgs[slot].as_ref().unwrap();

                        let label_str = format!("WaterSim Update {} V{} C{}", i + 1, vol_idx, ci);
                        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                            view: dst,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })];
                        let desc = wgpu::RenderPassDescriptor {
                            label: Some(&label_str),
                            color_attachments: &color_attachments,
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        };
                        let mut pass = ctx.begin_render_pass(&desc);
                        pass.set_pipeline(&self.update_pipeline);
                        pass.set_bind_group(0, bg, &[]);
                        // The instance is the volume whose row it reads.
                        pass.draw(0..6, vol_idx..vol_idx + 1);
                        drop(pass);
                        self.front_per_layer[layer] = !self.front_per_layer[layer];
                    }

                    // ---- 4. Normal recomputation (per layer) --------------------------
                    {
                        let src = layer_view(layer, self.front_per_layer[layer]);
                        let dst = if self.front_per_layer[layer] {
                            &self.sim_layer_views_b[layer]
                        } else {
                            &self.sim_layer_views_a[layer]
                        };

                        if self.normal_bg_keys[layer].as_ref() != Some(src) {
                            let nrm_label = format!("WaterSim Normal BG V{} C{}", vol_idx, ci);
                            self.normal_bgs[layer] =
                                Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: Some(&nrm_label),
                                    layout: &self.sim_bgl,
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: wgpu::BindingResource::TextureView(src),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 2,
                                            resource: self.normal_bufs[ci].as_entire_binding(),
                                        },
                                    ],
                                }));
                            self.normal_bg_keys[layer] = Some(src.clone());
                        }
                        let bg = self.normal_bgs[layer].as_ref().unwrap();

                        let nrm_pass_label = format!("WaterSim Normal V{} C{}", vol_idx, ci);
                        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                            view: dst,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })];
                        let desc = wgpu::RenderPassDescriptor {
                            label: Some(&nrm_pass_label),
                            color_attachments: &color_attachments,
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        };
                        let mut pass = ctx.begin_render_pass(&desc);
                        pass.set_pipeline(&self.normal_pipeline);
                        pass.set_bind_group(0, bg, &[]);
                        pass.draw(0..6, 0..1);
                        drop(pass);
                        self.front_per_layer[layer] = !self.front_per_layer[layer];
                    }
                }
            }
        }

        // ---- Consolidate: copy any layers still on tex_b to tex_a --------
        // After simulation, ensure all layers are on tex_a for rendering.
        let mut encoder = ctx.graphics_cmds();
        for layer in 0..total_layers as usize {
            if !self.front_per_layer[layer] {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.sim_tex_b,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.sim_tex_a,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: SIM_SIZE,
                        height: SIM_SIZE,
                        depth_or_array_layers: 1,
                    },
                );
                self.front_per_layer[layer] = true;
            }
        }

        // ---- 5. Global FFT caustics by area-ratio triangle splats ------------
        if volume_count > 0 {
            if let Some(vols_buf) = water_volumes_buf {
                let new_key = (vols_buf.clone(), self.ocean_fft.slopes_view().clone());
                for slice in 0..3 {
                    if self.caustics_bg_keys[slice].as_ref() != Some(&new_key) {
                        self.caustics_bgs[slice] =
                            Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some("Water Caustics BG"),
                                layout: &self.caustics_render_bgl,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: vols_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::TextureView(
                                            self.ocean_fft.slopes_view(),
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 2,
                                        resource: wgpu::BindingResource::Sampler(
                                            &self.output_sampler,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 3,
                                        resource:
                                            self.caustics_slice_params[slice].as_entire_binding(),
                                    },
                                ],
                            }));
                        self.caustics_bg_keys[slice] = Some(new_key.clone());
                    }
                }

                let near_attachments = [Some(wgpu::RenderPassColorAttachment {
                    view: &self.caustics_near_msaa_view,
                    resolve_target: Some(&self.caustics_near_mip_views[0]),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })];
                let near_desc = wgpu::RenderPassDescriptor {
                    label: Some("Water Near Caustics Area Splat"),
                    color_attachments: &near_attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                };
                {
                    let mut pass = ctx.begin_render_pass(&near_desc);
                    pass.set_pipeline(&self.caustics_pipeline);
                    pass.set_viewport(
                        0.0,
                        0.0,
                        CAUSTICS_SIZE as f32,
                        CAUSTICS_SIZE as f32,
                        0.0,
                        1.0,
                    );
                    pass.set_scissor_rect(0, 0, CAUSTICS_SIZE, CAUSTICS_SIZE);
                    pass.set_bind_group(0, self.caustics_bgs[0].as_ref().unwrap(), &[]);
                    pass.set_vertex_buffer(0, self.caustics_vbufs[0].slice(..));
                    pass.set_index_buffer(
                        self.caustics_ibufs[0].slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(0..self.caustics_index_counts[0], 0, 0..1);
                }

                let deep_attachments = [Some(wgpu::RenderPassColorAttachment {
                    view: &self.caustics_deep_msaa_view,
                    resolve_target: Some(&self.caustics_deep_mip_views[0]),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })];
                let deep_desc = wgpu::RenderPassDescriptor {
                    label: Some("Water Deep Caustics Area Splat"),
                    color_attachments: &deep_attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                };
                {
                    let mut pass = ctx.begin_render_pass(&deep_desc);
                    pass.set_pipeline(&self.caustics_pipeline);
                    pass.set_viewport(
                        0.0,
                        0.0,
                        CAUSTICS_DEEP_SIZE as f32,
                        CAUSTICS_DEEP_SIZE as f32,
                        0.0,
                        1.0,
                    );
                    pass.set_scissor_rect(0, 0, CAUSTICS_DEEP_SIZE, CAUSTICS_DEEP_SIZE);
                    for slice in 1..3 {
                        pass.set_bind_group(0, self.caustics_bgs[slice].as_ref().unwrap(), &[]);
                        pass.set_vertex_buffer(0, self.caustics_vbufs[slice].slice(..));
                        pass.set_index_buffer(
                            self.caustics_ibufs[slice].slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..self.caustics_index_counts[slice], 0, 0..1);
                    }
                }

                // Generate independent repeatable mip chains. Keeping near
                // and deep data in separate textures avoids bilinear/mip
                // bleeding between packed depth slices.
                for mip in 1..self.caustics_near_mip_views.len() {
                    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Water Near Caustics Mip BG"),
                        layout: &self.foam_mip_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(
                                    &self.caustics_near_mip_views[mip - 1],
                                ),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.caustics_sampler),
                            },
                        ],
                    });
                    let attachments = [Some(wgpu::RenderPassColorAttachment {
                        view: &self.caustics_near_mip_views[mip],
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })];
                    let desc = wgpu::RenderPassDescriptor {
                        label: Some("Water Near Caustics Mip"),
                        color_attachments: &attachments,
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    };
                    let mut pass = ctx.begin_render_pass(&desc);
                    pass.set_pipeline(&self.caustics_mip_pipeline);
                    pass.set_bind_group(0, &bind_group, &[]);
                    pass.draw(0..3, 0..1);
                }
                for mip in 1..self.caustics_deep_mip_views.len() {
                    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Water Deep Caustics Mip BG"),
                        layout: &self.foam_mip_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(
                                    &self.caustics_deep_mip_views[mip - 1],
                                ),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.caustics_sampler),
                            },
                        ],
                    });
                    let attachments = [Some(wgpu::RenderPassColorAttachment {
                        view: &self.caustics_deep_mip_views[mip],
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })];
                    let desc = wgpu::RenderPassDescriptor {
                        label: Some("Water Deep Caustics Mip"),
                        color_attachments: &attachments,
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    };
                    let mut pass = ctx.begin_render_pass(&desc);
                    pass.set_pipeline(&self.caustics_mip_pipeline);
                    pass.set_bind_group(0, &bind_group, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
        }

        // Everything below is screen-space and only reproduces `pre_aa` when
        // no volume is live. Skip it rather than copying the full-resolution
        // image. The simulation and caustics above are fixed-size and keep
        // running so their state is unchanged when a volume appears.
        self.screen_active = water_volumes_buf.is_some();
        if !self.screen_active {
            return Ok(());
        }

        // ---- 6. Blit pre_aa -> water_output (scene baseline) -----------------
        if self.water_output_view.is_none() {
            self.water_output_view = ctx.resource_pool.get_view("water_output").cloned();
        }
        let water_output_view = self
            .water_output_view
            .as_ref()
            .expect("water_output view from graph");
        let scene_view: &wgpu::TextureView = ctx
            .registry
            .get(helio_core::ResourceKey::new(
                helio_frame_api::color::SCENE_COLOR,
            ))
            .unwrap_or(&self.pre_aa_fallback_view);
        if self.blit_bg_key.as_ref() != Some(scene_view) {
            self.blit_bg = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Water Blit BG"),
                layout: &self.blit_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(scene_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.output_sampler),
                    },
                ],
            }));
            self.blit_bg_key = Some(scene_view.clone());
        }
        {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: water_output_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let desc = wgpu::RenderPassDescriptor {
                label: Some("Water Blit"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            };
            let mut pass = ctx.begin_render_pass(&desc);
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, self.blit_bg.as_ref().unwrap(), &[]);
            pass.draw(0..3, 0..1);
        }

        // Bake the 512² tileable foam lace/billow map once, then downsample
        // every level explicitly so the distant foam remains stable.
        if !self.foam_baked {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &self.foam_mip_views[0],
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let bake_desc = wgpu::RenderPassDescriptor {
                label: Some("Water Foam Material Bake"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            };
            let mut pass = ctx.begin_render_pass(&bake_desc);
            pass.set_pipeline(&self.foam_bake_pipeline);
            pass.draw(0..3, 0..1);
            drop(pass);

            for mip in 1..FOAM_MIP_LEVELS as usize {
                let source = &self.foam_mip_views[mip - 1];
                let target = &self.foam_mip_views[mip];
                let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Water Foam Mip BG"),
                    layout: &self.foam_mip_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.foam_sampler),
                        },
                    ],
                });
                let attachments = [Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })];
                let mip_desc = wgpu::RenderPassDescriptor {
                    label: Some("Water Foam Mip"),
                    color_attachments: &attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                };
                let mut pass = ctx.begin_render_pass(&mip_desc);
                pass.set_pipeline(&self.foam_mip_pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            self.foam_baked = true;
        }

        // ---- 7. Water surface render -> water_output --------------------------
        if volume_count > 0 {
            if let Some(vols_buf) = water_volumes_buf {
                let gbuffer_normal_view = ctx
                    .registry
                    .get::<helio_core::ViewGroup<'_, 4>>(helio_core::ResourceKey::new(
                        helio_frame_api::gbuffer::GROUP,
                    ))
                    .map(|gb| gb.views[1])
                    .unwrap_or(&self.gbuffer_fallback_view);
                let depth_view = ctx.depth;

                let hiz_nearest_view = match ctx
                    .resource_pool
                    .get_view(helio_frame_api::depth::HIZ_NEAREST)
                {
                    Some(v) => v.clone(),
                    None => {
                        return Err(helio_core::Error::InvalidPassConfig(
                            "WaterSim reflections require hiz_nearest; HiZBuildPass must run \
                             before WaterSim in the graph"
                                .to_string(),
                        ));
                    }
                };

                let atmosphere_buf = ctx
                    .registry
                    .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(
                        helio_frame_api::lighting::sky::ATMOSPHERE_FRAME,
                    ))
                    .unwrap_or(&self.fallback_atmosphere);
                let sky_table_buf = ctx
                    .registry
                    .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(
                        helio_frame_api::lighting::sky::ATMOSPHERE_SKY_TABLE,
                    ))
                    .unwrap_or(&self.fallback_sky_table);

                let sky_view_tex = ctx
                    .registry
                    .get::<&wgpu::TextureView>(helio_core::ResourceKey::new(
                        helio_frame_api::lighting::sky::ATMOSPHERE_SKY_VIEW,
                    ))
                    .unwrap_or(&self.fallback_sky_view);

                let lights_buf = ctx
                    .scene_buffers
                    .get(BufferKey::of(helio_light_api::LIGHT_LIST))
                    .map(|handle| &handle.buffer)
                    .unwrap_or(&self.fallback_lights);

                // Rebuilt whenever anything it binds changes.
                let render_bg = self
                    .render_bg
                    .get_or_create(
                        ctx.device,
                        &wgpu::BindGroupDescriptor {
                            label: Some("Water Render BG"),
                            layout: &self.render_bgl,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: ctx.camera.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: vols_buf.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: wgpu::BindingResource::TextureView(
                                        self.ocean_fft.displacement_view(),
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 3,
                                    resource: wgpu::BindingResource::Sampler(&self.output_sampler),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 4,
                                    resource: wgpu::BindingResource::TextureView(
                                        &self.caustics_near_view,
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 5,
                                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 6,
                                    resource: wgpu::BindingResource::TextureView(scene_view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 7,
                                    resource: self.viewport_buf.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 8,
                                    resource: wgpu::BindingResource::TextureView(depth_view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 9,
                                    resource: wgpu::BindingResource::Sampler(&self.depth_sampler),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 10,
                                    resource: wgpu::BindingResource::TextureView(
                                        gbuffer_normal_view,
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 11,
                                    resource: wgpu::BindingResource::TextureView(&hiz_nearest_view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 12,
                                    resource: wgpu::BindingResource::TextureView(
                                        self.ocean_fft.slopes_view(),
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 13,
                                    resource: wgpu::BindingResource::TextureView(
                                        &self.sim_array_view_a,
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 14,
                                    resource: self.ocean_fft.phase_binding().as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 15,
                                    resource: wgpu::BindingResource::TextureView(&self.shore_view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 16,
                                    resource: self.shore_params.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 17,
                                    resource: wgpu::BindingResource::TextureView(&self.foam_view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 18,
                                    resource: wgpu::BindingResource::Sampler(&self.foam_sampler),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 19,
                                    resource: self
                                        .reflection_capture
                                        .as_ref()
                                        .expect("reflection capture created in prepare")
                                        .camera_buffer()
                                        .as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 20,
                                    resource: wgpu::BindingResource::TextureView(
                                        self.reflection_capture
                                            .as_ref()
                                            .expect("reflection capture created in prepare")
                                            .reflection_view(),
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 23,
                                    resource: wgpu::BindingResource::TextureView(
                                        &self.caustics_deep_view,
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 24,
                                    resource: wgpu::BindingResource::Sampler(
                                        &self.caustics_sampler,
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 25,
                                    resource: atmosphere_buf.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 26,
                                    resource: sky_table_buf.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 27,
                                    resource: wgpu::BindingResource::TextureView(sky_view_tex),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 28,
                                    resource: lights_buf.as_entire_binding(),
                                },
                            ],
                        },
                    )
                    .clone();

                let depth_attachment = wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: None,
                    stencil_ops: None,
                };
                let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                    view: water_output_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })];

                // Ultra-Water is one continuous ocean. The first authored
                // volume supplies its height and optical properties; its old
                // AABB no longer bounds the ocean surface.
                {
                    let mut cmds = ctx.graphics_cmds();
                    let mut pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Water Surface"),
                        color_attachments: &color_attachments,
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &self.water_depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(helio_core::depth::CLEAR),
                                // Kept: `WaterReactivityPass` reads where the
                                // surface drew to tell TSR where the water is.
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&self.surface_pipeline);
                    pass.set_bind_group(0, &render_bg, &[]);
                    pass.set_vertex_buffer(0, self.top_vbuf.slice(..));
                    pass.set_index_buffer(self.top_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..self.top_index_count, 0, 0..1);
                }

                // Shore-breaker puffs and bounded user-submitted impact
                // crowns share the ocean depth test and blend over the
                // completed water surface. Their source events are already
                // rebased and aged in prepare().
                if self.spray_settings.enabled {
                    let spray_bg = self
                        .spray_bg
                        .get_or_create(
                            ctx.device,
                            &wgpu::BindGroupDescriptor {
                                label: Some("Water Spray BG"),
                                layout: &self.spray_bgl,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: ctx.camera.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: self.spray_emitters_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 2,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.shore_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 3,
                                        resource: self.shore_params.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 4,
                                        resource: self.spray_impacts_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 5,
                                        resource: vols_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 6,
                                        resource: wgpu::BindingResource::TextureView(
                                            self.ocean_fft.displacement_view(),
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 7,
                                        resource: wgpu::BindingResource::Sampler(
                                            &self.output_sampler,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 8,
                                        resource: self
                                            .ocean_fft
                                            .phase_binding()
                                            .as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 9,
                                        resource: self.spray_params_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 10,
                                        resource: wgpu::BindingResource::TextureView(depth_view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 11,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.sim_array_view_a,
                                        ),
                                    },
                                ],
                            },
                        )
                        .clone();
                    let attachments = [Some(wgpu::RenderPassColorAttachment {
                        view: water_output_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })];
                    let depth_attachment = wgpu::RenderPassDepthStencilAttachment {
                        view: depth_view,
                        depth_ops: None,
                        stencil_ops: None,
                    };
                    let mut spray_cmds = ctx.graphics_cmds();
                    let mut spray_pass =
                        spray_cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Water Breaker and Impact Spray"),
                            color_attachments: &attachments,
                            depth_stencil_attachment: Some(depth_attachment),
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                    spray_pass.set_bind_group(0, &spray_bg, &[]);
                    if self.spray_live_emitter_count > 0 {
                        spray_pass.set_pipeline(&self.spray_breaker_pipeline);
                        spray_pass.draw(0..6, 0..self.spray_live_emitter_count);
                    }
                    if self.spray_live_impact_count > 0 {
                        spray_pass.set_pipeline(&self.spray_impact_pipeline);
                        spray_pass.draw(0..6, 0..self.spray_live_impact_count * 64);
                    }
                }

                // Sample the same finished textures and shared surface
                // functions used above. The copy/map is bounded and async;
                // if all staging generations are occupied this frame's
                // request batch is dropped without delaying rendering.
                if let Some(vols_buf) = water_volumes_buf {
                    let mut cmds = ctx.graphics_cmds();
                    self.surface_queries.encode(
                        ctx.device,
                        ctx.queue,
                        ctx.world_origin,
                        ctx.frame_num,
                        &mut cmds,
                        vols_buf,
                        self.ocean_fft.displacement_view(),
                        &self.output_sampler,
                        self.ocean_fft.slopes_view(),
                        &self.sim_array_view_a,
                        self.ocean_fft.phase_binding(),
                        &self.shore_view,
                        &self.shore_params,
                        f64::from(self.ocean_fft.time()),
                    );
                }

                // 3. Underwater effect
                {
                    let shafts_bg = self
                        .shafts_bg
                        .get_or_create(
                            ctx.device,
                            &wgpu::BindGroupDescriptor {
                                label: Some("Water Half-Resolution Shafts BG"),
                                layout: &self.shafts_bgl,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: ctx.camera.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: vols_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 2,
                                        resource: wgpu::BindingResource::TextureView(depth_view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 4,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.caustics_near_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 5,
                                        resource: wgpu::BindingResource::Sampler(
                                            &self.caustics_sampler,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 6,
                                        resource: wgpu::BindingResource::TextureView(
                                            self.ocean_fft.displacement_view(),
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 7,
                                        resource: wgpu::BindingResource::Sampler(
                                            &self.output_sampler,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 8,
                                        resource: self
                                            .ocean_fft
                                            .phase_binding()
                                            .as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 9,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.caustics_deep_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 10,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.sim_array_view_a,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 15,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.shore_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 16,
                                        resource: self.shore_params.as_entire_binding(),
                                    },
                                ],
                            },
                        )
                        .clone();
                    let shafts_attachments = [Some(wgpu::RenderPassColorAttachment {
                        view: &self.shafts_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })];
                    let shafts_desc = wgpu::RenderPassDescriptor {
                        label: Some("Water Half-Resolution Caustic Shafts"),
                        color_attachments: &shafts_attachments,
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    };
                    let mut shafts_pass = ctx.begin_render_pass(&shafts_desc);
                    shafts_pass.set_pipeline(&self.shafts_pipeline);
                    shafts_pass.set_bind_group(0, &shafts_bg, &[]);
                    shafts_pass.draw(0..3, 0..1);
                    drop(shafts_pass);

                    let tint_atmosphere = ctx
                        .registry
                        .get::<&wgpu::Buffer>(helio_core::ResourceKey::new(
                            helio_frame_api::lighting::sky::ATMOSPHERE_FRAME,
                        ))
                        .unwrap_or(&self.fallback_atmosphere);
                    let tint_sky_view = ctx
                        .registry
                        .get::<&wgpu::TextureView>(helio_core::ResourceKey::new(
                            helio_frame_api::lighting::sky::ATMOSPHERE_SKY_VIEW,
                        ))
                        .unwrap_or(&self.fallback_sky_view);

                    // Rebuilt whenever anything it binds changes.
                    let tint_bg = self
                        .underwater_tint_bg
                        .get_or_create(
                            ctx.device,
                            &wgpu::BindGroupDescriptor {
                                label: Some("Water Underwater Tint BG"),
                                layout: &self.underwater_tint_bgl,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: ctx.camera.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: vols_buf.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 2,
                                        resource: wgpu::BindingResource::TextureView(
                                            water_output_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 3,
                                        resource: wgpu::BindingResource::Sampler(
                                            &self.output_sampler,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 4,
                                        resource: wgpu::BindingResource::TextureView(depth_view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 5,
                                        resource: wgpu::BindingResource::Sampler(
                                            &self.depth_sampler,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 6,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.sim_array_view_a,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 7,
                                        resource: wgpu::BindingResource::Sampler(
                                            &self.output_sampler,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 9,
                                        resource: wgpu::BindingResource::TextureView(
                                            self.ocean_fft.displacement_view(),
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 10,
                                        resource: self
                                            .ocean_fft
                                            .phase_binding()
                                            .as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 11,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.shafts_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 15,
                                        resource: wgpu::BindingResource::TextureView(
                                            &self.shore_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 16,
                                        resource: self.shore_params.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 20,
                                        resource: tint_atmosphere.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 21,
                                        resource: wgpu::BindingResource::TextureView(tint_sky_view),
                                    },
                                ],
                            },
                        )
                        .clone();
                    let tint_attachments = [Some(wgpu::RenderPassColorAttachment {
                        view: &self.tint_scratch_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })];
                    let mut cmds = ctx.graphics_cmds();
                    let mut tint_pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Water Underwater Tint"),
                        color_attachments: &tint_attachments,
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    tint_pass.set_pipeline(&self.underwater_tint_pipeline);
                    tint_pass.set_bind_group(0, &tint_bg, &[]);
                    tint_pass.draw(0..3, 0..1);
                    drop(tint_pass);

                    let scratch_blit_bg = self.tint_blit_bg.get_or_insert_with(|| {
                        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Water Tint Blit BG"),
                            layout: &self.blit_bgl,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(
                                        &self.tint_scratch_view,
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&self.output_sampler),
                                },
                            ],
                        })
                    });
                    let blit_attachments = [Some(wgpu::RenderPassColorAttachment {
                        view: water_output_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })];
                    let mut blit_pass = cmds.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Water Tint Blit Back"),
                        color_attachments: &blit_attachments,
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    blit_pass.set_pipeline(&self.blit_pipeline);
                    blit_pass.set_bind_group(0, &*scratch_blit_bg, &[]);
                    blit_pass.draw(0..3, 0..1);
                }
            }
        }

        Ok(())
    }
}
