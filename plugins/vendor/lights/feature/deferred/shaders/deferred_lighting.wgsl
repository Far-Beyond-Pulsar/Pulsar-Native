//!use helio_prelude
//!use pbr_eval
//!use atmosphere
//!use atmosphere_sky_table
//!use frame_indirect_specular
//!use light_list
//!use shadow_atlas

//! Deferred lighting pass.
//!
//! Runs as a fullscreen triangle (no vertex buffer) over the G-buffer written
//! by the gbuffer pass.  Performs the full Cook-Torrance PBR evaluation,
//! PCF shadow sampling, GI probes, environment IBL and tonemapping
//! in a single screen-space draw — O(pixels) instead of O(pixels × lights).
//!
//! Feature override constants injected by PipelineCache:
//!   override ENABLE_LIGHTING:   bool = false;
//!   override LIGHT_COUNT:       u32  = 0u;
//!   override ENABLE_SHADOWS:    bool = false;
//!   override MAX_SHADOW_LIGHTS: u32  = 0u;

// ── Uniforms ──────────────────────────────────────────────────────────────────

const ENABLE_LIGHTING: bool = true;
const ENABLE_SHADOWS: bool = true;
const MAX_SHADOW_LIGHTS: u32 = 42u;

struct Globals {
    frame:             u32,
    delta_time:        f32,
    light_count:       u32,
    ambient_intensity: f32,
    ambient_color:     vec4<f32>,
    csm_splits:        vec4<f32>,
    debug_mode:        u32,
    num_tiles_x:       u32,
    // Number of entries in reflection_captures. Zero skips capture blending
    // entirely and falls through to the skylight cubemap (layer 0).
    reflection_capture_count: u32,
    // 0 where the target does not support reflections (Apple platforms). The
    // cube array, SSR and planar composites are all skipped, leaving indirect
    // specular at zero — direct light and ambient still apply. SsrPass
    // and PlanarReflectionPass are not in the graph at all on those targets.
    enable_reflections: u32,
    // 0 disables the environment-cubemap indirect specular term specifically.
    // The cubemap is the base reflection layer; SSR and planar only composite
    // over it. Scalars, not vec3<u32>: a vec3 would align the tail to 16 and
    // desync the struct from its Rust mirror.
    enable_env_reflections: u32,
    measured_lighting_ms: f32,
    pixel_count: u32,
    _pad_0: u32,
}

/// Per-cascade shadow configuration (16 bytes, matches libhelio::CascadeConfig)
struct CascadeConfig {
    split_distance:   f32,  // Far plane distance (meters)
    depth_bias:       f32,  // Base depth bias
    filter_radius:    f32,  // PCF filter radius (texels)
    pcss_light_size:  f32,  // PCSS light size (meters, 0.0 = disable)
}

/// Global shadow configuration (96 bytes, matches libhelio::ShadowConfig)
struct ShadowConfig {
    cascades:             array<CascadeConfig, 4>,  // 64 bytes
    enable_pcss:          u32,                      // Global PCSS toggle
    pcss_blocker_samples: u32,                      // Blocker search samples
    pcss_filter_samples:  u32,                      // PCSS filter samples
    pcf_sample_count:     u32,                      // Standard PCF sample count (4/8/12/16)
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var <uniform> globals:       Globals;
@group(0) @binding(7) var <uniform> shadow_config: ShadowConfig;

// Shadow-caster counts per atlas, written by ObjectBatch on the GPU this
// frame. An atlas with no casters holds only the cleared far plane (1.0), so
// every LessEqual comparison against it is 1 and every depth read is 1.0:
// skipping it cannot change min(dynamic, static). Scenes whose casters are
// all static (or all movable) then take one atlas sample per tap, not two.
struct ShadowCasterCounts {
    static_casters:       u32,
    movable_casters:      u32,
    transmissive_casters: u32,
    _pad:                 u32,
}
@group(0) @binding(8) var<storage, read> shadow_caster_counts: ShadowCasterCounts;

// The atmosphere resolved this frame (the frame's `sky` contract), or
// a zeroed frame (planet.w = 0) without one. A uniform: the fragment stage
// is at its storage-buffer limit. The sun reaches surfaces through the air
// between them and space, and the sky lights them; the air between them and
// the eye is the atmosphere composite's.
@group(0) @binding(9) var<uniform> atmosphere: AtmosphereFrame;
// The sky over each surface (`AtmosphereFrame` docs, `SKY_TABLE_*`).
@group(0) @binding(10) var<uniform> atmosphere_sky_table: array<vec4<f32>, 2304>;

// The frame's view remaps (`helio_frame_api::lighting::VIEW_REMAPS`): per
// chain, the matrix taking a point of the host world into the world the
// chain's pixels show (`view_remap_of` tags them). Such a pixel is drawn at a
// transformed copy of its content, so it is lit where the content really is:
// with that world's lights, sun and sky, not the host's. A uniform for the
// same reason as the atmosphere: the fragment stage is at its storage-buffer
// limit.
@group(0) @binding(11) var<uniform> view_remaps: array<mat4x4<f32>, 256>;

fn complexity_color(value: f32) -> vec3<f32> {
    let t = clamp(value, 0.0, 1.0);
    if t < 0.25 { return mix(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 1.0), t * 4.0); }
    if t < 0.5 { return mix(vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), (t - 0.25) * 4.0); }
    if t < 0.75 { return mix(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 1.0, 0.0), (t - 0.5) * 4.0); }
    return mix(vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), (t - 0.75) * 4.0);
}

// Fixed absolute scale in microseconds per pixel. Unlike a normalized
// complexity view, one color always represents the same measured duration.
fn time_cost_color(us_per_pixel: f32) -> vec3<f32> {
    let t = max(us_per_pixel, 0.0);
    if t < 0.01 { return mix(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 1.0), t / 0.01); }
    if t < 0.03 { return mix(vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), (t - 0.01) / 0.02); }
    if t < 0.1 { return mix(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 1.0, 0.0), (t - 0.03) / 0.07); }
    if t < 0.5 { return mix(vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), (t - 0.1) / 0.4); }
    return vec3<f32>(1.0, 0.0, 0.0);
}

// The sky's irradiance / pi at a surface `from_camera` (km, relative to the
// camera) with normal `n`: its own sky, by its altitude and sun angle, not
// the eye's (`sky_table_kernel`). Above the atmosphere the planet's glow
// falls with its solid angle.
fn atmosphere_local_irradiance(from_camera: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    if atmosphere.planet.w < 0.5 { return vec3<f32>(0.0); }
    let bottom = atmosphere.params.bottom_radius;
    let top = atmosphere.params.top_radius;
    let pos = from_camera - atmosphere.planet.xyz;
    let r = max(length(pos), bottom);
    let up = normalize(pos);
    let sun = atmosphere.sun.xyz;
    let mu_s = dot(up, sun);
    var x = sun - up * mu_s;
    if dot(x, x) < 1e-8 { x = cross(up, select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(up.x) > 0.9)); }
    x = normalize(x);
    let z = cross(x, up);
    let l = vec3<f32>(dot(n, x), dot(n, up), dot(n, z));
    let height = clamp((r - bottom) / (top - bottom), 0.0, 1.0);
    let fr = sqrt(height) * f32(SKY_TABLE_R - 1u);
    let fm = clamp((mu_s - SKY_TABLE_MU_LO) / (1.0 - SKY_TABLE_MU_LO), 0.0, 1.0) * f32(SKY_TABLE_MU - 1u);
    let r0 = min(u32(fr), SKY_TABLE_R - 2u);
    let m0 = min(u32(fm), SKY_TABLE_MU - 2u);
    let w = vec2<f32>(fr - f32(r0), fm - f32(m0));
    let basis = array<f32, 6>(0.282095, 0.488603 * l.y, 0.488603 * l.x, 1.092548 * l.x * l.y, 0.315392 * (3.0 * l.z * l.z - 1.0), 0.546274 * (l.x * l.x - l.y * l.y));
    var e = vec3<f32>(0.0);
    for (var k = 0u; k < SKY_TABLE_COEFFS; k++) {
        let a = (r0 * SKY_TABLE_MU + m0) * SKY_TABLE_COEFFS + k;
        let b = a + SKY_TABLE_COEFFS;
        let c = a + SKY_TABLE_MU * SKY_TABLE_COEFFS;
        let d = c + SKY_TABLE_COEFFS;
        let v = mix(mix(atmosphere_sky_table[a].rgb, atmosphere_sky_table[b].rgb, w.y), mix(atmosphere_sky_table[c].rgb, atmosphere_sky_table[d].rgb, w.y), w.x);
        e += v * basis[k];
    }
    let above = length(pos) / top;
    let falloff = select(1.0, 1.0 / (above * above), above > 1.0);
    return max(e, vec3<f32>(0.0)) * atmosphere.sun_illuminance.rgb * falloff;
}

// min(dynamic, static) shadow comparison, skipping an atlas with no casters.
fn compare_shadow_atlases(uv: vec2<f32>, layer: u32, depth_ref: f32) -> f32 {
    var lit = 1.0;
    if shadow_caster_counts.movable_casters != 0u {
        lit = budget_compare_dynamic(uv, u32(i32(layer)), depth_ref);
    }
    if shadow_caster_counts.static_casters != 0u {
        lit = min(lit, budget_compare_static(uv, u32(i32(layer)), depth_ref));
    }
    return lit;
}

// min(dynamic, static) occluder depth, skipping an atlas with no casters.
fn load_shadow_occluder_depth(pixel: vec2<i32>, layer: u32) -> f32 {
    var depth = 1.0;
    if shadow_caster_counts.movable_casters != 0u {
        depth = budget_depth_dynamic(pixel, u32(i32(layer)));
    }
    if shadow_caster_counts.static_casters != 0u {
        depth = min(depth, budget_depth_static(pixel, u32(i32(layer))));
    }
    return depth;
}

