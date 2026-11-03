//!use helio_prelude
//!use sun_occlusion
//!use cloud_shadow
//
// Volumetric fog — froxel grid (Hillaire, "Physically Based and Unified Volumetric
// Rendering in Frostbite", SIGGRAPH 2015).
//
// Two compute passes over a view-space 3D grid, rather than a raymarch per pixel:
//
//   cs_inject     — one thread per froxel: density, one shadow tap per light,
//                   temporally blended against the reprojected previous frame.
//   cs_integrate  — one thread per (x,y) column: marches z once, turning the
//                   per-froxel scattering/extinction into accumulated
//                   in-scattering + transmittance.
//
// The composite in postprocess.wgsl is then a single trilinear 3D fetch at the
// pixel's depth:
//
//     color = color * fog.a + fog.rgb
//
// Why this shape:
//   - Cost is decoupled from screen resolution. 192x108x128 = ~2.65M froxels lit
//     once each, against 1280x720x64 = ~59M samples for the per-pixel march.
//   - The trilinear fetch filters in x, y *and* depth, so there is no blocky
//     upsample and no bilateral filter to write.
//   - Temporal reprojection carries samples across frames, which is what makes
//     one shadow tap per froxel enough. Without it this would need many more.

// ── Fog config ──────────────────────────────────────────────────────────────
// Byte-identical to GpuFogUniforms (64 bytes at the PP fog block).
// The legacy block supplies a compatibility medium and the resolved view range.

struct FogUniforms {
    fog_enabled:               u32,
    fog_mode:                  u32,
    fog_density:               f32,
    fog_height_falloff:        f32,
    fog_start_distance:        f32,
    fog_max_distance:          f32,
    fog_height:                f32,
    fog_scattering_anisotropy: f32,
    fog_color:                 vec3<f32>,
    _pad_fog_color:            f32,
    fog_emissive:              vec3<f32>,
    _pad_fog_emissive:         f32,
}

const FOG_MODE_UNIFORM: u32 = 0u;
const FOG_MODE_HEIGHT: u32  = 1u;

// ── Lights ──────────────────────────────────────────────────────────────────
// Mirror of GpuLight (128 bytes). See that struct's doc comment for the
// full list of shaders that must be edited together.

struct GpuLight {
    position_range:  vec4<f32>,
    direction_outer: vec4<f32>,
    color_intensity: vec4<f32>,
    shadow_index:    u32,
    light_type:      u32,
    inner_angle:     f32,
    _pad:            u32,
    god_rays_enabled:  u32,
    god_rays_density:  f32,
    god_rays_weight:   f32,
    god_rays_decay:    f32,
    god_rays_exposure: f32,
    flare_enabled:      u32,
    flare_type:         u32,
    flare_intensity:    f32,
    flare_scale:        f32,
    flare_tint_r:       f32,
    flare_tint_g:       f32,
    flare_tint_b:       f32,
    ies_profile_index:    i32,
    light_function_index: i32,
    ies_angle_scale:      f32,
    ies_angle_offset:     f32,
}

struct LightMatrix { mat: mat4x4<f32>,
    atlas: vec4f,
    policy: vec4u, }

struct FogGlobals {
    /// Globals.csm_splits — cascade boundaries for directional shadow lookup.
    csm_splits:  vec4<f32>,
    _reserved: u32,
    frame:       u32,
    /// 0 on the first frame or after a camera cut: ignore history.
    history_valid: u32,
    /// Weight of the current frame in the temporal blend. Lower = steadier, but
    /// slower to react to lights and shadows moving.
    temporal_blend: f32,
    time: f32,
    /// Last frame's camera jitter (NDC), to reproject into its unjittered grid.
    prev_jitter_x: f32,
    prev_jitter_y: f32,
    /// Seconds since the previous frame (smoke history advection).
    delta_time: f32,
    grid: vec3<u32>,
    enabled: u32,
    // Samples, volume bounds and the camera are in the frame's space:
    // relative to the frame's world origin, an f32 pair (hi + lo). Planet
    // centres are world anchors, rebased by it, and heights along world Y
    // add it back.
    origin_hi: vec4<f32>,
    origin_lo: vec4<f32>,
    // The world origin modulo SMOKE_PERIOD (m): smoke noise is anchored to
    // the world.
    noise_phase: vec4<f32>,
}

const LIGHT_DIRECTIONAL: u32 = 0u;
const LIGHT_POINT:       u32 = 1u;
const LIGHT_SPOT:        u32 = 2u;

const NO_SHADOW: u32 = 4294967295u;

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform>       fog:             FogUniforms;
@group(0) @binding(2) var<uniform>       fog_globals:     FogGlobals;
@group(0) @binding(3) var<storage, read> lights:          array<GpuLight>;
@group(0) @binding(4) var<storage, read> shadow_matrices: array<LightMatrix>;
@group(0) @binding(5) var                shadow_atlas:    texture_depth_2d_array;
@group(0) @binding(6) var                shadow_samp:     sampler_comparison;
@group(0) @binding(20) var               static_shadow_atlas: texture_depth_2d_array;
/// Translucent casters (stained glass): rgb = 1 - T, a = 1 - nearest pane depth.
@group(0) @binding(21) var               shadow_transmittance: texture_2d_array<f32>;
// The sun's occlusion by terrain outside the shadow maps (voxel terrain:
// `sun_occlusion`), its levels 0 when none is published.
@group(0) @binding(22) var               sun_occlusion_maps: texture_2d<f32>;
@group(0) @binding(23) var<uniform>      sun_occlusion_frame: SunOcclusionFrame;
@group(0) @binding(26) var<storage, read> sun_occlusion_pages: array<u32>;
// The clouds' shadow along the sun (helio-pass-sky's `cloud_shadow`), its
// frame zero when no clouds are published.
@group(0) @binding(24) var               cloud_shadow_map: texture_2d<f32>;
@group(0) @binding(25) var<uniform>      cloud_shadow_frame: CloudShadowFrame;
/// Previous frame's scattering grid, for temporal reprojection.
@group(0) @binding(7) var                scatter_history: texture_3d<f32>;
@group(0) @binding(8) var                linear_samp:     sampler;
/// rgb = in-scattered radiance * density, a = extinction.
@group(0) @binding(9) var                scatter_out:     texture_storage_3d<rgba16float, write>;

// SceneDB PP row. Only its fog block is interpreted; shader_source() derives
// the opaque head and tail from the current Rust PP ABI.
struct FogVolume {
    bounds_min: vec4<f32>,
    bounds_max: vec4<f32>,
    priority: f32,
    blend_radius: f32,
    blend_weight: f32,
    unbound: u32,
    _pad: vec4<f32>,
    before_fog: array<vec4<u32>, __PP_HEAD_VEC4__>,
    medium: FogUniforms,
    after_fog: array<vec4<u32>, __PP_TAIL_VEC4__>,
}
struct WorldMedium {
    enabled: u32, mode: u32, extinction: f32, height_falloff: f32,
    height: f32, anisotropy: f32, _pad: vec2<f32>,
    albedo: vec3<f32>, _pad_albedo: f32,
    emission: vec3<f32>, _pad_emission: f32,
    // Heights above a planet: centre (world m), radius (m); radius 0: along Y.
    planet: vec4<f32>,
}
struct LocalMedium {
    bounds_min: vec4<f32>, bounds_max: vec4<f32>, medium: WorldMedium,
    edge_fade: f32, _pad0: f32, _pad1: f32, _pad2: f32,
}
struct RenderSettings {
    occupied: u32, view_id: u32, quality: u32, enabled: u32,
    max_distance: f32, light_max_distance: f32, temporal_blend: f32, history_rejection: f32,
    light_samples: u32, history_epoch: u32, _pad: vec2<u32>,
}
struct ActiveMedia {
    volume_count: u32,
    light_count: u32,
    previous_range: f32,
    history_compatible: u32,
    global_count: u32,
    local_count: u32,
    has_medium: u32,
    previous_quality: u32,
    grid: vec3<u32>,
    render_enabled: u32,
    light_max_distance: f32,
    temporal_blend: f32,
    history_rejection: f32,
    light_samples: u32,
    previous_epoch: u32,
    epoch: u32,
    previous_light_hash: u32,
    last_active: u32,
    _reserved: array<u32, 12>,
    volumes: array<u32, 64>,
    globals: array<u32, 64>,
    locals: array<u32, 64>,
    // Every active light row, in row order; the buffer holds one slot per
    // SceneDB light row, so the list never overflows (Pulsar-Native#838).
    lights: array<u32>,
}
@group(0) @binding(11) var<storage, read> volumes: array<FogVolume>;
@group(0) @binding(12) var<storage, read_write> media_list: ActiveMedia;
// Distinct layout for classification: this buffer is never bound while used
// as an indirect-dispatch argument (STORAGE_WRITE and INDIRECT conflict).
// [0..3] inject, [3..6] cull, [6..9] integrate (columns of the allocated grid),
// [9] the range published to compositing (f32 bits; see cs_classify).
@group(0) @binding(13) var<storage, read_write> dispatch: array<u32, 10>;
@group(0) @binding(14) var<storage, read> global_media: array<WorldMedium>;
@group(0) @binding(15) var<storage, read> local_media: array<LocalMedium>;
@group(0) @binding(16) var<storage, read> render_settings: array<RenderSettings>;
@group(0) @binding(17) var<storage, read> legacy_media: array<FogUniforms>;
@group(0) @binding(18) var<storage, read_write> resolved_fog: FogUniforms;
struct Cluster { count: u32, lights: array<u32, 64> }
@group(0) @binding(19) var<storage, read_write> clusters: array<Cluster>;

fn finite_clamp(v: f32, lo: f32, hi: f32) -> f32 {
    return select(lo, min(v, hi), v >= lo);
}

// SceneDB rows are indexed by entity, so every scanned array spans the whole
// entity range and is mostly holes. Scans use one 256-thread workgroup and
// read only row headers; lowest-row selection keeps results deterministic.
const SCAN_THREADS: u32 = 256u;
const NONE_ROW: u32 = 0xffffffffu;
var<workgroup> wg_legacy_row: atomic<u32>;
var<workgroup> wg_exact_row: atomic<u32>;
var<workgroup> wg_fallback_row: atomic<u32>;
/// Farthest distance (f32 bits; positive floats order as u32) from the camera to
/// any active bounded medium.
var<workgroup> wg_media_far: atomic<u32>;

/// Largest camera distance to the box: the farthest corner.
fn farthest_corner_distance(camera: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> f32 {
    return length(max(abs(camera - lo), abs(camera - hi)));
}

@compute @workgroup_size(256)
fn cs_resolve(@builtin(local_invocation_index) lid: u32) {
    if lid == 0u {
        atomicStore(&wg_legacy_row, NONE_ROW);
        atomicStore(&wg_exact_row, NONE_ROW);
        atomicStore(&wg_fallback_row, NONE_ROW);
        atomicStore(&wg_media_far, 0u);
    }
    workgroupBarrier();
    let view_id = bitcast<u32>(cameras[0].jitter_frame.w);
    // Bounded media must never be cut by the grid's far plane: moving away from
    // a volume would otherwise shear it off slice by slice until nothing is
    // left. The grid range grows to hold every active volume.
    let camera = cameras[0].position_near.xyz;
    for (var i = lid; i < arrayLength(&volumes); i += SCAN_THREADS) {
        if legacy_volume_active(volumes[i]) {
            atomicMax(&wg_media_far, bitcast<u32>(farthest_corner_distance(camera, volumes[i].bounds_min.xyz, volumes[i].bounds_max.xyz)));
        }
    }
    for (var i = lid; i < arrayLength(&local_media); i += SCAN_THREADS) {
        if local_medium_active(local_media[i]) {
            atomicMax(&wg_media_far, bitcast<u32>(farthest_corner_distance(camera, local_media[i].bounds_min.xyz, local_media[i].bounds_max.xyz)));
        }
    }
    for (var i = lid; i < arrayLength(&legacy_media); i += SCAN_THREADS) {
        if legacy_media[i].fog_enabled != 0u { atomicMin(&wg_legacy_row, i); }
    }
    for (var i = lid; i < arrayLength(&render_settings); i += SCAN_THREADS) {
        if render_settings[i].occupied == 0u { continue; }
        let row_view = render_settings[i].view_id;
        if row_view == view_id { atomicMin(&wg_exact_row, i); }
        else if row_view == 0xffffffffu { atomicMin(&wg_fallback_row, i); }
    }
    workgroupBarrier();
    if lid != 0u { return; }

    var config = fog;
    let legacy_row = atomicLoad(&wg_legacy_row);
    if legacy_row != NONE_ROW { config = legacy_media[legacy_row]; }
    // Exact view matches beat the all-view fallback; lowest row wins ties.
    var selected = atomicLoad(&wg_exact_row);
    if selected == NONE_ROW { selected = atomicLoad(&wg_fallback_row); }
    var quality = 0u;
    var epoch = 0u;
    // The post-process fog block's range applies only when that fog is on.
    // Disabled, the resolver reports a 1-unit placeholder range; using it
    // would confine global and local media to the first unit of the grid.
    var range = select(1000.0, config.fog_max_distance,
        config.fog_enabled != 0u && config.fog_max_distance > HELIO_FROXEL_NEAR);
    media_list.render_enabled = fog_globals.enabled;
    media_list.temporal_blend = finite_clamp(fog_globals.temporal_blend, 0.01, 1.0);
    media_list.history_rejection = 0.8;
    media_list.light_max_distance = range;
    media_list.light_samples = 16u;
    if selected != 0xffffffffu {
        let s = render_settings[selected];
        quality = min(s.quality, 1u);
        epoch = s.history_epoch;
        range = s.max_distance;
        media_list.render_enabled &= s.enabled;
        media_list.temporal_blend = finite_clamp(s.temporal_blend, 0.01, 1.0);
        media_list.history_rejection = finite_clamp(s.history_rejection, 0.01, 1.0);
        media_list.light_max_distance = finite_clamp(s.light_max_distance, 0.1, 100000.0);
        media_list.light_samples = select(clamp(s.light_samples, 8u, 32u), select(16u, 24u, quality == 1u), s.light_samples == 0u);
    }
    // Grow (never shrink) the range to the farthest bounded medium, in coarse
    // geometric steps so it does not change every frame as the camera moves:
    // a changed range invalidates the whole history.
    let media_far = bitcast<f32>(atomicLoad(&wg_media_far));
    if media_far > range {
        range = min(exp2(ceil(log2(media_far * 1.05) * 2.0) * 0.5), 20000.0);
    }
    range = finite_clamp(range, HELIO_FROXEL_NEAR + 0.01, 100000.0);
    media_list.history_compatible = select(0u, 1u,
        abs(media_list.previous_range - range) < 0.001 && media_list.previous_epoch == epoch && media_list.previous_quality == quality);
    media_list.previous_range = range;
    media_list.previous_epoch = epoch;
    media_list.previous_quality = quality;
    media_list.epoch = epoch;
    media_list.grid = max(fog_globals.grid / select(2u, 1u, quality == 1u), vec3<u32>(1));
    config.fog_max_distance = range;
    if media_list.render_enabled == 0u { config.fog_enabled = 0u; }
    resolved_fog = config;

}

fn world_medium_active(m: WorldMedium) -> bool {
    return m.enabled != 0u && (m.extinction > 0.0 || any(m.emission > vec3<f32>(0.0)));
}
fn legacy_volume_active(v: FogVolume) -> bool {
    return v.unbound == 0u && v.blend_weight > 0.0 && v.medium.fog_enabled != 0u
        && v.medium.fog_density > 0.0 && all(v.bounds_max.xyz > v.bounds_min.xyz);
}
fn local_medium_active(v: LocalMedium) -> bool {
    return world_medium_active(v.medium) && all(v.bounds_max.xyz > v.bounds_min.xyz);
}
fn light_active(l: GpuLight) -> bool {
    return l.god_rays_enabled != 0u && l.color_intensity.w > 0.0
        && l.god_rays_weight > 0.0 && l.god_rays_density > 0.0 && l.god_rays_exposure > 0.0;
}
fn hash_word(h: u32, word: u32) -> u32 { return (h ^ word) * 16777619u; }
fn hash_vector(h: u32, v: vec4<f32>) -> u32 {
    return hash_word(hash_word(hash_word(hash_word(h, bitcast<u32>(v.x)), bitcast<u32>(v.y)), bitcast<u32>(v.z)), bitcast<u32>(v.w));
}

var<workgroup> wg_volume_rows: array<u32, 64>;
var<workgroup> wg_global_rows: array<u32, 64>;
var<workgroup> wg_local_rows: array<u32, 64>;
var<workgroup> wg_light_offsets: array<u32, 256>;
var<workgroup> wg_volume_count: atomic<u32>;
var<workgroup> wg_global_count: atomic<u32>;
var<workgroup> wg_local_count: atomic<u32>;
var<workgroup> wg_light_count: atomic<u32>;
var<workgroup> wg_light_hash: atomic<u32>;

// Structural identity only: which rows participate, their type and shadow slot.
// Continuous changes (flicker, colour, gains, a moving or rotating light) must
// not discard history: every froxel would then show its raw jittered sample,
// i.e. per-frame noise. The exponential blend follows them within a few
// frames, as it does for drifting smoke.
fn light_hash_of(i: u32, light: GpuLight) -> u32 {
    var h = hash_word(2166136261u, i);
    h = hash_word(h, light.shadow_index);
    return hash_word(h, light.light_type);
}

// Authored media, by value: an edit (density, colour, emission, bounds) is a
// discrete change that must show this frame, not fade in over the blend.
// Animated smoke is time-driven and leaves the rows, so this hash, unchanged.
fn medium_hash_of(seed: u32, m: WorldMedium) -> u32 {
    var h = hash_word(hash_word(seed, m.enabled), m.mode);
    h = hash_vector(h, vec4<f32>(m.extinction, m.height_falloff, m.height, m.anisotropy));
    h = hash_vector(h, vec4<f32>(m.albedo, 0.0));
    h = hash_vector(h, m.planet);
    return hash_vector(h, vec4<f32>(m.emission, 0.0));
}

// Parallel compaction of live rows, then a rank sort so the compact lists are
// in row order regardless of atomic ordering. Media counts past a list's
// capacity are still counted; consumers then scan every row instead of losing
// any. The light list is sized to every row and never overflows.
@compute @workgroup_size(256)
fn cs_classify(@builtin(local_invocation_index) lid: u32) {
    if lid == 0u {
        atomicStore(&wg_volume_count, 0u);
        atomicStore(&wg_global_count, 0u);
        atomicStore(&wg_local_count, 0u);
        atomicStore(&wg_light_count, 0u);
        atomicStore(&wg_light_hash, 0u);
    }
    workgroupBarrier();
    for (var i = lid; i < arrayLength(&volumes); i += SCAN_THREADS) {
        if volumes[i].blend_weight > 0.0 && legacy_volume_active(volumes[i]) {
            let slot = atomicAdd(&wg_volume_count, 1u);
            if slot < 64u { wg_volume_rows[slot] = i; }
        }
    }
    for (var i = lid; i < arrayLength(&global_media); i += SCAN_THREADS) {
        if global_media[i].enabled != 0u && world_medium_active(global_media[i]) {
            atomicAdd(&wg_light_hash, medium_hash_of(hash_word(0x9e3779b9u, i), global_media[i]));
            let slot = atomicAdd(&wg_global_count, 1u);
            if slot < 64u { wg_global_rows[slot] = i; }
        }
    }
    for (var i = lid; i < arrayLength(&local_media); i += SCAN_THREADS) {
        if local_media[i].medium.enabled != 0u && local_medium_active(local_media[i]) {
            let bounds = hash_vector(hash_vector(hash_word(0x85ebca6bu, i), local_media[i].bounds_min),
                local_media[i].bounds_max);
            atomicAdd(&wg_light_hash, medium_hash_of(hash_word(bounds, bitcast<u32>(local_media[i].edge_fade)), local_media[i].medium));
            let slot = atomicAdd(&wg_local_count, 1u);
            if slot < 64u { wg_local_rows[slot] = i; }
        }
    }
    // Lights: each thread scans one contiguous chunk of rows, so a prefix sum
    // of the chunk counts places every active row in row order. The list is
    // complete however many lights are active, and later stages loop over it
    // instead of every allocated row (Pulsar-Native#838).
    let light_rows = min(arrayLength(&lights), arrayLength(&media_list.lights));
    let light_chunk = (light_rows + SCAN_THREADS - 1u) / SCAN_THREADS;
    let light_begin = min(lid * light_chunk, light_rows);
    let light_end = min(light_begin + light_chunk, light_rows);
    var chunk_lights = 0u;
    for (var i = light_begin; i < light_end; i++) {
        if lights[i].god_rays_enabled != 0u && light_active(lights[i]) {
            // Commutative combination: independent of scan order.
            atomicAdd(&wg_light_hash, light_hash_of(i, lights[i]));
            chunk_lights++;
        }
    }
    wg_light_offsets[lid] = chunk_lights;
    workgroupBarrier();
    if lid == 0u {
        var total = 0u;
        for (var j = 0u; j < SCAN_THREADS; j++) {
            let c = wg_light_offsets[j];
            wg_light_offsets[j] = total;
            total += c;
        }
        atomicStore(&wg_light_count, total);
    }
    workgroupBarrier();
    var light_slot = wg_light_offsets[lid];
    for (var i = light_begin; i < light_end; i++) {
        if lights[i].god_rays_enabled != 0u && light_active(lights[i]) {
            media_list.lights[light_slot] = i;
            light_slot++;
        }
    }
    workgroupBarrier();
    let volume_total = atomicLoad(&wg_volume_count);
    let global_total = atomicLoad(&wg_global_count);
    let local_total = atomicLoad(&wg_local_count);
    let light_total = atomicLoad(&wg_light_count);
    // Rows are unique, so each element's rank is the count of smaller rows.
    if lid < min(volume_total, 64u) {
        let row = wg_volume_rows[lid];
        var rank = 0u;
        for (var j = 0u; j < min(volume_total, 64u); j++) { rank += select(0u, 1u, wg_volume_rows[j] < row); }
        media_list.volumes[rank] = row;
    }
    if lid < min(global_total, 64u) {
        let row = wg_global_rows[lid];
        var rank = 0u;
        for (var j = 0u; j < min(global_total, 64u); j++) { rank += select(0u, 1u, wg_global_rows[j] < row); }
        media_list.globals[rank] = row;
    }
    if lid < min(local_total, 64u) {
        let row = wg_local_rows[lid];
        var rank = 0u;
        for (var j = 0u; j < min(local_total, 64u); j++) { rank += select(0u, 1u, wg_local_rows[j] < row); }
        media_list.locals[rank] = row;
    }
    if lid != 0u { return; }

    media_list.volume_count = volume_total;
    media_list.global_count = global_total;
    media_list.local_count = local_total;
    media_list.light_count = light_total;
    // Lights (structural) and media (by value) share one commutative hash.
    let light_hash = atomicLoad(&wg_light_hash) ^ light_total;
    if media_list.previous_light_hash != light_hash { media_list.history_compatible = 0u; }
    media_list.previous_light_hash = light_hash;
    media_list.has_medium = select(0u, 1u, media_list.render_enabled != 0u &&
        ((fog.fog_enabled != 0u && fog.fog_density > 0.0) || media_list.volume_count + media_list.global_count + media_list.local_count > 0u));
    let was_active = media_list.last_active;
    if media_list.has_medium != media_list.last_active { media_list.history_compatible = 0u; }
    media_list.last_active = media_list.has_medium;
    let tiles = (media_list.grid.xy + vec2<u32>(7)) / 8u;
    dispatch[0] = tiles.x * media_list.has_medium;
    dispatch[1] = tiles.y;
    dispatch[2] = media_list.grid.z;
    dispatch[3] = tiles.x * media_list.has_medium;
    dispatch[4] = tiles.y;
    dispatch[5] = (media_list.grid.z + 3u) / 4u;
    // Integration rewrites the whole grid. With no medium this frame or last,
    // the grid already holds the neutral (0,0,0,1) and needs no work at all.
    let integrate = select(0u, 1u, media_list.has_medium != 0u || was_active != 0u);
    dispatch[6] = ((fog_globals.grid.x + 7u) / 8u) * integrate;
    dispatch[7] = (fog_globals.grid.y + 7u) / 8u;
    dispatch[8] = 1u;
    // Published as fog_parameters.max_distance. A grid that needs no
    // integration is uniformly neutral (0,0,0,1), so compositing it is the
    // identity: a zero range tells consumers to pass the image through
    // instead of sampling the grid for every pixel.
    dispatch[9] = bitcast<u32>(select(0.0, fog.fog_max_distance, integrate != 0u));
}

// An overflow scans all rows instead of silently losing light or extinction.
fn volume_count() -> u32 { return select(media_list.volume_count, arrayLength(&volumes), media_list.volume_count > 64u); }
fn global_count() -> u32 { return select(media_list.global_count, arrayLength(&global_media), media_list.global_count > 64u); }
fn local_count() -> u32 { return select(media_list.local_count, arrayLength(&local_media), media_list.local_count > 64u); }
fn light_count() -> u32 { return media_list.light_count; }
fn volume_index(i: u32) -> u32 { if media_list.volume_count > 64u { return i; } return media_list.volumes[i]; }
fn global_index(i: u32) -> u32 { if media_list.global_count > 64u { return i; } return media_list.globals[i]; }
fn local_index(i: u32) -> u32 { if media_list.local_count > 64u { return i; } return media_list.locals[i]; }
fn light_index(i: u32) -> u32 { return media_list.lights[i]; }

fn cluster_index(tile: vec3<u32>) -> u32 {
    let tiles = (media_list.grid.xy + vec2<u32>(7)) / 8u;
    return (tile.z * tiles.y + tile.y) * tiles.x + tile.x;
}

@compute @workgroup_size(1)
fn cs_cull(@builtin(global_invocation_id) tile: vec3<u32>) {
    let cell_min = tile * vec3<u32>(8, 8, 4);
    let cell_max = min(cell_min + vec3<u32>(8, 8, 4), media_list.grid);
    var lo = vec3<f32>(1e30);
    var hi = vec3<f32>(-1e30);
    // The frustum cell's eight corners conservatively bound every jittered sample.
    for (var corner = 0u; corner < 8u; corner++) {
        let c = vec3<u32>(select(cell_min.x, cell_max.x, (corner & 1u) != 0u),
            select(cell_min.y, cell_max.y, (corner & 2u) != 0u),
            select(cell_min.z, cell_max.z, (corner & 4u) != 0u));
        let uvw = vec3<f32>(c) / vec3<f32>(media_list.grid);
        let p = froxel_world_pos(uvw.xy, uvw.z);
        lo = min(lo, p); hi = max(hi, p);
    }
    let id = cluster_index(tile);
    var count = 0u;
    for (var i = 0u; i < light_count(); i++) {
        let index = light_index(i);
        let l = lights[index];
        if !light_active(l) { continue; }
        let delta = l.position_range.xyz - clamp(l.position_range.xyz, lo, hi);
        if l.light_type != LIGHT_DIRECTIONAL && dot(delta, delta) > l.position_range.w * l.position_range.w { continue; }
        if count < 64u { clusters[id].lights[count] = index; }
        count++;
    }
    clusters[id].count = count;
}

// Integration reads the scattering grid written above and writes the accumulated
// result. Separate group so the two dispatches can swap only what differs.
@group(1) @binding(0) var scatter_in:     texture_3d<f32>;
/// rgb = accumulated in-scattering, a = transmittance.
@group(1) @binding(1) var integrated_out: texture_storage_3d<rgba16float, write>;

// ── Density ─────────────────────────────────────────────────────────────────

// No planet: heights along world Y (the post-process media).
const FLAT: vec4<f32> = vec4<f32>(0.0);

// A world position in the frame's space; the large terms cancel first.
fn in_frame(world: vec3<f32>) -> vec3<f32> {
    return (world - fog_globals.origin_hi.xyz) - fog_globals.origin_lo.xyz;
}

// Height of frame point `p`: its altitude over `planet` (world centre,
// radius), or its world Y without one.
fn medium_height(p: vec3<f32>, planet: vec4<f32>) -> f32 {
    if planet.w > 0.0 { return length(p - in_frame(planet.xyz)) - planet.w; }
    return (p.y + fog_globals.origin_hi.y) + fog_globals.origin_lo.y;
}

// Up at frame point `p`: away from `planet`'s centre, or +Y without one.
fn medium_up(p: vec3<f32>, planet: vec4<f32>) -> vec3<f32> {
    if planet.w > 0.0 { return normalize(p - in_frame(planet.xyz)); }
    return vec3<f32>(0.0, 1.0, 0.0);
}

fn density_at(p: vec3<f32>, config: FogUniforms, planet: vec4<f32>) -> f32 {
    var density = max(config.fog_density, 0.0);
    if config.fog_mode != FOG_MODE_UNIFORM {
        // Full density at or below fog_height, decaying exponentially above it.
        // max(h, 0) rather than h: without the clamp a point far below the base
        // gets exp(+large) and the grid fills with an opaque wall.
        let h = medium_height(p, planet) - config.fog_height;
        density *= exp(-max(h, 0.0) * max(config.fog_height_falloff, 0.0));
    }
    if config.fog_mode == 2u {
        let advected = (p + fog_globals.noise_phase.xyz) * SMOKE_SCALE - SMOKE_DRIFT * fog_globals.time;
        let coarse = smoke_noise(advected);
        let detail = smoke_noise(advected * 2.07 + vec3<f32>(coarse * 1.7));
        let billow = smoothstep(0.25, 0.72, coarse * 0.7 + detail * 0.3);
        density *= billow * billow * 2.8;
    }
    return density;
}

fn smoke_hash(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.yzx + vec3<f32>(33.33));
    return fract((q.x + q.y) * q.z);
}