// Group 1 – G-buffer inputs (read-only, textureLoad)
@group(1) @binding(0) var gbuf_albedo:   texture_2d<f32>;       // Rgba8Unorm   albedo.rgb + alpha
@group(1) @binding(1) var gbuf_normal:   texture_2d<f32>;       // Rgba16Float  world-space normal
@group(1) @binding(2) var gbuf_orm:      texture_2d<f32>;       // Rgba8Unorm   AO, roughness, metallic
@group(1) @binding(3) var gbuf_emissive: texture_2d<f32>;       // Rgba16Float  pre-multiplied emissive
@group(1) @binding(4) var gbuf_depth:    texture_depth_2d;      // Depth32Float
// R8Unorm screen-space AO (SSAO or pre-baked equivalent). 1.0 = fully lit, 0.0 = fully occluded.
// Bound to a 1×1 white fallback texture when neither SSAO nor baked AO is available.
@group(1) @binding(5) var screen_ao:     texture_2d<f32>;
@group(1) @binding(6) var screen_ao_samp: sampler;
// Lightmap UVs from GBuffer (Rg16Float, contains atlas UV coordinates for lightmap lookup)
@group(1) @binding(7) var gbuf_lightmap_uv: texture_2d<f32>;
// SSS data (Rgba16Float): subsurface_color.rgb, subsurface_radius
@group(1) @binding(8) var gbuf_sss: texture_2d<f32>;
// Extra surface data (Rgba16Float): roughness_aniso_x, roughness_aniso_y, aniso_rotation, numeric surface_flags
@group(1) @binding(9) var gbuf_extra: texture_2d<f32>;
// The sun's visibility at each pixel (the frame's `sun_visibility`: media's
// shadow times the occluders' `directional_visibility`), x the
// visibility and yzw the sun's direction it holds for; zero where none is
// published.
@group(1) @binding(10) var sun_visibility: texture_2d<f32>;

// Group 2 – lights, shadows, environment (same as forward geometry pass)
@group(2) @binding(0) var <storage, read> lights:          array<GpuLight>;
@group(2) @binding(1)  var shadow_atlas:         texture_depth_2d_array;  // Dynamic (Movable objects)
@group(2) @binding(11) var static_shadow_atlas:  texture_depth_2d_array;  // Static (cached forever)
@group(2) @binding(2) var shadow_sampler: sampler_comparison;
// Reflection cube array. Layer i is capture i's pre-filtered cubemap; mip m
// corresponds to a GGX lobe of increasing roughness. Layer 0 doubles as the
// skylight fallback for surfaces no capture reaches.
@group(2) @binding(3) var env_cube:       texture_cube_array<f32>;
@group(2) @binding(4) var <storage, read> shadow_matrices: array<LightMatrix>;
@group(2) @binding(6) var env_sampler:    sampler;
@group(2) @binding(7) var shadow_depth_sampler: sampler;  // Non-comparison sampler for PCSS blocker search
// Radiance media add at each pixel (`helio_frame_api::lighting::TRANSMISSION`:
// light water focuses, say), added to the indirect term. 1x1 black without.
@group(2) @binding(8) var transmission: texture_2d<f32>;
// Baked lightmap atlas (Rgba16Float, pre-baked indirect illumination for Static geometry)
@group(2) @binding(12) var baked_lightmap: texture_2d<f32>;
@group(2) @binding(13) var baked_lightmap_sampler: sampler;
// The frame's indirect-specular layers (`helio_frame_api::lighting::
// INDIRECT_SPECULAR_LAYERS`), composited over the environment in order: rgb
// the reflected radiance before Fresnel, a its coverage. 1x1 black without.
@group(2) @binding(14) var specular_layer_0: texture_2d<f32>;
@group(2) @binding(16) var specular_layer_1: texture_2d<f32>;
@group(2) @binding(17) var planar_sampler: sampler;
// IES light profile textures (R8Unorm, 256×256 per slice, C type angular distribution)
@group(2) @binding(18) var ies_textures: texture_2d_array<f32>;
@group(2) @binding(19) var ies_sampler: sampler;
// Coloured transmittance of translucent shadow casters (stained glass), stored
// as complements: rgb = 1 - T, a = 1 - light-space depth of the nearest pane.
// Zero is unfiltered. See helio-pass-shadow.
@group(2) @binding(20) var shadow_transmittance: texture_2d_array<f32>;
@group(2) @binding(21) var transmittance_sampler: sampler;

// Reflection captures, uploaded sorted by influence volume, largest first.
// The blend below runs front-to-back and saturates, so ordering is what lets a
// small capture override the larger one it sits inside.
struct GpuReflectionCapture {
    position_radius:    vec4<f32>,   // xyz = world position, w = influence radius
    extents_transition: vec4<f32>,   // xyz = local half-extents (box), w = transition distance
    world_to_local:     mat4x4<f32>, // box parallax; identity for spheres
    cubemap_index:      i32,         // cube array layer, -1 = no cubemap
    shape:              u32,         // 0 = sphere, 1 = box
    mobility:           u32,         // 0 = static, 1 = dynamic
    brightness:         f32,
}
@group(2) @binding(15) var<storage, read> reflection_captures: array<GpuReflectionCapture>;

const CAPTURE_SHAPE_SPHERE: u32 = 0u;
const CAPTURE_SHAPE_BOX:    u32 = 1u;

// ENV_MAX_LOD imported from pbr_eval.wgsl

// Group 3 – tiled light culling results (written by LightCullPass each frame)
const TILE_SIZE:          u32 = 16u;
const MAX_LIGHTS_PER_TILE: u32 = 64u;
// A remapped pixel walks the scene's lights instead of its tile's list (the list
// is of the host world's view); only its footprint pays.
const REMAPPED_MAX_LIGHTS: u32 = 256u;
@group(3) @binding(0) var<storage, read> tile_light_lists:  array<u32>;
@group(3) @binding(1) var<storage, read> tile_light_counts: array<u32>;
// The voxel terrain's shadows of local lights (its denoiser's): three words
// a pixel (rows of the frame): the tile slots of up to four lights it
// tracks (a byte each, 0xff none), their visibility (a byte each), and for
// the other slots (the first 32) bit i whether the pixel sees the tile
// list's light i. One word (or none past its end): every light seen.
@group(3) @binding(2) var<storage, read> local_visibility: array<u32>;

// How much of tile slot `slot`'s light the terrain lets reach the pixel
// whose words start at `at`.
fn terrain_light_visibility(at: u32, slot: u32) -> f32 {
    if arrayLength(&local_visibility) <= 3u || at + 2u >= arrayLength(&local_visibility) { return 1.0; }
    let tracked = local_visibility[at];
    for (var k = 0u; k < 4u; k++) {
        if ((tracked >> (8u * k)) & 0xffu) == slot {
            return f32((local_visibility[at + 1u] >> (8u * k)) & 0xffu) / 255.0;
        }
    }
    return f32((local_visibility[at + 2u] >> slot) & 1u);
}
// The frame's indirect diffuse / pi a pixel (`indirect_diffuse`): rgb and
// how much its publisher holds the point, half floats. One entry (none
// held): nothing publishes it.
@group(3) @binding(3) var<storage, read> gi_irradiance: array<vec2<u32>>;
// cluster bindings removed - GPU-driven architecture

// Cluster constants removed - GPU-driven architecture

// ── Fullscreen-triangle vertex shader ────────────────────────────────────────

struct VSOut {
    @builtin(position) clip_pos: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    // Three vertices covering the entire NDC square.
    // No vertex buffer required — just draw(3, 1, 0, 0).
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: VSOut;
    out.clip_pos = vec4<f32>(pos[vi], 0.0, 1.0);
    return out;
}

// ── Shadow helpers ────────────────────────────────────────────────────────────

const ATLAS_SIZE: f32 = 1024.0;

// Vogel disk sampling - blue-noise-like spiral pattern for high-quality PCF
fn vogel_disk_sample(sample_idx: u32, sample_count: u32, theta: f32) -> vec2<f32> {
    let GOLDEN_ANGLE = 2.39996323;  // 2π / φ² (golden angle in radians)
    let r = sqrt(f32(sample_idx) + 0.5) / sqrt(f32(sample_count));
    let angle = f32(sample_idx) * GOLDEN_ANGLE + theta;
    return vec2<f32>(cos(angle), sin(angle)) * r;
}

// Per-pixel hash for PCF rotation (reduces banding artifacts)
fn hash22(p: vec2<f32>) -> f32 {
    let p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    let d = dot(p3, vec3<f32>(p3.y + 33.33, p3.z + 33.33, p3.x + 33.33));
    return fract((p3.x + p3.y) * d);
}

// Normal-offset bias constant (world-space units).
// Shifts the shadow query point along the surface normal before projecting into
// light space, eliminating self-shadowing without any visible surface gap.
// This is the same technique used by UE4 ("Normal Shadow Bias") and Unity HDRP.
const NORMAL_OFFSET_SCALE: f32 = 0.01;

// High-quality PCF shadow sampling with Vogel disk pattern.
// world_pos must already have normal-offset applied (call shadow_factor, not this directly).
// Adaptive sample count: cascade_idx determines quality (distant cascades use fewer samples).
fn sample_cascade_shadow(
    layer: u32,
    cascade_idx: u32,
    cascade_scale: f32,
    world_pos: vec3<f32>,
    frag_coord: vec2<f32>,
    frame: u32
) -> f32 {
    let light_clip = shadow_matrices[layer].mat * vec4<f32>(world_pos, 1.0);
    if light_clip.w <= 0.0 { return 1.0; }

    let ndc       = light_clip.xyz / light_clip.w;
    let shadow_uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);

    if any(shadow_uv < vec2<f32>(0.0)) || any(shadow_uv > vec2<f32>(1.0))
       || ndc.z < 0.0 || ndc.z > 1.0 {
        return 1.0;
    }

    let receiver_depth = max(0.0, ndc.z - shadow_config.cascades[cascade_idx].depth_bias);

    let filter_radius = (2.0 / budget_resolution(layer)) * cascade_scale;

    // Per-pixel rotation to break up banding (stable hash — no frame counter)
    let theta = hash22(frag_coord) * 6.28318530718;

    // OPTIMIZATION: Adaptive PCF sample count based on cascade distance
    // Distant cascades are naturally blurrier and need fewer samples for good quality.
    // This provides 20-40% shadow performance improvement with minimal visual impact.
    let base_count = shadow_config.pcf_sample_count;
    var pcf_count: u32;
    switch cascade_idx {
        case 0u: { pcf_count = base_count; }                           // Closest: full quality
        case 1u: { pcf_count = max(base_count * 3u / 4u, 4u); }       // 75% samples
        case 2u: { pcf_count = max(base_count / 2u, 4u); }            // 50% samples
        default: { pcf_count = max(base_count / 4u, 4u); }            // Farthest: 25% samples (min 4)
    }

    var lit_sum = 0.0;
    for (var i = 0u; i < pcf_count; i++) {
        let offset = vogel_disk_sample(i, pcf_count, theta) * filter_radius;
        // Minimum of both atlases — pixel is lit only if neither occludes it.
        // This is the Unreal-style static/dynamic shadow combine for mixed mobility scenes.
        lit_sum += compare_shadow_atlases(shadow_uv + offset, layer, receiver_depth);
    }

    return lit_sum / f32(pcf_count);
}