fn smoke_noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(mix(smoke_hash(i), smoke_hash(i + vec3<f32>(1,0,0)), u.x),
            mix(smoke_hash(i + vec3<f32>(0,1,0)), smoke_hash(i + vec3<f32>(1,1,0)), u.x), u.y),
        mix(mix(smoke_hash(i + vec3<f32>(0,0,1)), smoke_hash(i + vec3<f32>(1,0,1)), u.x),
            mix(smoke_hash(i + vec3<f32>(0,1,1)), smoke_hash(i + vec3<f32>(1,1,1)), u.x), u.y), u.z);
}

struct Medium {
    extinction: f32,
    albedo: vec3<f32>,
    emissive: vec3<f32>,
    anisotropy: f32,
    scattering_weight: f32,
    /// Extinction-weighted share (0..1) of this sample that is animated smoke.
    smoke: f32,
}

/// World-space velocity of the smoke pattern: density_at samples noise at
/// p * SMOKE_SCALE - SMOKE_DRIFT * time, so features travel at DRIFT / SCALE.
const SMOKE_SCALE: f32 = 0.24;
const SMOKE_DRIFT: vec3<f32> = vec3<f32>(0.10, 0.24, 0.055);
fn smoke_velocity() -> vec3<f32> { return SMOKE_DRIFT / SMOKE_SCALE; }