// ── PCSS (Contact-Hardening Shadows) ─────────────────────────────────────────

// Step 1: Blocker search - find average occluder depth in light-space
fn pcss_blocker_search(
    layer: u32,
    shadow_uv: vec2<f32>,
    receiver_depth: f32,
    search_radius: f32,
    blocker_samples: u32,
    theta: f32
) -> vec2<f32> {  // Returns (avg_blocker_depth, num_blockers)
    var blocker_sum = 0.0;
    var blocker_count = 0.0;

    for (var i = 0u; i < blocker_samples; i++) {
        let offset = vogel_disk_sample(i, blocker_samples, theta) * search_radius;
        let sample_uv = shadow_uv + offset;

        // Convert UV to pixel coordinates for textureLoad (no filtering needed for blocker search)
        let pixel_coord = vec2<i32>(sample_uv * budget_resolution(layer));

        // Bounds check to prevent out-of-range access
        if any(pixel_coord < vec2<i32>(0)) || any(pixel_coord >= vec2<i32>(i32(budget_resolution(layer)))) {
            continue;
        }

        // Sample actual depth value (not comparison) for blocker detection.
        // Use min of dynamic and static atlases — the closer occluder is the true blocker.
        let occluder_depth = load_shadow_occluder_depth(pixel_coord, layer);

        if occluder_depth < receiver_depth - 0.0001 {  // Is blocker
            blocker_sum += occluder_depth;
            blocker_count += 1.0;
        }
    }

    if blocker_count < 0.5 {
        return vec2<f32>(0.0, 0.0);  // Fully lit (no blockers found)
    }

    return vec2<f32>(blocker_sum / blocker_count, blocker_count);
}

// Step 2: Compute penumbra size based on blocker-receiver distance
fn pcss_penumbra_size(
    receiver_depth: f32,
    avg_blocker_depth: f32,
    light_size: f32
) -> f32 {
    // Classic PCSS formula: penumbra_width = (d_receiver - d_blocker) / d_blocker * light_width
    // Contact shadows (blocker_depth ≈ receiver_depth) → small penumbra (sharp)
    // Distant shadows (receiver_depth >> blocker_depth) → large penumbra (soft)
    return (receiver_depth - avg_blocker_depth) / max(avg_blocker_depth, 0.001) * light_size;
}

// Step 3: Full PCSS shadow sampling (blocker search + variable-kernel PCF).
// world_pos must already have normal-offset applied (call shadow_factor, not this directly).
fn sample_cascade_shadow_pcss(
    layer: u32,
    cascade_idx: u32,
    world_pos: vec3<f32>,
    frag_coord: vec2<f32>,
    frame: u32
) -> f32 {
    let config = shadow_config.cascades[cascade_idx];
    let light_clip = shadow_matrices[layer].mat * vec4<f32>(world_pos, 1.0);
    if light_clip.w <= 0.0 { return 1.0; }

    let ndc = light_clip.xyz / light_clip.w;
    let shadow_uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);

    if any(shadow_uv < vec2<f32>(0.0)) || any(shadow_uv > vec2<f32>(1.0))
       || ndc.z < 0.0 || ndc.z > 1.0 {
        return 1.0;
    }

    // Keep blocker detection and the penumbra estimate unbiased; apply the
    // per-cascade depth bias only to the final visibility comparisons.
    let blocker_receiver_depth = ndc.z;
    let compare_depth = max(0.0, ndc.z - config.depth_bias);
    let theta = hash22(frag_coord) * 6.28318530718;

    // Step 1: Blocker search (average occluder depth)
    // Uses unbiased depth so nearby occluders are correctly identified.
    let search_radius = config.pcss_light_size / budget_resolution(layer);
    let blocker = pcss_blocker_search(layer, shadow_uv, blocker_receiver_depth, search_radius,
                                       shadow_config.pcss_blocker_samples, theta);

    if blocker.y < 0.5 {
        return 1.0;  // No blockers - fully lit (early exit optimization)
    }

    // Step 2: Compute penumbra size (distance-based filter width)
    let penumbra = pcss_penumbra_size(blocker_receiver_depth, blocker.x, config.pcss_light_size);
    let filter_radius = clamp(penumbra / budget_resolution(layer),
                                config.filter_radius / budget_resolution(layer),
                                config.filter_radius * 3.0 / budget_resolution(layer));

    // Step 3: Variable-kernel PCF (filter size scales with penumbra)
    var lit_sum = 0.0;
    for (var i = 0u; i < shadow_config.pcss_filter_samples; i++) {
        let offset = vogel_disk_sample(i, shadow_config.pcss_filter_samples, theta) * filter_radius;
        // Combine dynamic and static atlases: shadowed by either
        lit_sum += compare_shadow_atlases(shadow_uv + offset, layer, compare_depth);
    }

    return lit_sum / f32(shadow_config.pcss_filter_samples);
}

// Colour a light by the translucent casters between it and the receiver.
// Uses the same face and normal offset as `shadow_factor`; directional lights
// take the nearest cascade (the tint is low-frequency, so no cascade blend).
// Tint of the stained-glass panes a receiver lies behind. Each of the four
// nearest texels is depth-tested on its own, then the results are weighted
// bilinearly. Bilinearly filtering the pane depth (alpha) first and testing
// once failed the test along every pane edge wherever a neighbouring texel
// held no pane: an untinted rim and stair-stepped edges, most visible in the
// coarse cascades, where a half-resolution texel covers several centimetres.
fn glass_tint(uv:vec2f, layer:i32, receiver:f32)->vec3f {
    return budget_glass_tint(shadow_transmittance,uv,layer,receiver);
}

fn light_transmittance(light_idx: u32, world_pos: vec3<f32>, N: vec3<f32>) -> vec3<f32> {
    if !ENABLE_SHADOWS { return vec3<f32>(1.0); }
    let light = lights[light_idx];
    if light.shadow_index == 4294967295u { return vec3<f32>(1.0); }
    if light.shadow_index >= arrayLength(&shadow_matrices) { return vec3<f32>(1.0); }

    var light_dir: vec3<f32>;
    if light.light_type == 0u {
        light_dir = normalize(-light.direction_outer.xyz);
    } else {
        light_dir = normalize(light.position_range.xyz - world_pos);
    }
    let NdotL      = max(dot(N, light_dir), 0.0);
    let biased_pos = world_pos + N * NORMAL_OFFSET_SCALE * (1.0 - NdotL);

    var layer = light.shadow_index;
    if light.light_type == 1u {
        layer = light.shadow_index + point_light_face(biased_pos - light.position_range.xyz);
    } else if light.light_type == 0u {
        let dist   = length(world_pos - cameras[0].position_near.xyz);
        let splits = globals.csm_splits;
        var cascade = 3u;
        if dist < splits.x { cascade = 0u; }
        else if dist < splits.y { cascade = 1u; }
        else if dist < splits.z { cascade = 2u; }
        layer = light.shadow_index + cascade;
    }

    let light_clip = shadow_matrices[layer].mat * vec4<f32>(biased_pos, 1.0);
    if light_clip.w <= 0.0 { return vec3<f32>(1.0); }
    let ndc = light_clip.xyz / light_clip.w;
    let uv  = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
        return vec3<f32>(1.0);
    }
    // Only receivers behind the nearest pane are filtered.
    return glass_tint(uv, i32(layer), 1.0 - ndc.z);
}