fn world_config(m: WorldMedium) -> FogUniforms {
    var f: FogUniforms;
    f.fog_enabled = m.enabled;
    f.fog_mode = m.mode;
    f.fog_density = finite_clamp(m.extinction, 0.0, 10000.0);
    f.fog_height_falloff = finite_clamp(m.height_falloff, 0.0, 10000.0);
    f.fog_height = m.height;
    return f;
}

fn local_shape(p: vec3<f32>, v: LocalMedium) -> f32 {
    if any(p < v.bounds_min.xyz) || any(p > v.bounds_max.xyz) { return 0.0; }
    if v.edge_fade <= 0.0 { return 1.0; }
    let edge = min(p - v.bounds_min.xyz, v.bounds_max.xyz - p);
    return smoothstep(0.0, v.edge_fade, min(edge.x, min(edge.y, edge.z)));
}

fn add_world_medium(accum: Medium, m: WorldMedium, p: vec3<f32>, shape: f32) -> Medium {
    var result = accum;
    var config = world_config(m);
    config.fog_density = 1.0;
    let profile = density_at(p, config, m.planet) * shape;
    let d = finite_clamp(m.extinction, 0.0, 10000.0) * profile;
    let scattering = clamp(m.albedo, vec3<f32>(0), vec3<f32>(1)) * d;
    result.extinction += d;
    if m.mode == 2u { result.smoke += d; }
    result.albedo += scattering;
    result.emissive += max(m.emission, vec3<f32>(0)) * profile;
    // Scattering-weighted phase, not absorption-weighted phase.
    let weight = dot(scattering, vec3<f32>(1.0 / 3.0));
    result.anisotropy += finite_clamp(m.anisotropy, -0.95, 0.95) * weight;
    result.scattering_weight += weight;
    return result;
}

fn medium_at(p: vec3<f32>, view_depth: f32) -> Medium {
    var m: Medium;
    if fog.fog_enabled != 0u && view_depth >= fog.fog_start_distance {
        m.extinction = density_at(p, fog, FLAT);
        if fog.fog_mode == 2u { m.smoke = m.extinction; }
        m.albedo = clamp(fog.fog_color, vec3<f32>(0.0), vec3<f32>(1.0)) * m.extinction;
        m.emissive = fog.fog_emissive * m.extinction;
        m.scattering_weight = dot(m.albedo, vec3<f32>(1.0 / 3.0));
        m.anisotropy = fog.fog_scattering_anisotropy * m.scattering_weight;
    }
    for (var i = 0u; i < volume_count(); i++) {
        let v = volumes[volume_index(i)];
        if !legacy_volume_active(v) { continue; }
        if any(p < v.bounds_min.xyz) || any(p > v.bounds_max.xyz) { continue; }
        // A bounded volume's box is its extent; its authored max distance is the
        // grid range (resolve), not a second cutoff that would clip the box.
        if view_depth < v.medium.fog_start_distance { continue; }
        let edge = min(p - v.bounds_min.xyz, v.bounds_max.xyz - p);
        let fade = smoothstep(0.0, max(v.blend_radius, 0.001), min(edge.x, min(edge.y, edge.z)));
        var shape = fade;
        if v.medium.fog_mode == 2u {
            // A billowing pocket tapers before the AABB corners; the box is
            // still its authoritative extent, not an opaque rectangular slab.
            let q = (2.0 * p - v.bounds_min.xyz - v.bounds_max.xyz) / (v.bounds_max.xyz - v.bounds_min.xyz);
            shape *= 1.0 - smoothstep(0.35, 1.0, dot(q, q));
        }
        let d = density_at(p, v.medium, FLAT) * clamp(v.blend_weight, 0.0, 1.0) * shape;
        // Overlapping media add extinction/scattering, not camera-weighted screen effects.
        m.extinction += d;
        if v.medium.fog_mode == 2u { m.smoke += d; }
        m.albedo += clamp(v.medium.fog_color, vec3<f32>(0.0), vec3<f32>(1.0)) * d;
        m.emissive += v.medium.fog_emissive * d;
        let weight = dot(clamp(v.medium.fog_color, vec3<f32>(0), vec3<f32>(1)) * d, vec3<f32>(1.0 / 3.0));
        m.anisotropy += v.medium.fog_scattering_anisotropy * weight;
        m.scattering_weight += weight;
    }
    for (var i = 0u; i < global_count(); i++) {
        let config = global_media[global_index(i)];
        if world_medium_active(config) { m = add_world_medium(m, config, p, 1.0); }
    }
    for (var i = 0u; i < local_count(); i++) {
        let v = local_media[local_index(i)];
        if local_medium_active(v) { m = add_world_medium(m, v.medium, p, local_shape(p, v)); }
    }
    if m.extinction > 0.0 { m.albedo /= m.extinction; m.smoke = clamp(m.smoke / m.extinction, 0.0, 1.0); }
    if m.scattering_weight > 0.0 { m.anisotropy /= m.scattering_weight; }
    return m;
}