fn raster_shadow_factor(light_idx: u32, world_pos: vec3<f32>, N: vec3<f32>, frag_coord: vec2<f32>, frame: u32) -> f32 {
    if !ENABLE_SHADOWS { return 1.0; }
    // MAX_SHADOW_LIGHTS bounds the atlas caster slot (shadow_index / 6), not
    // the light's row. SceneDB rows are entity indices, so a row bound silently
    // dropped shadows for every light spawned after the 42nd entity.

    let light = lights[light_idx];

    // Check if this light actually casts shadows (shadow_index != u32::MAX)
    if light.shadow_index == 4294967295u { return 1.0; }
    if light.shadow_index >= arrayLength(&shadow_matrices) { return 1.0; }

    // Normal-offset: shift the world-space query point along the surface normal
    // toward the light before projecting.  This eliminates self-shadowing caused
    // by floating-point depth quantization, without the visible gap from a
    // constant depth-offset.  Scale by (1 - NdotL) so face-on surfaces (no
    // self-shadow risk) get near-zero offset while grazing surfaces get the full
    // amount — exactly matching the UE4 / Unity HDRP normal-bias approach.
    var light_dir: vec3<f32>;
    if light.light_type == 0u {
        light_dir = normalize(-light.direction_outer.xyz);
    } else {
        light_dir = normalize(light.position_range.xyz - world_pos);
    }
    let NdotL         = max(dot(N, light_dir), 0.0);
    let normal_offset = N * NORMAL_OFFSET_SCALE * (1.0 - NdotL);
    let biased_pos    = world_pos + normal_offset;

    var layer: u32;
    if light.light_type > 0u && light.light_type < 2u {  // Point light (type 1)
        let to_frag = biased_pos - light.position_range.xyz;
        layer = light.shadow_index + point_light_face(to_frag);
        return sample_cascade_shadow(layer, 0u, 1.0, biased_pos, frag_coord, frame);
    } else if light.light_type == 0u {  // Directional light (type 0)
        let dist = length(world_pos - cameras[0].position_near.xyz);
        let splits = globals.csm_splits;
        
        // Determine cascades and blend factor
        var cascade_a = 3u;
        var cascade_b = 3u;
        var blend = 0.0;
        
        const BLEND_ZONE = 0.1;  // 10% blend zone around boundaries
        
        if dist < splits.x * (1.0 - BLEND_ZONE / 2.0) {
            cascade_a = 0u;
        } else if dist < splits.x * (1.0 + BLEND_ZONE / 2.0) {
            // Blend zone between cascade 0 and 1
            cascade_a = 0u;
            cascade_b = 1u;
            blend = smoothstep(
                splits.x * (1.0 - BLEND_ZONE / 2.0),
                splits.x * (1.0 + BLEND_ZONE / 2.0),
                dist
            );
        } else if dist < splits.y * (1.0 - BLEND_ZONE / 2.0) {
            cascade_a = 1u;
        } else if dist < splits.y * (1.0 + BLEND_ZONE / 2.0) {
            // Blend zone between cascade 1 and 2
            cascade_a = 1u;
            cascade_b = 2u;
            blend = smoothstep(
                splits.y * (1.0 - BLEND_ZONE / 2.0),
                splits.y * (1.0 + BLEND_ZONE / 2.0),
                dist
            );
        } else if dist < splits.z * (1.0 - BLEND_ZONE / 2.0) {
            cascade_a = 2u;
        } else if dist < splits.z * (1.0 + BLEND_ZONE / 2.0) {
            // Blend zone between cascade 2 and 3
            cascade_a = 2u;
            cascade_b = 3u;
            blend = smoothstep(
                splits.z * (1.0 - BLEND_ZONE / 2.0),
                splits.z * (1.0 + BLEND_ZONE / 2.0),
                dist
            );
        } else {
            cascade_a = 3u;
        }
        
        // Use PCSS if enabled and light size is non-zero for this cascade
        let use_pcss = shadow_config.enable_pcss != 0u && shadow_config.cascades[cascade_a].pcss_light_size > 0.0;

        let layer_a = light.shadow_index + cascade_a;
        var shadow_a: f32;
        if use_pcss {
            shadow_a = sample_cascade_shadow_pcss(layer_a, cascade_a, biased_pos, frag_coord, frame);
        } else {
            let cascade_scale_a = 1.0 + f32(cascade_a) * 1.5;
            shadow_a = sample_cascade_shadow(layer_a, cascade_a, cascade_scale_a, biased_pos, frag_coord, frame);
        }

        // If no blending needed, return immediately
        if blend <= 0.001 { return shadow_a; }

        // Blend between cascades if needed
        if cascade_b != cascade_a && blend > 0.001 {
            let use_pcss_b = shadow_config.enable_pcss != 0u && shadow_config.cascades[cascade_b].pcss_light_size > 0.0;
            let layer_b = light.shadow_index + cascade_b;
            var shadow_b: f32;
            if use_pcss_b {
                shadow_b = sample_cascade_shadow_pcss(layer_b, cascade_b, biased_pos, frag_coord, frame);
            } else {
                let cascade_scale_b = 1.0 + f32(cascade_b) * 1.5;
                shadow_b = sample_cascade_shadow(layer_b, cascade_b, cascade_scale_b, biased_pos, frag_coord, frame);
            }
            return mix(shadow_a, shadow_b, blend);
        }

        return shadow_a;
    } else {
        // Spot light (type 2)
        layer = light.shadow_index;
        return sample_cascade_shadow(layer, 0u, 1.0, biased_pos, frag_coord, frame);
    }
}

// ── Surface flags (read from gbuf_extra.a) ──────────────────────────────────

// SURFACE_FLAG_* and ENV_MAX_LOD imported from pbr_eval.wgsl

// env_brdf_approx imported from pbr_eval.wgsl

// ── Reflection captures ──────────────────────────────────────────────────────

// Anchor the reflection ray to the capture's sphere so the cubemap reads as a
// room at a real distance rather than an infinitely distant backdrop.
fn parallax_sphere(P: vec3<f32>, R: vec3<f32>, center: vec3<f32>, radius: f32) -> vec3<f32> {
    let oc = P - center;
    let b  = dot(oc, R);
    let c  = dot(oc, oc) - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return R;
    }
    let t = -b + sqrt(disc);  // far hit: the ray leaves the volume here
    if t <= 0.0 {
        return R;
    }
    return (P + R * t) - center;
}

// Box parallax against the capture's oriented box.
//
// The slab test runs in capture-local space, but the resulting ray parameter t
// is invariant under the affine world→local transform, so the hit point can be
// reconstructed in world space as P + R*t. That keeps this to a single stored
// matrix — reconstructing the hit in local space instead would need the inverse
// transform as well, just to rotate the direction back.
fn parallax_box(P: vec3<f32>, R: vec3<f32>, cap: GpuReflectionCapture) -> vec3<f32> {
    let pl = (cap.world_to_local * vec4<f32>(P, 1.0)).xyz;
    let rl = (cap.world_to_local * vec4<f32>(R, 0.0)).xyz;
    let e  = cap.extents_transition.xyz;

    // Components of rl at zero produce infinities here; max() then discards
    // them, which is the correct answer for a ray parallel to that slab.
    let inv = 1.0 / rl;
    let t1  = (-e - pl) * inv;
    let t2  = (e - pl) * inv;
    let tv  = max(t1, t2);
    let t   = min(min(tv.x, tv.y), tv.z);
    if t <= 0.0 || !(t < 3.402823e38) {
        return R;
    }
    return (P + R * t) - cap.position_radius.xyz;
}

// How strongly a capture claims this point: 1 well inside, falling to 0 at the
// volume boundary so neighbouring captures cross-fade instead of popping.
fn capture_weight(P: vec3<f32>, cap: GpuReflectionCapture) -> f32 {
    if cap.shape == CAPTURE_SHAPE_BOX {
        let pl = (cap.world_to_local * vec4<f32>(P, 1.0)).xyz;
        let e  = cap.extents_transition.xyz;
        let d  = e - abs(pl);  // distance inside each pair of faces
        if d.x < 0.0 || d.y < 0.0 || d.z < 0.0 {
            return 0.0;
        }
        let transition = max(cap.extents_transition.w, 1e-4);
        return clamp(min(min(d.x, d.y), d.z) / transition, 0.0, 1.0);
    }
    let radius = cap.position_radius.w;
    let dist   = distance(P, cap.position_radius.xyz);
    if dist > radius {
        return 0.0;
    }
    // Fade across the outer 10% of the radius.
    let fade = max(radius * 0.1, 1e-4);
    return clamp((radius - dist) / fade, 0.0, 1.0);
}

// Blend every capture covering P, then let the skylight fill whatever coverage
// is left over.
//
// Captures arrive sorted smallest-influence-first, so accumulating front-to-back
// gives a small capture first claim on the pixel and lets the loop stop early
// once coverage saturates.
fn sample_reflection_environment(P: vec3<f32>, R: vec3<f32>, lod: f32) -> vec3<f32> {
    var accum   = vec3<f32>(0.0);
    var accum_a = 0.0;

    // Reflections compiled out for this target: no capture blend, and no
    // skylight fallback either — the skylight is layer 0 of the same cube array.
    if globals.enable_reflections == 0u {
        return accum;
    }

    let count = min(globals.reflection_capture_count, 64u);
    for (var i = 0u; i < count; i = i + 1u) {
        if accum_a >= 0.999 {
            break;
        }
        let cap = reflection_captures[i];
        if cap.cubemap_index < 0 {
            continue;  // no cubemap resident yet (unbaked, or awaiting capture)
        }
        let w = capture_weight(P, cap);
        if w <= 0.0 {
            continue;
        }

        var dir: vec3<f32>;
        if cap.shape == CAPTURE_SHAPE_BOX {
            dir = parallax_box(P, R, cap);
        } else {
            dir = parallax_sphere(P, R, cap.position_radius.xyz, cap.position_radius.w);
        }

        let s = textureSampleLevel(env_cube, env_sampler, dir, cap.cubemap_index, lod).rgb
              * cap.brightness;
        let contrib = w * (1.0 - accum_a);
        accum   = accum + s * contrib;
        accum_a = accum_a + contrib;
    }

    if accum_a < 0.999 {
        // Layer 0 stands in as the skylight: an un-parallaxed lookup for points
        // no capture reaches.
        let sky = textureSampleLevel(env_cube, env_sampler, R, 0, lod).rgb;
        accum = accum + sky * (1.0 - accum_a);
    }
    return accum;
}

// A light's direction from `world_pos` (toward it) and its radiance there
// (none past its range).
struct Incidence {
    L: vec3<f32>,
    radiance: vec3<f32>,
}

fn light_incidence(light: GpuLight, world_pos: vec3<f32>) -> Incidence {
    var L:        vec3<f32>;
    var radiance: vec3<f32>;

    if light.light_type == 0u {  // Directional light
        L        = normalize(-light.direction_outer.xyz);
        // A distant light crosses the atmosphere, if any, to reach the point.
        let through = atmosphere_transmittance_towards(
            atmosphere, (world_pos - cameras[0].position_near.xyz) * 0.001, L);
        radiance = light.color_intensity.xyz * light.color_intensity.w * through;
    } else {  // Point or spot light
        let to_light = light.position_range.xyz - world_pos;
        let dist     = length(to_light);
        if dist > light.position_range.w { return Incidence(to_light / max(dist, 1e-6), vec3<f32>(0.0)); }
        L = to_light / dist;
        var atten = 1.0 / (dist * dist + 0.0001);
        let normalized_dist = dist / light.position_range.w;
        atten *= max(0.0, 1.0 - normalized_dist * normalized_dist * normalized_dist * normalized_dist);
        if light.light_type == 2u {  // Spot light
            let cos_a = dot(-L, light.direction_outer.xyz);
            atten    *= smoothstep(light.direction_outer.w, light.inner_angle, cos_a);
        }
        // IES light profile: sample angular intensity distribution
        if light.ies_profile_index >= 0 {
            let light_dir = normalize(light.direction_outer.xyz);
            let theta = acos(clamp(dot(-L, light_dir), -1.0, 1.0));
            let cross_dir = cross(-L, light_dir);
            let phi = atan2(cross_dir.x, cross_dir.y);
            let ies_uv = vec2<f32>(
                phi / 6.2831853 + 0.5,
                theta * 0.6366198,
            ) * vec2<f32>(light.ies_angle_scale, light.ies_angle_scale)
            + vec2<f32>(light.ies_angle_offset / 360.0, 0.0);
            let ies_sample = textureSampleLevel(
                ies_textures, ies_sampler, ies_uv, u32(light.ies_profile_index), 0.0
            ).r;
            atten *= ies_sample;
        }
        // Light function (gobo/cookie) projection
        if light.light_function_index >= 0 {
            let light_to_surface = world_pos - light.position_range.xyz;
            let light_dir = normalize(light.direction_outer.xyz);
            let projected = light_to_surface - dot(light_to_surface, light_dir) * light_dir;
            let proj_len = max(length(projected), 0.0001);
            let projected_n = projected / proj_len;
            // Build tangent frame: handle degenerate case where light_dir ≈ up
            let gobo_up_vec = select(
                vec3<f32>(0.0, 1.0, 0.0),
                vec3<f32>(0.0, 0.0, 1.0),
                abs(dot(light_dir, vec3<f32>(0.0, 1.0, 0.0))) > 0.99,
            );
            let gobo_right = normalize(cross(light_dir, gobo_up_vec));
            let gobo_up = cross(gobo_right, light_dir);
            let gobo_uv = vec2<f32>(
                dot(projected, gobo_right) / (light.position_range.w * 0.5) + 0.5,
                dot(projected, gobo_up) / (light.position_range.w * 0.5) + 0.5,
            );
            let gobo_sample = textureSampleLevel(
                ies_textures, ies_sampler, clamp(gobo_uv, vec2<f32>(0.0), vec2<f32>(1.0)),
                u32(light.light_function_index), 0.0
            ).r;
            atten *= gobo_sample;
        }
        radiance = light.color_intensity.xyz * light.color_intensity.w * atten;
    }

    return Incidence(L, radiance);
}

// Evaluate one direct light with the full Cook-Torrance BRDF.
// `sf` is the shadow factor (0=shadowed, 1=lit), computed at the call site.
// When `is_anisotropic` is true, uses anisotropic GGX distribution with the
// given tangent direction, ax/aniso roughness in X, ay/aniso roughness in Y.
// For SSS surfaces (has_subsurface), applies wrap-diffuse lighting with
// transmission through the subsurface, tinted by subsurface_color.
fn pbr_direct_light(
    light:     GpuLight,
    world_pos: vec3<f32>,
    N:         vec3<f32>,
    V:         vec3<f32>,
    F0:        vec3<f32>,
    albedo:    vec3<f32>,
    roughness: f32,
    metallic:  f32,
    sf:        f32,
    is_anisotropic: bool,
    T:         vec3<f32>,
    ax:        f32,
    ay:        f32,
    has_subsurface: bool,
    subsurface_color: vec3<f32>,
) -> vec3<f32> {
    let incidence = light_incidence(light, world_pos);
    let L = incidence.L;
    let radiance = incidence.radiance;

    let NdL = max(dot(N, L), 0.0);
    if NdL == 0.0 { return vec3<f32>(0.0); }

    if all(radiance < vec3<f32>(0.002)) { return vec3<f32>(0.0); }

    let H         = normalize(V + L);
    let F         = fresnel_schlick(max(dot(H, V), 0.0), F0);
    let kD        = (1.0 - F) * (1.0 - metallic);
    let NdV       = max(dot(N, V), 0.0);
    var specular: vec3<f32>;

    if is_anisotropic {
        let phi_h    = compute_phi_h(N, H, T, cross(N, T));
        let D        = distribution_ggx_anisotropic(max(dot(N, H), 0.0), ax, ay, phi_h);
        let G        = geometry_smith_anisotropic(NdV, NdL, ax, ay);
        specular = D * G * F / (4.0 * NdV * NdL + 0.0001);
    } else {
        let D        = distribution_ggx(N, H, roughness);
        let G        = geometry_smith(N, V, L, roughness);
        specular = D * G * F / (4.0 * NdV * NdL + 0.0001);
    }

    var diffuse_term = kD * albedo / PI;
    var NdotL_effective = NdL;

    // Subsurface scattering: wrap-diffuse + transmission approximation.
    // Light penetrates the surface, scatters internally tinted by
    // subsurface_color, and exits.  The wrap term extends the diffuse
    // lobe into the back-facing hemisphere, mimicking internal scatter.
    if has_subsurface {
        let wrap = 0.2 + 0.4 * (1.0 - roughness); // tighter wrap for smoother surfaces
        NdotL_effective = max((dot(N, L) + wrap) / (1.0 + wrap), 0.0);
        // SSS diffuse: standard diffuse dimmed, plus a tinted wrap term
        let sss_diffuse = (1.0 - F) * albedo / PI * (1.0 - metallic);
        let sss_scatter = subsurface_color * (1.0 / PI);
        diffuse_term = mix(sss_diffuse, sss_scatter, 0.5);
        // Reduce shadow occlusion — SSS surfaces let light through
        let sf_sss = mix(sf, 1.0, 0.3);
        return (diffuse_term + specular) * radiance * NdotL_effective * sf_sss;
    }

    return (diffuse_term + specular) * radiance * NdotL_effective * sf;
}

// ── Tonemapping & bloom ───────────────────────────────────────────────────────
// Handled by PostProcessPass (helio-pass-postprocess). This pass writes raw HDR
// linear light to pre_aa — no tonemapping or bloom here.