// Clip the light segment to each local volume before quadrature; a small dense
// cloud must not disappear between samples of a much longer light ray.
fn ray_box(p: vec3<f32>, dir: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>, distance: f32) -> vec2<f32> {
    var interval = vec2<f32>(0.0, distance);
    for (var axis = 0u; axis < 3u; axis++) {
        if abs(dir[axis]) < 1e-8 {
            if p[axis] < lo[axis] || p[axis] > hi[axis] { return vec2<f32>(0); }
        } else {
            let a = (lo[axis] - p[axis]) / dir[axis];
            let b = (hi[axis] - p[axis]) / dir[axis];
            interval.x = max(interval.x, min(a, b));
            interval.y = min(interval.y, max(a, b));
        }
    }
    return vec2<f32>(interval.x, max(interval.x, interval.y));
}

fn global_optical_depth(config: FogUniforms, planet: vec4<f32>, p: vec3<f32>, dir: vec3<f32>, distance: f32) -> f32 {
    if config.fog_mode == 0u { return max(config.fog_density, 0.0) * distance; }
    if config.fog_mode != 2u {
        // Integrate the exponential height profile analytically. Order the
        // endpoints by height so the exponent never grows or overflows. Over
        // a planet, in the plane tangent at `p` (light segments are far
        // shorter than the planet's radius).
        let rise = dot(dir, medium_up(p, planet));
        let slope = abs(rise);
        if slope < 1e-8 { return density_at(p, config, planet) * distance; }
        let height = medium_height(p, planet);
        let low = min(height, height + rise * distance);
        let below = clamp((config.fog_height - low) / slope, 0.0, distance);
        let falloff = max(config.fog_height_falloff, 0.0);
        let start = exp(-max(low + below * slope - config.fog_height, 0.0) * falloff);
        return max(config.fog_density, 0.0) * (below + start * segment_integral(falloff * slope, distance - below));
    }
    let count = max(media_list.light_samples, 1u);
    let step = distance / f32(count);
    var tau = 0.0;
    for (var i = 0u; i < count; i++) {
        tau += density_at(p + dir * ((f32(i) + 0.5) * step), config, planet) * step;
    }
    return tau;
}

fn medium_transmittance(p: vec3<f32>, dir: vec3<f32>, distance: f32) -> f32 {
    var tau = 0.0;
    if fog.fog_enabled != 0u { tau += global_optical_depth(fog, FLAT, p, dir, distance); }
    for (var i = 0u; i < global_count(); i++) {
        let m = global_media[global_index(i)];
        if world_medium_active(m) { tau += global_optical_depth(world_config(m), m.planet, p, dir, distance); }
    }
    let count = max(media_list.light_samples, 1u);
    for (var i = 0u; i < local_count(); i++) {
        let v = local_media[local_index(i)];
        if !local_medium_active(v) { continue; }
        let segment = ray_box(p, dir, v.bounds_min.xyz, v.bounds_max.xyz, distance);
        let length = segment.y - segment.x;
        if length <= 0.0 { continue; }
        let config = world_config(v.medium);
        if v.edge_fade <= 0.0 {
            tau += global_optical_depth(config, v.medium.planet, p + dir * segment.x, dir, length);
        } else {
            let step = length / f32(count);
            for (var j = 0u; j < count; j++) {
                let q = p + dir * (segment.x + (f32(j) + 0.5) * step);
                tau += density_at(q, config, v.medium.planet) * local_shape(q, v) * step;
            }
        }
    }
    for (var i = 0u; i < volume_count(); i++) {
        let v = volumes[volume_index(i)];
        if !legacy_volume_active(v) { continue; }
        let segment = ray_box(p, dir, v.bounds_min.xyz, v.bounds_max.xyz, distance);
        let step = (segment.y - segment.x) / f32(count);
        for (var j = 0u; j < count; j++) {
            let q = p + dir * (segment.x + (f32(j) + 0.5) * step);
            let edge = min(q - v.bounds_min.xyz, v.bounds_max.xyz - q);
            var shape = smoothstep(0.0, max(v.blend_radius, 0.001), min(edge.x, min(edge.y, edge.z)));
            if v.medium.fog_mode == 2u {
                let relative = (2.0 * q - v.bounds_min.xyz - v.bounds_max.xyz) / (v.bounds_max.xyz - v.bounds_min.xyz);
                shape *= 1.0 - smoothstep(0.35, 1.0, dot(relative, relative));
            }
            tau += density_at(q, v.medium, FLAT) * clamp(v.blend_weight, 0.0, 1.0) * shape * step;
        }
    }
    return exp(-min(tau, 80.0));
}

// ── Froxel <-> world ────────────────────────────────────────────────────────

/// World-space ray through an NDC point: the unprojected point at infinity
/// (reversed depth 0) minus the eye, in homogeneous form. Dividing by its
/// w == 0 would give NaN; this stays exact and cannot disagree with the depth
/// reconstruction the rest of the engine does.
fn pixel_ray(ndc: vec2<f32>) -> vec3<f32> {
    let p_far = cameras[0].view_proj_inv * vec4<f32>(ndc, 0.0, 1.0);
    return normalize(p_far.xyz - p_far.w * cameras[0].position_near.xyz);
}

/// World position at the centre of a froxel, given normalized grid coords.
///
/// `slice_norm` maps through the prelude's exponential distribution, so this is
/// the exact inverse of what the composite does to find a slice from a depth.
fn froxel_world_pos(uv: vec2<f32>, slice_norm: f32) -> vec3<f32> {
    // The grid is anchored to the UNJITTERED frustum. With TSR the camera
    // matrices carry a sub-pixel jitter that changes every frame; a grid that
    // moves with it is resampled at the jitter delta each frame, and steep
    // gradients (a lamp's glow, shaft edges) then oscillate: shimmer that grows
    // with resolution. Adding the jitter back unprojects through the jittered
    // inverse to the unjittered ray.
    let ndc = helio_uv_to_ndc(uv) + cameras[0].jitter_frame.xy;

    let dir = pixel_ray(ndc);

    let view_depth = helio_froxel_view_depth_from_slice(slice_norm, fog.fog_max_distance);

    // Slices are planes of constant *view depth*, not spheres of constant radius,
    // so the radial distance along this ray is view_depth / cos(angle to forward).
    // Skipping this bows the grid toward the camera at the screen edges.
    let fwd = normalize(cameras[0].forward_far.xyz);
    let cos_a = max(dot(dir, fwd), 1e-4);
    return cameras[0].position_near.xyz + dir * (view_depth / cos_a);
}

// ── Shadowing ───────────────────────────────────────────────────────────────

// Same face order and tie policy as deferred_lighting.wgsl / HLFS and the
// shadow-matrix producer: +X, -X, +Y, -Y, +Z, -Z. Projection uses the shared
// helio_shadow_project helper, including atlas Y flip and validity checks.
fn point_light_face(dir: vec3<f32>) -> u32 {
    let a = abs(dir);
    if a.x >= a.y && a.x >= a.z { return select(0u, 1u, dir.x < 0.0); }
    if a.y >= a.x && a.y >= a.z { return select(2u, 3u, dir.y < 0.0); }
    return select(4u, 5u, dir.z < 0.0);
}

/// Fraction of `light_idx` reaching `p`. 1.0 = fully lit: its shadow maps,
/// and for the sun the terrain's occlusion (shafts through canopies and past
/// ridges of terrain the shadow maps do not hold) and the clouds' shadow.
fn shaft_visibility(light_idx: u32, p0: vec3<f32>) -> vec3<f32> {
    var lit = shadow_map_visibility(light_idx, p0);
    if lights[light_idx].light_type == LIGHT_DIRECTIONAL {
        let from_camera = p0 - cameras[0].position_near.xyz;
        // A froxel's footprint: a tenth of its distance (fog's shafts are
        // soft; coarse levels are resident around the eye).
        lit *= sun_occlusion_point(from_camera, 0.1 * length(from_camera)) * cloud_shadow(from_camera);
    }
    return lit;
}

/// Fraction of `light_idx` reaching `p` past its shadow maps' casters.
///
/// One comparison tap, not the PCF/PCSS kernel deferred lighting uses. That is
/// affordable because it runs once per froxel rather than once per pixel per
/// step, and the temporal blend averages the result across frames.
fn shadow_map_visibility(light_idx: u32, p0: vec3<f32>) -> vec3<f32> {
    if textureDimensions(shadow_atlas).x <= 1u { return vec3<f32>(1.0); }
    let light = lights[light_idx];
    if light.shadow_index == NO_SHADOW { return vec3<f32>(1.0); }

    // Fog has no surface normal to offset along, so receivers lying in or near
    // a caster (the floor under a local volume, a mesh inside it) compare
    // against their own depth and flip lit/shadowed by quantisation: diagonal
    // acne stripes that crawl as the sample lattice moves with the camera.
    // Slide the query toward the light by about one froxel, which keeps a cell
    // from shadowing itself without visibly detaching shafts from casters.
    var toward_light: vec3<f32>;
    if light.light_type == LIGHT_DIRECTIONAL {
        toward_light = normalize(-light.direction_outer.xyz);
    } else {
        toward_light = normalize(light.position_range.xyz - p0);
    }
    let p = p0 + toward_light * clamp(max(sample_lateral, 0.03) * 1.25, 0.03, 2.5);

    var layer = light.shadow_index;

    if light.light_type == LIGHT_DIRECTIONAL {
        let dist = length(p - cameras[0].position_near.xyz);
        let sel = helio_csm_select(dist, fog_globals.csm_splits);
        layer = light.shadow_index + sel.cascade_a;
    } else if light.light_type == LIGHT_POINT {
        layer = light.shadow_index + point_light_face(p - light.position_range.xyz);
    }

    if layer >= arrayLength(&shadow_matrices) { return vec3<f32>(1.0); }
    let proj = helio_shadow_project(shadow_matrices[layer].mat, p);
    // Outside the map or behind the light: lit, not shadowed. Returning 0.0 would
    // ring the fog with a black shell wherever the cascade ends.
    if !proj.valid { return vec3<f32>(1.0); }

    // Dynamic (movable) and cached static casters, as deferred lighting does.
    let lit = min(budget_compare_dynamic(proj.uv, u32(layer), proj.depth),
                  budget_compare_static(proj.uv, u32(layer), proj.depth));
    if lit <= 0.0 || budget_layer(layer) >= textureNumLayers(shadow_transmittance) { return vec3<f32>(lit); }
    // Light that crossed stained glass arrives coloured: the shafts take the
    // panes' tint, not just their outline.
    return lit * glass_tint(proj.uv, i32(layer), 1.0 - proj.depth);
}