// ── Fragment entry ────────────────────────────────────────────────────────────

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(i32(in.clip_pos.x), i32(in.clip_pos.y));

    // ── Depth guard: sky areas are already in the target → discard ──
    let depth = textureLoad(gbuf_depth, pix, 0);
    if helio_is_sky(depth) { discard; }

    // ── Read G-buffer ─────────────────────────────────────────────────────────
    let albedo_a  = textureLoad(gbuf_albedo,   pix, 0);
    let normal_r  = textureLoad(gbuf_normal,   pix, 0);
    let orm_r     = textureLoad(gbuf_orm,      pix, 0);
    let emissive_r = textureLoad(gbuf_emissive, pix, 0);
    let emissive  = emissive_r.rgb;

    let albedo    = albedo_a.rgb;
    let alpha     = albedo_a.a;
    let N_host    = normalize(normal_r.xyz);
    let ao        = orm_r.r;
    let roughness = orm_r.g;
    let metallic  = orm_r.b;

    // Screen-space AO (SSAO or pre-baked AO).  Sampled by normalised screen UV
    // so it works regardless of whether the AO texture is at a different resolution.
    let screen_uv    = in.clip_pos.xy / vec2<f32>(textureDimensions(gbuf_albedo));
    // The G-buffer's surface tag (the prelude's `GBUFFER_TAG_*`): a
    // traced surface or dense geometry carries its own occlusion, which a
    // screen-space AO kernel would overwhelm (coarse lattice edges at
    // distance, a meadow's blades), so it gets material AO and hemisphere
    // fill only.
    let surface_tag = textureLoad(gbuf_lightmap_uv, pix, 0).rg;
    let is_traced = gbuffer_tagged(surface_tag, GBUFFER_TAG_TRACED);
    // A traced drawer's debug colours (trace cost, levels) unlit.
    if globals.debug_mode == 32u || globals.debug_mode == 34u || (globals.debug_mode == 35u && pix.x * 2 < i32(textureDimensions(gbuf_albedo).x)) {
        if is_traced {
            return vec4<f32>(albedo, 1.0);
        }
        return vec4<f32>(0.035, 0.035, 0.035, 1.0);
    }
    if globals.debug_mode == 33u {
        // Per-pixel source of the visible surface. Warm = rasterized, cool = ray
        // traced. A traced drawer marks its pixels `GBUFFER_TAG_TRACED`. The
        // virtual-geometry shade pass paints its own pixels with the
        // source `vis_resolve` recorded for each one (hardware raster, compute
        // micro-raster, dense peel, ray query) and marks them with albedo alpha 0:
        // take that colour, never a guess from this pass's own state.
        // `GBUFFER_TAG_UNLIGHTMAPPED` cannot be that marker: every classic mesh
        // without a lightmap writes it too. Anything unmarked was written by
        // the classic G-buffer raster.
        let painted_by_virtual_geometry = albedo_a.a < 0.004;
        if is_traced {
            return vec4<f32>(0.12, 0.78, 1.0, 1.0);
        }
        if painted_by_virtual_geometry {
            return vec4<f32>(albedo, 1.0);
        }
        return vec4<f32>(1.0, 0.36, 0.08, 1.0);
    }
    let ssao_factor  = select(textureSample(screen_ao, screen_ao_samp, screen_uv).r, 1.0, gbuffer_own_occlusion(surface_tag));
    // Combined AO: material AO from G-buffer × screen-space AO.
    let ao_combined  = ao * ssao_factor;

    // Material and G-buffer visualizations return before lighting mixes in
    // direct or indirect contributions.
    if globals.debug_mode == 6u {
        return vec4<f32>(vec3<f32>(roughness), 1.0);
    }
    if globals.debug_mode == 7u {
        return vec4<f32>(vec3<f32>(metallic), 1.0);
    }
    if globals.debug_mode == 8u {
        return vec4<f32>(vec3<f32>(ao), 1.0);
    }
    if globals.debug_mode == 9u {
        let readable_emissive = emissive / (vec3<f32>(1.0) + max(emissive, vec3<f32>(0.0)));
        return vec4<f32>(readable_emissive, 1.0);
    }
    if globals.debug_mode == 12u {
        return vec4<f32>(vec3<f32>(ao_combined), 1.0);
    }
    if globals.debug_mode == 13u {
        let f0 = clamp(vec3<f32>(normal_r.w, orm_r.a, emissive_r.a), vec3<f32>(0.0), vec3<f32>(1.0));
        return vec4<f32>(f0, 1.0);
    }
    if globals.debug_mode == 14u {
        if !gbuffer_has_lightmap(surface_tag) {
            return vec4<f32>(0.12, 0.12, 0.12, 1.0);
        }
        return vec4<f32>(fract(surface_tag), 0.0, 1.0);
    }

    // ── Debug mode: bypass lighting ───────────────────────────────────────────
    // Mode 1 (UV Grid) and Mode 2 (Texture Direct) show raw colors without lighting
    // Mode 3 (Lit without normal mapping) goes through normal lighting
    // Mode 4 (G-buffer readback test) shows albedo read from G-buffer without lighting
    // Mode 5 (World normals) remaps N from [-1,1] → [0,1] as RGB (R=+X, G=+Y, B=+Z)
    // Mode 20 (VG triangle debug): per-face colour written into albedo by vg_gbuffer.wgsl
    // Mode 21 (VG cluster colours) and modes 100..127 (virtual-geometry debug views):
    // colours written into albedo by helio-pass-vg-shade's debug resolve
    if globals.debug_mode == 1u || globals.debug_mode == 2u || globals.debug_mode == 4u
    || globals.debug_mode == 20u || globals.debug_mode == 21u
    || (globals.debug_mode >= 100u && globals.debug_mode < 128u) {
        return vec4<f32>(albedo, alpha);
    }
    if globals.debug_mode == 5u {
        return vec4<f32>(N_host * 0.5 + 0.5, 1.0);
    }

    let lightmap_uv     = surface_tag;
    let has_lightmap    = gbuffer_has_lightmap(lightmap_uv);

    // ── Reconstruct world position from depth + inv_view_proj ────────────────
    // clip_pos.xy is in viewport space (0→width, 0→height, y↓).
    // Convert to NDC: x ∈ [-1,1], y ∈ [1,-1] (wgpu NDC y+ = up, viewport y+ = down).
    let screen_size = vec2<f32>(textureDimensions(gbuf_albedo));
    let uv_01       = in.clip_pos.xy / screen_size;
    let ndc_xy      = vec2<f32>(uv_01.x * 2.0 - 1.0, 1.0 - uv_01.y * 2.0);
    let world_h     = cameras[0].view_proj_inv * vec4<f32>(ndc_xy, depth, 1.0);
    let world_pos_host = world_h.xyz / world_h.w;

    // A remapped pixel is shaded where its content really is: back through the chain.
    let remap_chain = view_remap_of(lightmap_uv);
    let remapped    = remap_chain >= 0;
    var world_pos = world_pos_host;
    var N         = N_host;
    var eye       = cameras[0].position_near.xyz;
    if remapped {
        let unmap = view_remaps[remap_chain];
        world_pos = (unmap * vec4<f32>(world_pos_host, 1.0)).xyz;
        N         = normalize((unmap * vec4<f32>(N_host, 0.0)).xyz);
        eye       = (unmap * vec4<f32>(eye, 1.0)).xyz;
    }

    // ── Debug mode 10: shadow factor heatmap ──────────────────────────────────
    // Shows shadow_factor() per light averaged across all lights.
    // White = fully lit, black = fully occluded.
    // Useful for verifying shadow atlas is filled and matrices are correct.
    if globals.debug_mode == 10u {
        var shadow_sum = 0.0;
        for (var i = 0u; i < globals.light_count; i++) {
            shadow_sum += shadow_factor(i, world_pos, N, in.clip_pos.xy, globals.frame);
        }
        let sf = shadow_sum / max(f32(globals.light_count), 1.0);
        return vec4<f32>(sf, sf, sf, 1.0);
    }

    // ── Debug mode 11: light-space projection for first light face 0 ─────────
    // Orange gradient = pixel is inside the light frustum, depth = ndc.z.
    // Dark blue = pixel is outside the frustum (w<=0 or uv out of [0,1]).
    // Use this to verify shadow matrices are computed by ShadowMatrixPass.
    if globals.debug_mode == 11u && globals.light_count > 0u {
        let lc  = shadow_matrices[0u].mat * vec4<f32>(world_pos, 1.0);
        if lc.w > 0.001 {
            let ndc3 = lc.xyz / lc.w;
            let uv   = vec2<f32>(ndc3.x * 0.5 + 0.5, -ndc3.y * 0.5 + 0.5);
            if all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0))
                    && ndc3.z >= 0.0 && ndc3.z <= 1.0 {
                return vec4<f32>(ndc3.z, ndc3.z * 0.3, 0.0, 1.0);
            }
        }
        return vec4<f32>(0.0, 0.0, 0.2, 1.0);
    }

    // ── PBR setup ─────────────────────────────────────────────────────────────
    let F0  = clamp(vec3<f32>(normal_r.w, orm_r.a, emissive_r.a), vec3<f32>(0.0), vec3<f32>(0.999));
    let V   = normalize(eye - world_pos);
    let NdV = max(dot(N, V), 0.0);

    if globals.debug_mode == 19u {
        let fresnel = F0 + (vec3<f32>(1.0) - F0) * pow(1.0 - NdV, 5.0);
        return vec4<f32>(fresnel, 1.0);
    }

    // ── SSS / Extra surface data ──────────────────────────────────────────────
    let sss_r        = textureLoad(gbuf_sss, pix, 0);
    let extra_r      = textureLoad(gbuf_extra, pix, 0);
    // Flags are numerically encoded in the G-buffer so the Rgba16Float target
    // can preserve these small integer values exactly.
    let surface_flags = u32(round(extra_r.a));
    let is_anisotropic = (surface_flags & SURFACE_FLAG_ANISOTROPIC) != 0u;
    let has_subsurface = (surface_flags & SURFACE_FLAG_SUBSURFACE) != 0u;
    let low_specular   = (surface_flags & SURFACE_FLAG_LOW_SPECULAR) != 0u;
    let two_sided      = (surface_flags & SURFACE_FLAG_TWO_SIDED) != 0u;

    if globals.debug_mode == 15u {
        return vec4<f32>(sss_r.rgb, 1.0);
    }
    if globals.debug_mode == 16u {
        return vec4<f32>(
            clamp(extra_r.r, 0.0, 1.0),
            clamp(extra_r.g, 0.0, 1.0),
            fract(extra_r.b / (2.0 * 3.14159265)),
            1.0,
        );
    }
    if globals.debug_mode == 17u {
        return vec4<f32>(
            select(0.0, 1.0, is_anisotropic),
            select(0.0, 1.0, has_subsurface),
            select(0.0, 1.0, low_specular),
            1.0,
        );
    }
    if globals.debug_mode == 18u {
        let radius = max(sss_r.a, 0.0);
        let readable_radius = radius / (1.0 + radius);
        return vec4<f32>(vec3<f32>(readable_radius), 1.0);
    }

    // Per-surface work is displayed as a scalar heat map. Do not shade this
    // diagnostic with light intensity, N·L, or material response: those create
    // shape contrast that obscures the work being measured.
    if globals.debug_mode == 50u || globals.debug_mode == 51u || globals.debug_mode == 52u {
        let tile_x = u32(in.clip_pos.x) / TILE_SIZE;
        let tile_y = u32(in.clip_pos.y) / TILE_SIZE;
        let tile_idx = tile_y * globals.num_tiles_x + tile_x;
        var light_pass_count = tile_light_counts[tile_idx];
        if remapped { light_pass_count = min(globals.light_count, REMAPPED_MAX_LIGHTS); }

        var affecting_lights = 0.0;
        var shadow_work = 0.0;
        for (var i = 0u; i < light_pass_count; i++) {
            var light_idx = i;
            if !remapped { light_idx = tile_light_lists[tile_idx * MAX_LIGHTS_PER_TILE + i]; }
            let light = lights[light_idx];
            if light.light_type != 0u {
                let to_light = light.position_range.xyz - world_pos;
                let dist = length(to_light);
                let range = max(light.position_range.w, 1e-4);
                if dist >= range { continue; }
            }
            // Count light shader work, not its brightness. Point and spot
            // influence is bounded by the authored range; directional lights
            // affect every surface. Spot-cone attenuation still costs work and
            // therefore does not darken pixels outside the cone.
            affecting_lights += 1.0;
            if light.shadow_index != 0xffffffffu && !remapped { shadow_work += 1.0; }
        }

        if globals.debug_mode == 50u {
            return vec4<f32>(complexity_color(affecting_lights / 8.0), 1.0);
        }

        let is_anisotropic = (surface_flags & SURFACE_FLAG_ANISOTROPIC) != 0u;
        let has_subsurface = (surface_flags & SURFACE_FLAG_SUBSURFACE) != 0u;
        // Count shader features, not their authored values (roughness,
        // metallic, and lighting response do not change the execution path).
        var material_work = 1.0;
        material_work += select(0.0, 1.8, is_anisotropic) + select(0.0, 1.2, has_subsurface);
        material_work += select(0.0, 0.4, has_lightmap);
        let surface_work = 2.0 + material_work * 2.0 + affecting_lights * 1.6 + shadow_work * 1.1;
        if globals.debug_mode == 52u {
            // Show an absolute estimated time contribution per output pixel.
            // The pass timestamp is real GPU timing; surface_work distributes
            // that measured duration according to this pixel's known shader
            // work. Values are encoded against fixed microseconds-per-pixel
            // thresholds, never frame-relative percentiles or scene exposure.
            let estimated_us_per_pixel = globals.measured_lighting_ms * 1000.0
                * surface_work / f32(max(globals.pixel_count, 1u));
            return vec4<f32>(time_cost_color(estimated_us_per_pixel), 1.0);
        }
        return vec4<f32>(complexity_color(surface_work / 32.0), 1.0);
    }

    // Anisotropic GGX parameters
    let aniso_ax    = extra_r.r;
    let aniso_ay    = extra_r.g;
    let aniso_rot   = extra_r.b;

    // Compute aniso tangent direction from world normal + rotation
    var aniso_T = vec3<f32>(0.0);
    if is_anisotropic {
        let up_ref = vec3<f32>(0.0, 1.0, 0.0);
        var T_ref = normalize(cross(cross(N, up_ref), N));
        if length(T_ref) < 0.001 {
            T_ref = normalize(cross(cross(N, vec3<f32>(1.0, 0.0, 0.0)), N));
        }
        let B = cross(N, T_ref);
        let cos_a = cos(aniso_rot);
        let sin_a = sin(aniso_rot);
        aniso_T = T_ref * cos_a + B * sin_a;
    }

    // ── Direct lighting ────────────────────────────────────────────────────────
    // GPU-driven: iterate all visible lights (already culled on CPU by distance).
    // Shadow factor affects ONLY direct lighting (Lo).  Ambient / indirect light
    // is handled separately — shadow maps do not occlude it (that is AO's job).
    let sun_seen = textureLoad(sun_visibility, pix, 0);
    if globals.debug_mode == 26u {
        return vec4<f32>(vec3<f32>(clamp(sun_seen.x, 0.0, 1.0)), 1.0);
    }
    var Lo = vec3<f32>(0.0);
    if ENABLE_LIGHTING {
        let tile_x = u32(in.clip_pos.x) / TILE_SIZE;
        let tile_y = u32(in.clip_pos.y) / TILE_SIZE;
        let tile_idx = tile_y * globals.num_tiles_x + tile_x;
        var light_pass_count = tile_light_counts[tile_idx];
        if remapped {
            light_pass_count = min(globals.light_count, REMAPPED_MAX_LIGHTS);
        }
        let local_words = 3u * (u32(in.clip_pos.y) * u32(screen_size.x) + u32(in.clip_pos.x));
        for (var i = 0u; i < light_pass_count; i++) {
            var light_idx = i;
            if !remapped {
                light_idx = tile_light_lists[tile_idx * MAX_LIGHTS_PER_TILE + i];
            }
            let light = lights[light_idx];
            if light.light_type != 0u {
                let dist = length(light.position_range.xyz - world_pos);
                if dist > light.position_range.w { continue; }
            }
            // A surface facing away from the light receives nothing from it:
            // pbr_direct_light returns exactly zero whenever N·L <= 0, so its
            // shadow and transmittance lookups cannot change Lo. L is built
            // the way pbr_direct_light builds it; the margin keeps the skip
            // strictly inside that case even if the two dot products round
            // differently.
            var to_light_dir: vec3<f32>;
            if light.light_type == 0u {
                to_light_dir = normalize(-light.direction_outer.xyz);
            } else {
                let to_light = light.position_range.xyz - world_pos;
                to_light_dir = to_light / length(to_light);
            }
            // Two-sided foliage takes light from behind through itself
            // (`SURFACE_FLAG_TWO_SIDED`), shadowed as its back face.
            let back = dot(N, to_light_dir) < -1e-4;
            if back && !two_sided { continue; }
            let N_lit = select(N, -N, back);
            // A remapped pixel shows another world: the host's shadow maps
            // say nothing about it. Must be a real `if`, not select():
            // select() evaluates both arguments, paying for a full PCF/PCSS
            // shadow sample per light only to throw it away.
            var sf = 1.0;
            var transmit = vec3<f32>(1.0);
            if !remapped {
                sf = shadow_factor(light_idx, world_pos, N_lit, in.clip_pos.xy, globals.frame);
                if sf > 0.0 {
                    transmit = light_transmittance(light_idx, world_pos, N_lit);
                }
            }
            // The terrain's shadow of a local light (`local_visibility`).
            if !remapped && light.light_type != 0u && i < 32u {
                sf *= terrain_light_visibility(local_words, i);
            }
            if !remapped && light.light_type == 0u && dot(sun_seen.yzw, sun_seen.yzw) > 0.5 {
                let light_direction = -normalize(light.direction_outer.xyz);
                if dot(normalize(sun_seen.yzw), light_direction) > 0.99999 {
                    sf *= clamp(sun_seen.x, 0.0, 1.0);
                }
            }
            if back {
                let incidence = light_incidence(light, world_pos);
                Lo += transmit * sss_r.rgb / PI * incidence.radiance * max(-dot(N, incidence.L), 0.0) * sf;
                continue;
            }
            let sss_color = sss_r.rgb;
            Lo += transmit * pbr_direct_light(light, world_pos, N, V, F0, albedo, roughness, metallic, sf, is_anisotropic, aniso_T, aniso_ax, aniso_ay, has_subsurface, sss_color);
        }
    }

    // ── SSS transmission glow (rim-light approximation) ──────────────────────
    // Light enters the surface near the silhouette, scatters internally tinted
    // by subsurface_color, and exits.  Modelled as a view-angle-dependent glow
    // that peaks at grazing angles (where the light path through the medium is
    // shortest/dimmest physically, but the perceived scatter volume is largest).
    var sss_transmission = vec3<f32>(0.0);
    if has_subsurface {
        let rim = pow(1.0 - NdV, 3.0);
        sss_transmission = sss_r.rgb * rim * 1.2;
    }

    // ── Baked lightmap indirect diffuse ───────────────────────────────────────
    // For static geometry: pre-computed multi-bounce GI from offline baking.
    //
    // The GBuffer vertex shader writes a sentinel of (-1, -1) into the lightmap UV
    // channel for instances that have no lightmap (lightmap_index == 0xFFFFFFFF).
    // We detect this sentinel here to skip the lightmap contribution entirely,
    // rather than checking `uv > 0.01` which would incorrectly skip valid atlas
    // regions whose top-left corner happens to be near (0, 0).
    //
    // The UV is already clamped to the region's half-texel-inset boundary in the
    // vertex shader, so textureSample cannot bleed into adjacent atlas regions.
    // textureSampleLevel instead of textureSample: control flow is non-uniform (depends on
    // per-fragment world_pos via clip_pos), so WebGPU requires an explicit LOD variant.
    let lightmap_sample = textureSampleLevel(baked_lightmap, baked_lightmap_sampler, lightmap_uv, 0.0).rgb;
    // Nebula stores Σ(radiance · NdotL) — the same weighted sum pbr_direct_light accumulates
    // into Lo.  No extra 1/π factor here: Nebula does not divide by π in the bake shader,
    // so neither do we.  This convention matches Unreal Engine's lightmap pipeline.
    let lightmap_indirect = lightmap_sample * albedo;

    // Indirect specular is composed by fs_reflection in a second draw. This
    // keeps both fragment stages within the WebGPU baseline texture limit.

    // ── INDIRECT LIGHTING ────────────────────────────────────────────────────
    // Hemisphere ambient is shadow-INDEPENDENT.  Shadow maps only affect direct
    // lighting (Lo above); ambient occlusion (ao from G-buffer ORM.r) handles
    // indirect-light occlusion instead.  This ensures shadowed areas still
    // receive fill light and are never pitch black.

    // With an atmosphere the sky itself is the ambient light: the sky over
    // the surface (its altitude and sun angle), the lit ground below its
    // horizon included, cosine-convolved.
    var hemi: vec3<f32>;
    if atmosphere.planet.w > 0.5 {
        // The sky over the surface, by its altitude and sun angle, not the
        // eye's.
        hemi = atmosphere_sky_table_eval(atmosphere, (world_pos - eye) * 0.001, N, vec3<f32>(1.0)) * albedo;
    } else {
        let sky_color    = globals.ambient_color.rgb * globals.ambient_intensity;
        let ground_color = sky_color * 0.15;
        let hemi_t       = N.y * 0.5 + 0.5;
        hemi             = mix(ground_color, sky_color, hemi_t) * albedo;
    }
    // Where the GI probes hold the point, their irradiance replaces the
    // sky's: bounce light, and no sky where the world hides it (a remapped
    // pixel's probes are the host's: it keeps the sky).
    let gi_word = u32(in.clip_pos.y) * u32(screen_size.x) + u32(in.clip_pos.x);
    var gi_weight = 0.0;
    var gi = vec3<f32>(0.0);
    if !remapped && arrayLength(&gi_irradiance) > 1u && gi_word < arrayLength(&gi_irradiance) {
        let g = gi_irradiance[gi_word];
        let ba = unpack2x16float(g.y);
        gi = vec3<f32>(unpack2x16float(g.x), ba.x);
        gi_weight = clamp(ba.y, 0.0, 1.0);
        // What GI changes against the sky's ambient: green brighter, red
        // darker (a stop a third of full), blue where no probe holds it.
        if globals.debug_mode == 29u {
            let l = vec3<f32>(0.2126, 0.7152, 0.0722);
            let stops = log2(max(dot(gi * albedo, l), 1e-6) / max(dot(hemi, l), 1e-6)) * gi_weight;
            return vec4<f32>(clamp(-stops / 3.0, 0.0, 1.0), clamp(stops / 3.0, 0.0, 1.0), 0.0, 1.0);
        }
        hemi = mix(hemi, gi * albedo, gi_weight);
    }
    if globals.debug_mode == 29u {
        return vec4<f32>(0.0, 0.0, 0.3, 1.0);
    }
    // The probes' irradiance, or (36) the resolve's view of the probes.
    if globals.debug_mode == 28u || globals.debug_mode == 36u {
        return vec4<f32>(select(vec3<f32>(0.25, 0.0, 0.25), gi, gi_weight > 0.0), 1.0);
    }

    // Baked lightmap weight: 1.0 for static objects with valid lightmap, 0.0 otherwise.
    let lm_weight      = select(0.0, 1.0, has_lightmap);

    if globals.debug_mode == 27u {
        return vec4<f32>(gi_weight, 0.0, lm_weight, 1.0);
    }

    // The sky's ambient or the GI probes' (`hemi`); the baked lightmap
    // replaces it where there is one (below).
    let ambient_final = hemi;

    // ── Combine ───────────────────────────────────────────────────────────────
    //
    // Unreal-style "Static light" model:
    //   • The baked lightmap encodes TOTAL LIGHTING (direct shadow + indirect GI)
    //     from every baked light.  For lightmapped surfaces Lo is suppressed so the
    //     same lights are not double-counted.
    //   • AO is NOT applied to the lightmap.  The path-traced bake already accounts
    //     for per-texel occlusion via shadow rays; applying screen-space AO on top
    //     would double-darken the result.
    //   • For un-lightmapped surfaces the normal dynamic path applies AO to the
    //     hemisphere/GI ambient term as usual.
    let lo_final      = Lo * (1.0 - lm_weight);          // suppress Lo for baked pixels
    let indirect_dyn  = ambient_final * ao_combined;  // AO on dynamic GI
    let indirect_bake = lightmap_indirect;            // no AO on lightmap
    let indirect      = select(indirect_dyn, indirect_bake, has_lightmap);
    var color         = lo_final + indirect;
    if globals.debug_mode == 22u {
        return vec4<f32>(lo_final, 1.0);
    }
    if globals.debug_mode == 23u {
        return vec4<f32>(indirect, 1.0);
    }
    if globals.debug_mode == 24u {
        return vec4<f32>(color, 1.0);
    }
    if globals.debug_mode == 25u {
        return vec4<f32>(emissive + sss_transmission, 1.0);
    }
    color        += emissive;               // emissive from G-buffer
    color        += sss_transmission;       // SSS rim glow
    // Transmission is added by fs_reflection (additive, like reflections).

    // Tonemapping & bloom handled by PostProcessPass — write raw HDR linear.
    return vec4<f32>(color, alpha);
}