// Tint of the stained-glass panes a receiver lies behind. Each of the four
// nearest texels is depth-tested on its own, then the results are weighted
// bilinearly. Bilinearly filtering the pane depth (alpha) first and testing
// once failed the test along every pane edge wherever a neighbouring texel
// held no pane: an untinted rim and stair-stepped edges, most visible in the
// coarse cascades, where a half-resolution texel covers several centimetres.
fn glass_tint(uv:vec2f, layer:i32, receiver:f32)->vec3f {
    let face=u32(layer);
    if !budget_valid(face,16u) {return vec3f(1);}
    let physical=i32(budget_layer(face));
    if physical>=i32(textureNumLayers(shadow_transmittance)) {return vec3f(1);}
    let dims=vec2f(textureDimensions(shadow_transmittance));
    let p=budget_uv(uv,face,dims)*dims-0.5;
    let base=floor(p);let f=fract(p);var tint=vec3f(0);
    let m=shadow_matrices[face];
    let offset=select(m.atlas.xy,vec2f(0),m.policy.w==0u);
    let size=select(m.atlas.z,1.0,m.policy.w==0u);
    for(var i=0;i<4;i++) {
        let o=vec2i(i&1,i>>1);
        let pixel=clamp(vec2i(base)+o,vec2i(offset*dims),vec2i((offset+vec2f(size))*dims)-1);
        let t=textureLoad(shadow_transmittance,pixel,physical,0);
        let w=select(1.0-f.x,f.x,o.x==1)*select(1.0-f.y,f.y,o.y==1);
        tint+=w*select(vec3f(1),1.0-t.rgb,receiver<t.a);
    }
    return mix(vec3f(1),tint,budget_strength(face));
}

// ── Light evaluation ────────────────────────────────────────────────────────

/// In-scattered radiance from one light at `p`, for a view ray `ray_dir`.
///
/// `god_rays_weight` / `god_rays_exposure` / `god_rays_density` come from the
/// radial-blur god-ray technique and have no physical meaning here; they are kept
/// as artistic multipliers so existing light setups author the same way.
/// `god_rays_decay` is the migrated volumetric geometric-shadow strength.
/// Editor-native lights use weight as their single artistic gain, density and
/// exposure equal to one, and enabled as their participation flag.
/// The froxel one injected sample stands for: its view ray segment [t0, t1]
/// (radial distances from the camera) and lateral half-width. Unset (t1 <= t0)
/// outside cs_inject, e.g. for point probes, which then evaluate at `p`.
var<private> sample_t0: f32 = 0.0;
var<private> sample_t1: f32 = 0.0;
var<private> sample_lateral: f32 = 0.0;
/// The froxel's centre ray (unjittered), along which local lights are evaluated.
var<private> sample_center_dir: vec3<f32> = vec3<f32>(0.0);

/// Mean of 1/r^2 from a point light over the froxel's ray segment, in closed
/// form: with h the light's distance from the ray and tc its projection,
/// int dt / (h^2 + (t - tc)^2) = atan((t - tc) / h) / h. Point-sampling the
/// jittered depth instead gave samples near a lamp wildly different radiance
/// (a froxel column is metres deep), which history cannot average out in
/// time: visible shimmer around every lamp in the medium. The lateral extent
/// softens h (mean of 1/r^2 over a disc), so the lamp centre stays finite.
fn segment_inverse_square(light_pos: vec3<f32>) -> f32 {
    let ray_dir = sample_center_dir;
    let origin = cameras[0].position_near.xyz;
    let to_light = light_pos - origin;
    let tc = dot(to_light, ray_dir);
    let h2 = max(dot(to_light, to_light) - tc * tc, 0.0) + sample_lateral * sample_lateral / 3.0;
    let h = sqrt(max(h2, 1e-8));
    let span = max(sample_t1 - sample_t0, 1e-6);
    return (atan((sample_t1 - tc) / h) - atan((sample_t0 - tc) / h)) / (h * span);
}

/// The froxel's stratified shadow taps (set by cs_inject). Unused outside it.
var<private> sample_taps: array<vec3<f32>, 4>;

fn shaft_visibility_averaged(light_idx: u32, p: vec3<f32>) -> vec3<f32> {
    if sample_t1 <= sample_t0 { return shaft_visibility(light_idx, p); }
    var sum = vec3<f32>(0.0);
    for (var k = 0u; k < 4u; k++) { sum += shaft_visibility(light_idx, sample_taps[k]); }
    return sum * 0.25;
}

/// Multiple-scattering approximation (Wrenninge, "Art-directable multiple
/// scattering"; used for production clouds and fog, e.g. Hillaire 2015/2020):
/// octave n contributes a^n * T^(b^n) * phase(g * c^n), so light that has been
/// scattered several times still reaches the interior of a dense medium, with a
/// softer, less forward-peaked lobe. Single scattering alone leaves any thick
/// self-shadowed volume black at its core. a = b = c = 0.5 conserves energy
/// (a <= b); the sum is normalised so an unshadowed point (T = 1) keeps its
/// single-scattering brightness.
const MS_OCTAVES: u32 = 4u;
fn multiple_scattering(transmittance: f32, cos_theta: f32, g: f32) -> f32 {
    let ln_t = log(max(transmittance, 1e-30));
    var sum = 0.0;
    var norm = 0.0;
    var a = 1.0;
    var b = 1.0;
    var c = 1.0;
    for (var n = 0u; n < MS_OCTAVES; n++) {
        sum += a * exp(ln_t * b) * helio_hg_phase(cos_theta, g * c);
        norm += a;
        a *= 0.5;
        b *= 0.5;
        c *= 0.5;
    }
    return sum / norm;
}

fn inscatter_from_light(light_idx: u32, p: vec3<f32>, ray_dir: vec3<f32>, anisotropy: f32) -> vec3<f32> {
    let light = lights[light_idx];

    // Opt-in per light: each one costs a shadow tap per froxel.
    if !light_active(light) { return vec3<f32>(0.0); }

    var to_light: vec3<f32>;
    var atten = 1.0;
    var shadow_distance = media_list.light_max_distance;
    var view_dir = ray_dir;
    // Where the medium's transmittance toward the light is evaluated.
    var eval_pos = p;

    if light.light_type == LIGHT_DIRECTIONAL {
        if dot(light.direction_outer.xyz, light.direction_outer.xyz) < 1e-12 { return vec3<f32>(0); }
        to_light = normalize(-light.direction_outer.xyz);
    } else {
        // Local lights are evaluated deterministically on the froxel's centre
        // ray: 1/r^2 is integrated over the segment in closed form, and the
        // direction (phase, cone, range window) is taken at the segment point
        // nearest the light, which dominates that integral. Near a lamp the
        // direction to it swings through large angles inside one froxel, so a
        // jittered point made the forward-peaked phase differ tenfold between
        // frames: shimmer in every lamp's glow. Shadow visibility still uses
        // the jittered p, which antialiases shadow edges over time.
        var q = p;
        if sample_t1 > sample_t0 {
            let origin = cameras[0].position_near.xyz;
            let t = clamp(dot(light.position_range.xyz - origin, sample_center_dir), sample_t0, sample_t1);
            q = origin + sample_center_dir * t;
            view_dir = sample_center_dir;
        }
        let delta = light.position_range.xyz - q;
        let dist = length(delta);
        // Transmittance toward the lamp is taken from the same deterministic
        // point: from the jittered p, exp(-sigma * distance) alone varied by a
        // few percent per frame in dense haze.
        eval_pos = q;
        shadow_distance = dist;
        let range = max(light.position_range.w, 1e-4);
        if dist > range { return vec3<f32>(0.0); }
        to_light = delta / max(dist, 1e-6);

        // Inverse-square with a windowed cutoff, so the contribution reaches zero
        // exactly at the range boundary instead of popping. Inside cs_inject the
        // froxel stores the segment mean of 1/r^2 (see segment_inverse_square).
        let window = clamp(1.0 - pow(dist / range, 4.0), 0.0, 1.0);
        var inverse_square = 1.0 / max(dist * dist, 1e-4);
        if sample_t1 > sample_t0 {
            inverse_square = segment_inverse_square(light.position_range.xyz);
        }
        atten = window * window * inverse_square;

        if light.light_type == LIGHT_SPOT {
            if dot(light.direction_outer.xyz, light.direction_outer.xyz) < 1e-12 { return vec3<f32>(0); }
            let cd = dot(-to_light, normalize(light.direction_outer.xyz));
            let outer = light.direction_outer.w;
            let inner = light.inner_angle;
            let spot = clamp((cd - outer) / max(inner - outer, 1e-4), 0.0, 1.0);
            atten *= spot * spot;
        }
    }

    // cos = 1 looking straight at the light, so g > 0 peaks into the sun.
    let cos_theta = dot(view_dir, to_light);
    let g = clamp(anisotropy, -0.95, 0.95);
    // ABI adapter: decay was unused; it now controls geometric fog-shadow
    // strength. Medium absorption remains physical even with geometry opt-out.
    let shadow_strength = finite_clamp(light.god_rays_decay, 0.0, 1.0);
    var vis = vec3<f32>(1.0);
    if shadow_strength > 0.0 { vis = mix(vec3<f32>(1.0), shaft_visibility_averaged(light_idx, p), shadow_strength); }
    if all(vis <= vec3<f32>(0.0)) || atten <= 0.0 { return vec3<f32>(0); }
    let scatter = multiple_scattering(medium_transmittance(eval_pos, to_light, shadow_distance), cos_theta, g);

    let radiance = light.color_intensity.rgb * light.color_intensity.w;
    return radiance * atten * scatter * vis
        * light.god_rays_weight * light.god_rays_exposure * light.god_rays_density;
}

// ── Temporal reprojection ───────────────────────────────────────────────────

/// Previous frame's scattering at world position `p`, or `none` if it reprojects
/// off-grid.
///
/// Returns w < 0 to signal "no history" — a froxel that was off-screen or behind
/// the camera last frame has nothing to blend with, and reusing a clamped edge
/// sample there smears fog across the screen edges as the camera turns.
fn sample_history(p: vec3<f32>) -> vec4<f32> {
    let prev_clip = cameras[0].prev_view_proj * vec4<f32>(p, 1.0);
    // For the engine's perspective matrix, clip.w is the positive view depth.
    if prev_clip.w <= HELIO_FROXEL_NEAR { return vec4<f32>(0.0, 0.0, 0.0, -1.0); }

    // prev_view_proj is last frame's jittered matrix: remove its jitter to land
    // in last frame's (unjittered) grid.
    let prev_ndc = prev_clip.xyz / prev_clip.w;
    let prev_uv = helio_ndc_to_uv(prev_ndc.xy - vec2<f32>(fog_globals.prev_jitter_x, fog_globals.prev_jitter_y));
    if any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0)) {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }

    let prev_slice = helio_froxel_slice_from_view_depth(prev_clip.w, fog.fog_max_distance);
    if prev_slice < 0.0 || prev_slice > 1.0 {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }

    return textureSampleLevel(scatter_history, linear_samp,
        scatter_coordinates(vec3<f32>(prev_uv, prev_slice), textureDimensions(scatter_history)), 0.0);
}

fn scatter_coordinates(uvw: vec3<f32>, physical: vec3<u32>) -> vec3<f32> {
    let logical = vec3<f32>(media_list.grid);
    return clamp(uvw * logical, vec3<f32>(0.5), logical - 0.5) / vec3<f32>(physical);
}

fn temporal_result(current: vec4<f32>, history: vec4<f32>, blend: f32, rejection: f32) -> vec4<f32> {
    // Never resurrect removed media, vanished lights, or residual emission.
    if all(current == vec4<f32>(0)) || all(current.rgb == vec3<f32>(0)) { return current; }
    // Only a medium appearing or vanishing rejects history. Lighting is never
    // compared: each sample is jittered inside its froxel, so across a shaft
    // edge successive samples legitimately differ by 100%, and rejecting on
    // that is exactly what left the raw per-frame noise on screen.
    let density_change = abs(history.a - current.a) / max(max(history.a, current.a), 1e-8);
    let weight = max(blend, smoothstep(rejection, min(rejection + 0.15, 1.0), density_change));
    return mix(history, current, clamp(weight, 0.0, 1.0));
}

// ── Injection ───────────────────────────────────────────────────────────────

/// The medium averaged over the froxel's volume: 2x2x2 fixed sub-positions.
/// A hard-edged box sampled once per cell is a binary function, so its outline
/// becomes a staircase as wide as a froxel; averaging gives partial coverage at
/// the boundary (a ramp) while staying fully deterministic frame to frame.
/// Sums are of extensive quantities (scattering, emission, weighted phase), so
/// the result is the true volume average, not an average of ratios.
fn medium_averaged(gid: vec3<u32>, dims: vec3<u32>, centre: vec3<f32>, view_depth: f32) -> Medium {
    var extinction = 0.0;
    var scattering = vec3<f32>(0.0);
    var emissive = vec3<f32>(0.0);
    var phase_weight = 0.0;
    var anisotropy = 0.0;
    var smoke = 0.0;
    for (var k = 0u; k < 8u; k++) {
        let sub = vec3<f32>(select(0.25, 0.75, (k & 1u) != 0u), select(0.25, 0.75, (k & 2u) != 0u),
            select(0.25, 0.75, (k & 4u) != 0u));
        let q = froxel_world_pos((vec2<f32>(gid.xy) + sub.xy) / vec2<f32>(dims.xy),
            (f32(gid.z) + sub.z) / f32(dims.z));
        let m = medium_at(q, view_depth);
        extinction += m.extinction;
        scattering += m.albedo * m.extinction;
        emissive += m.emissive;
        anisotropy += m.anisotropy * m.scattering_weight;
        phase_weight += m.scattering_weight;
        smoke += m.smoke * m.extinction;
    }
    var result: Medium;
    result.extinction = extinction * 0.125;
    if extinction > 0.0 {
        result.albedo = scattering / extinction;
        result.smoke = clamp(smoke / extinction, 0.0, 1.0);
    }
    result.emissive = emissive * 0.125;
    result.scattering_weight = phase_weight * 0.125;
    if phase_weight > 0.0 { result.anisotropy = anisotropy / phase_weight; }
    return result;
}

@compute @workgroup_size(8, 8, 1)
fn cs_inject(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = media_list.grid;
    if any(gid >= dims) { return; }

    let coord = vec3<i32>(gid);

    if media_list.has_medium == 0u {
        textureStore(scatter_out, coord, vec4<f32>(0.0));
        return;
    }

    // Deterministic sampling: no per-frame or per-cell randomness, so a static
    // camera and scene give bit-identical froxels every frame and nothing can
    // flicker. Medium and phase are evaluated at the froxel centre; shadow
    // visibility averages four fixed taps spread over the cell (2x2 across the
    // screen, four depth strata), which replaces the old jitter without its
    // noise. Far froxels span metres, so a single tap would alias hard.
    // Moving media (smoke) are handled by advecting the history lookup along
    // the wind (see smoke_velocity), not by randomising the samples.
    let uv = (vec2<f32>(gid.xy) + 0.5) / vec2<f32>(dims.xy);
    let slice_norm = (f32(gid.z) + 0.5) / f32(dims.z);

    let p = froxel_world_pos(uv, slice_norm);
    let ray_dir = normalize(p - cameras[0].position_near.xyz);

    for (var k = 0u; k < 4u; k++) {
        let sub = vec2<f32>(select(0.25, 0.75, (k & 1u) != 0u), select(0.25, 0.75, (k & 2u) != 0u));
        let dz = 0.125 + 0.25 * f32(k);
        sample_taps[k] = froxel_world_pos((vec2<f32>(gid.xy) + sub) / vec2<f32>(dims.xy),
            (f32(gid.z) + dz) / f32(dims.z));
    }

    let view_depth = helio_froxel_view_depth_from_slice(slice_norm, fog.fog_max_distance);
    let medium = medium_averaged(gid, dims, p, view_depth);
    // This froxel's ray segment (radial = view depth / cos) and half-width.
    let radial_scale = distance(p, cameras[0].position_near.xyz) / max(view_depth, 1e-6);
    sample_t0 = helio_froxel_view_depth_from_slice(f32(gid.z) / f32(dims.z), fog.fog_max_distance) * radial_scale;
    sample_t1 = helio_froxel_view_depth_from_slice(f32(gid.z + 1u) / f32(dims.z), fog.fog_max_distance) * radial_scale;
    sample_lateral = 0.5 * distance(froxel_world_pos(uv + vec2<f32>(1.0 / f32(dims.x), 0.0), slice_norm), p);
    sample_center_dir = normalize(froxel_world_pos((vec2<f32>(gid.xy) + 0.5) / vec2<f32>(dims.xy), slice_norm)
        - cameras[0].position_near.xyz);
    let density = medium.extinction;
    // Integrate the complete medium. The full-resolution composite stops at
    // each pixel's surface depth; coarse depth rejection leaks at silhouettes.

    var scattering = vec3<f32>(0.0);
    if density > 0.0 {
        var radiance = vec3<f32>(0.0);
        let cluster = cluster_index(gid / vec3<u32>(8, 8, 4));
        let count = clusters[cluster].count;
        let overflow = count > 64u;
        let iterations = select(count, light_count(), overflow);
        for (var li = 0u; li < iterations; li++) {
            var index: u32;
            if overflow { index = light_index(li); } else { index = clusters[cluster].lights[li]; }
            radiance += inscatter_from_light(index, p, ray_dir, medium.anisotropy);
        }
        // With no actual illumination, scattering is zero. Sky/indirect
        // illumination needs an explicit radiance source, never a constant.
        scattering = radiance * medium.albedo * density;
    }
    scattering += medium.emissive;

    var result = vec4<f32>(scattering, density);

    if fog_globals.history_valid != 0u && media_list.history_compatible != 0u {
        // History stores the cell average: read it at the cell centre. Reading
        // it at this frame's jittered position resampled the history at a
        // random sub-froxel offset every frame, which blurred it and kept it
        // changing even in a static scene.
        // Smoke is the only moving medium. Its pattern at (p, t) equals the
        // pattern at (p - v*dt, t - dt), so reading history from there is the
        // fog equivalent of a TSR velocity buffer: the history lines up with the
        // smoke instead of lagging behind it (ghosting) or being rejected
        // (noise). Static media have smoke == 0 and read history in place.
        let history_pos = froxel_world_pos((vec2<f32>(gid.xy) + 0.5) / vec2<f32>(dims.xy),
            (f32(gid.z) + 0.5) / f32(dims.z))
            - smoke_velocity() * (fog_globals.delta_time * medium.smoke);
        let hist = sample_history(history_pos);
        if hist.w >= 0.0 {
            // Reject history at moving smoke edges and after density changes.
            // Stable interiors retain the low-noise lighting history.
            result = temporal_result(result, hist, media_list.temporal_blend, media_list.history_rejection);
        }
    }

    textureStore(scatter_out, coord, clamp(result, vec4<f32>(0), vec4<f32>(65504)));
}