// Reflection composition (and the added transmission) is deliberately
// isolated from base lighting so neither fragment entry point exceeds the
// WebGPU baseline of 16 sampled textures.
// The normal path is additively blended over fs_main; SSR debug modes use the
// companion replacement pipeline.
@fragment
fn fs_reflection(in: VSOut) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(i32(in.clip_pos.x), i32(in.clip_pos.y));
    let depth = textureLoad(gbuf_depth, pix, 0);
    if helio_is_sky(depth) { discard; }

    // The specular layers at this pixel, whatever their resolution (SSR traces
    // at half): premultiplied rgb and coverage.
    let uv = in.clip_pos.xy / vec2<f32>(textureDimensions(gbuf_depth));
    let ssr_hit = indirect_specular_at(specular_layer_0, uv);
    if globals.debug_mode == 30u {
        return vec4<f32>(vec3<f32>(ssr_hit.a), 1.0);
    }
    if globals.debug_mode == 31u {
        return vec4<f32>(ssr_hit.rgb, 1.0);
    }

    // These modes replace lighting with another diagnostic in fs_main.
    if globals.debug_mode == 1u || globals.debug_mode == 2u
        || globals.debug_mode == 4u || globals.debug_mode == 5u
        || globals.debug_mode == 6u || globals.debug_mode == 7u
        || globals.debug_mode == 8u || globals.debug_mode == 9u
        || globals.debug_mode == 10u || globals.debug_mode == 11u
        || globals.debug_mode == 12u || globals.debug_mode == 13u
        || globals.debug_mode == 14u || globals.debug_mode == 15u
        || globals.debug_mode == 16u || globals.debug_mode == 17u
        || globals.debug_mode == 18u || globals.debug_mode == 19u
        || globals.debug_mode == 22u || globals.debug_mode == 23u
        || globals.debug_mode == 24u || globals.debug_mode == 25u
        || globals.debug_mode == 26u || globals.debug_mode == 27u
        || globals.debug_mode == 28u || globals.debug_mode == 29u
        || globals.debug_mode == 34u || globals.debug_mode == 35u
        || globals.debug_mode == 36u
        || (globals.debug_mode >= 100u && globals.debug_mode < 128u)
        || globals.debug_mode == 20u || globals.debug_mode == 21u
        || globals.debug_mode == 32u || globals.debug_mode == 33u
        || globals.debug_mode == 50u || globals.debug_mode == 51u
        || globals.debug_mode == 52u {
        return vec4<f32>(0.0);
    }

    let normal_r = textureLoad(gbuf_normal, pix, 0);
    let orm_r = textureLoad(gbuf_orm, pix, 0);
    let emissive_r = textureLoad(gbuf_emissive, pix, 0);
    let N_host = normalize(normal_r.xyz);
    let roughness = orm_r.g;
    let F0 = clamp(
        vec3<f32>(normal_r.w, orm_r.a, emissive_r.a),
        vec3<f32>(0.0),
        vec3<f32>(0.999),
    );

    let screen_size = vec2<f32>(textureDimensions(gbuf_normal));
    let screen_uv = in.clip_pos.xy / screen_size;
    let ao_combined = orm_r.r * textureSample(screen_ao, screen_ao_samp, screen_uv).r;
    let lightmap_uv = textureLoad(gbuf_lightmap_uv, pix, 0).rg;
    let has_lightmap = gbuffer_has_lightmap(lightmap_uv);

    let ndc_xy = vec2<f32>(screen_uv.x * 2.0 - 1.0, 1.0 - screen_uv.y * 2.0);
    let world_h = cameras[0].view_proj_inv * vec4<f32>(ndc_xy, depth, 1.0);
    let world_pos_host = world_h.xyz / world_h.w;

    // A remapped pixel reflects its content's world, seen from where the eye is in it.
    let remap_chain = view_remap_of(lightmap_uv);
    let remapped = remap_chain >= 0;
    var world_pos = world_pos_host;
    var N = N_host;
    var eye = cameras[0].position_near.xyz;
    if remapped {
        let unmap = view_remaps[remap_chain];
        world_pos = (unmap * vec4<f32>(world_pos_host, 1.0)).xyz;
        N = normalize((unmap * vec4<f32>(N_host, 0.0)).xyz);
        eye = (unmap * vec4<f32>(eye, 1.0)).xyz;
    }
    let V = normalize(eye - world_pos);
    let NdV = max(dot(N, V), 0.0);
    let F_ibl = fresnel_schlick_roughness(NdV, F0, roughness);
    let R = reflect(-V, N);

    var spec_ind = vec3<f32>(0.0);
    if globals.enable_env_reflections != 0u {
        let env_lod = roughness * ENV_MAX_LOD;
        let env_sample = sample_reflection_environment(world_pos, R, env_lod);
        let env_brdf = env_brdf_approx(NdV, roughness);
        spec_ind = env_sample * (F0 * env_brdf.x + env_brdf.y);
    }

    // Screen-space hits are of the host's view; a remapped pixel has none of its own.
    if !remapped && globals.enable_reflections != 0u && ssr_hit.a > 0.0 {
        spec_ind = spec_ind * (1.0 - ssr_hit.a) + ssr_hit.rgb * F_ibl;
    }

    let planar_hit = indirect_specular_at(specular_layer_1, uv);
    if !remapped && globals.enable_reflections != 0u && planar_hit.a > 0.0 {
        spec_ind = spec_ind * (1.0 - planar_hit.a) + planar_hit.rgb * F_ibl;
    }

    let contribution = select(spec_ind * ao_combined, spec_ind, has_lightmap);
    var added = vec3<f32>(0.0);
    if all(vec2<u32>(pix) < textureDimensions(transmission)) {
        added = textureLoad(transmission, pix, 0).rgb;
    }
    return vec4<f32>(contribution + added, 0.0);
}