// Stable (1-exp(-tau))/sigma_t, including the exact vacuum limit. The series
// avoids cancellation for thin media without suppressing their scattering.
fn segment_integral(sigma_t: f32, length: f32) -> f32 {
    let tau = sigma_t * length;
    if tau < 0.01 { return length * (1.0 - tau * 0.5 + tau * tau / 6.0 - tau * tau * tau / 24.0); }
    return (1.0 - exp(-tau)) / sigma_t;
}

// ── Integration ─────────────────────────────────────────────────────────────

/// 3x3 tent filter over the screen axes of the scattering grid, as four
/// bilinear taps at the corners of the centre texel (weights 1-2-1 x 1-2-1).
/// Any froxel whose history was rejected, or whose single shadow tap is still
/// converging, carries per-cell noise; this removes the residual cell-scale
/// static before it is integrated along depth. Depth is not filtered, so fog
/// never leaks across a surface.
fn scatter_denoised(uvw: vec3<f32>) -> vec4<f32> {
    let physical = textureDimensions(scatter_in);
    let o = 0.5 / vec2<f32>(media_list.grid.xy);
    var sum = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let d = vec2<f32>(select(-o.x, o.x, (i & 1u) != 0u), select(-o.y, o.y, (i & 2u) != 0u));
        sum += textureSampleLevel(scatter_in, linear_samp,
            scatter_coordinates(vec3<f32>(uvw.xy + d, uvw.z), physical), 0.0);
    }
    return sum * 0.25;
}

@compute @workgroup_size(8, 8, 1)
fn cs_integrate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(integrated_out);
    if gid.x >= dims.x || gid.y >= dims.y { return; }
    if media_list.has_medium == 0u {
        for (var z = 0u; z < dims.z; z++) {
            textureStore(integrated_out, vec3<i32>(vec2<i32>(gid.xy), i32(z)), vec4<f32>(0, 0, 0, 1));
        }
        return;
    }

    let uv = (vec2<f32>(gid.xy) + 0.5) / vec2<f32>(dims.xy);

    // Slice planes are constant view depth, so a ray at the screen edge travels
    // further between two slices than one down the centre. Without this the fog
    // thins toward the corners.
    let ndc = helio_uv_to_ndc(uv) + cameras[0].jitter_frame.xy;
    let dir = pixel_ray(ndc);
    let cos_a = max(dot(dir, normalize(cameras[0].forward_far.xyz)), 1e-4);

    var accum = vec3<f32>(0.0);
    var transmittance = 1.0;
    var prev_depth = HELIO_FROXEL_NEAR;

    for (var z = 0u; z < dims.z; z++) {
        let slice_norm = (f32(z) + 1.0) / f32(dims.z);
        let depth = helio_froxel_view_depth_from_slice(slice_norm, fog.fog_max_distance);
        let step_len = max(depth - prev_depth, 0.0) / cos_a;
        prev_depth = depth;

        let uvw = vec3<f32>(uv, (f32(z) + 0.5) / f32(dims.z));
        let s = scatter_denoised(uvw);
        let scattering = s.rgb;
        let sigma_t = max(s.a, 0.0);

        let step_transmittance = exp(-sigma_t * step_len);

        // Hillaire's analytic slice integral: the exact integral of scattering
        // over a segment of constant density, rather than a point sample scaled
        // by step length. Point-sampling over-brightens as density rises, because
        // it has no saturation term.
        let s_int = scattering * segment_integral(sigma_t, step_len);

        accum += transmittance * s_int;
        transmittance *= step_transmittance;

        textureStore(
            integrated_out,
            vec3<i32>(i32(gid.x), i32(gid.y), i32(z)),
            vec4<f32>(min(accum, vec3<f32>(65504)), transmittance),
        );
    }
}

// Logical face metadata maps all filtering into a guarded physical tile.
fn budget_resolution(layer:u32)->f32 {
    if layer>=arrayLength(&shadow_matrices) {return 128.0;}
    let m=shadow_matrices[layer];
    return select(max(f32(m.policy.z),128.0),1024.0,m.policy.w==0u);
}
fn budget_layer(layer:u32)->u32 {
    return select(shadow_matrices[layer].policy.x,layer,shadow_matrices[layer].policy.w==0u);
}
fn budget_uv(uv:vec2f,layer:u32,dims:vec2f)->vec2f {
    let m=shadow_matrices[layer];
    let offset=select(m.atlas.xy,vec2f(0),m.policy.w==0u);
    let size=select(m.atlas.z,1.0,m.policy.w==0u);
    let half_texel=0.5/dims;
    return clamp(offset+uv*size,offset+half_texel,offset+vec2f(size)-half_texel);
}
fn budget_valid(layer:u32,disabled:u32)->bool {
    if layer>=arrayLength(&shadow_matrices) {return false;}
    let m=shadow_matrices[layer];
    return m.policy.w==0u || (m.policy.w==2u && m.atlas.z>0.0 && (m.policy.y&disabled)==0u);
}
fn budget_strength(layer:u32)->f32 {
    let m=shadow_matrices[layer];return select(m.atlas.w,1.0,m.policy.w==0u);
}

fn budget_compare_dynamic(uv:vec2f,layer:u32,depth:f32)->f32 {
    if !budget_valid(layer,32u) {return 1.0;}
    if budget_layer(layer)>=textureNumLayers(shadow_atlas) {return 1.0;}
    let value=textureSampleCompareLevel(shadow_atlas,shadow_samp,budget_uv(uv,layer,vec2f(textureDimensions(shadow_atlas))),i32(budget_layer(layer)),depth);
    return mix(1.0,value,budget_strength(layer));
}
fn budget_depth_dynamic(pixel:vec2i,layer:u32)->f32 {
    if !budget_valid(layer,32u) {return 1.0;}
    if budget_layer(layer)>=textureNumLayers(shadow_atlas) {return 1.0;}
    let dims=vec2f(textureDimensions(shadow_atlas));
    let uv=(vec2f(pixel)+0.5)/budget_resolution(layer);
    return textureLoad(shadow_atlas,vec2i(budget_uv(uv,layer,dims)*dims),i32(budget_layer(layer)),0);
}

fn budget_compare_static(uv:vec2f,layer:u32,depth:f32)->f32 {
    if !budget_valid(layer,16u) {return 1.0;}
    if budget_layer(layer)>=textureNumLayers(static_shadow_atlas) {return 1.0;}
    let value=textureSampleCompareLevel(static_shadow_atlas,shadow_samp,budget_uv(uv,layer,vec2f(textureDimensions(static_shadow_atlas))),i32(budget_layer(layer)),depth);
    return mix(1.0,value,budget_strength(layer));
}
fn budget_depth_static(pixel:vec2i,layer:u32)->f32 {
    if !budget_valid(layer,16u) {return 1.0;}
    if budget_layer(layer)>=textureNumLayers(static_shadow_atlas) {return 1.0;}
    let dims=vec2f(textureDimensions(static_shadow_atlas));
    let uv=(vec2f(pixel)+0.5)/budget_resolution(layer);
    return textureLoad(static_shadow_atlas,vec2i(budget_uv(uv,layer,dims)*dims),i32(budget_layer(layer)),0);
}