// One comparison against the dynamic or the static casters' atlas, and an
// occluder depth from either (the shadow atlas's `budget_*`; 32 and 16 are
// the caster-kind bits each atlas holds).
fn budget_compare_dynamic(uv:vec2f,layer:u32,depth:f32)->f32 {
    return budget_compare(shadow_atlas,shadow_sampler,uv,layer,depth,32u);
}
fn budget_depth_dynamic(pixel:vec2i,layer:u32)->f32 {
    return budget_depth(shadow_atlas,pixel,layer,32u);
}
fn budget_compare_static(uv:vec2f,layer:u32,depth:f32)->f32 {
    return budget_compare(static_shadow_atlas,shadow_sampler,uv,layer,depth,16u);
}
fn budget_depth_static(pixel:vec2i,layer:u32)->f32 {
    return budget_depth(static_shadow_atlas,pixel,layer,16u);
}

// Bounded screen-space fallback, shared by resident and nonresident lights so
// transitions across the atlas budget do not toggle contact visibility.
fn contact_visibility(light:GpuLight,position:vec3f,normal:vec3f)->f32 {
    if (light._pad&64u)!=0u {return 1.0;}
    let direction=select(normalize(light.position_range.xyz-position),normalize(-light.direction_outer.xyz),light.light_type==0u);
    let origin=position+normal*0.02;
    let reach=select(min(length(light.position_range.xyz-origin),0.5),0.5,light.light_type==0u);
    let dims=vec2i(textureDimensions(gbuf_depth));
    for(var i=1u;i<=16u;i++) {
        let p=origin+direction*(reach*f32(i)/16.0);
        let clip=cameras[0].view_proj*vec4f(p,1);
        if clip.w<=0.0 {break;}
        let ndc=clip.xyz/clip.w;let uv=vec2f(ndc.x*0.5+0.5,0.5-ndc.y*0.5);
        if any(uv<=vec2f(0)) || any(uv>=vec2f(1)) {break;}
        let depth=textureLoad(gbuf_depth,vec2i(uv*vec2f(dims)),0);
        if helio_is_sky(depth) {continue;}
        let h=cameras[0].view_proj_inv*vec4f(ndc.xy,depth,1);
        let hit=h.xyz/h.w;
        let delta=length(p-cameras[0].position_near.xyz)-length(hit-cameras[0].position_near.xyz);
        if delta>0.015 && delta<0.08 {
            let edge=min(min(uv.x,1.0-uv.x),min(uv.y,1.0-uv.y));
            return 1.0-smoothstep(0.0,0.05,edge)*(1.0-f32(i)/17.0);
        }
    }
    return 1.0;
}
fn shadow_factor(light_idx:u32,position:vec3f,normal:vec3f,pixel:vec2f,frame:u32)->f32 {
    if !ENABLE_SHADOWS {return 1.0;}
    let light=lights[light_idx];
    let requested=select(light.shadow_index!=0xffffffffu,(light._pad&4u)!=0u,(light._pad&8u)!=0u);
    if !requested || (light._pad&48u)==48u {return 1.0;}
    return min(raster_shadow_factor(light_idx,position,normal,pixel,frame),contact_visibility(light,position,normal));
}
