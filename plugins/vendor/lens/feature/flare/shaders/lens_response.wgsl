//!use helio_prelude

// ── Scene-linear lens response ───────────────────────────────────────────────
//
// Two inputs feed one optical model:
//
//   * Image-based: every bright pixel of the reconstructed HDR frame (emissive
//     surfaces, speculars, sky, fog) is scattered by the lens: glare, ghosts,
//     halo, streaks, starburst and dirt, sampled from a prefiltered pyramid.
//   * Analytic: scene lights are lens sources in their own right, so a light
//     just outside the frame still flares, and a light entering or leaving
//     the frame hands over continuously between the two paths. Visibility is
//     the light's own shadow map at the lens (the light sees the lens exactly
//     when the lens sees the light), and the depth buffer on screen.
//
// Both paths use the same pupil, rim and coating model, and the same energy
// calibration: the analytic source is expressed in the quarter-resolution
// texel units the image path extracts, so a light carries the same ghost
// energy whichever path renders it.

// The PP prefix stays opaque: 27 * 16 = 432 bytes. Binding starts at zero,
// avoiding min_uniform_buffer_offset_alignment restrictions on the lens tail.
struct Lens {
    enabled: u32,
    quality: u32,
    profile: u32,
    ghost_count: u32,
    intensity: f32,
    threshold: f32,
    soft_knee: f32,
    ghost_intensity: f32,
    halo_intensity: f32,
    glare_intensity: f32,
    streak_intensity: f32,
    dispersion: f32,
    reserved_vignette: f32,
    starburst_intensity: f32,
    starburst_length: f32,
    coating_strength: f32,
    ghost_rim: f32,
    dirt_intensity: f32,
    light_sources: u32,
    light_intensity: f32,
    field_margin: f32,
    response_time: f32,
    _pad0: f32,
    _pad1: f32,
}
struct PostProcess {
    prefix: array<vec4<u32>, 27>,
    lens: Lens,
}

// Indirect args per pyramid level: [3*k .. 3*k+3) for level k.
const LEVELS: u32 = 5u;
@group(0) @binding(0) var<uniform> pp: PostProcess;
// Extraction: the HDR input. Downsample: the previous pyramid level.
// Response: the whole pyramid (all mip levels).
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var destination: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var<storage, read_write> dispatch: array<u32, 15>;
// Temporal response only: last frame's output and the frame time.
struct Temporal { dt: f32, valid: f32, _pad: vec2<f32> }
@group(0) @binding(4) var history: texture_2d<f32>;
@group(0) @binding(5) var<uniform> temporal: Temporal;

// ── Optics inputs (sources and response) ─────────────────────────────────────

struct GpuLight {
    position_range:  vec4<f32>,
    direction_outer: vec4<f32>,
    color_intensity: vec4<f32>,
    shadow_index:    u32,
    light_type:      u32,
    inner_angle:     f32,
    _pad:            u32,
    god_rays:        vec4<f32>,
    god_rays_exposure: f32,
    flare:           array<u32, 7>,
    ies:             vec4<f32>,
}
struct LightMatrix { mat: mat4x4<f32>,
    atlas: vec4f,
    policy: vec4u, }

/// One analytic lens source, in the image path's units.
struct LensSource {
    /// Image-plane position (UV; may lie outside [0,1]).
    uv: vec2<f32>,
    /// Apparent radius in UV-x units (at least one quarter-res texel).
    radius: f32,
    /// Analytic share: off-frame handover x visibility x field falloff x gain.
    weight: f32,
    /// Quarter-res texel-sum energy: irradiance at the lens / texel solid angle.
    energy: vec3<f32>,
    _pad: f32,
}
const MAX_SOURCES: u32 = 32u;
struct LensSources {
    count: atomic<u32>,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    items: array<LensSource, 32>,
}

@group(1) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(1) @binding(1) var<storage, read> lights: array<GpuLight>;
@group(1) @binding(2) var scene_depth: texture_depth_2d;
@group(1) @binding(3) var<storage, read> shadow_matrices: array<LightMatrix>;
@group(1) @binding(4) var shadow_atlas: texture_depth_2d_array;
@group(1) @binding(5) var shadow_samp: sampler_comparison;
@group(1) @binding(6) var<storage, read_write> sources: LensSources;
@group(1) @binding(7) var dirt_tex: texture_2d<f32>;
@group(1) @binding(8) var dirt_samp: sampler;
// The camera's optics, resolved by the lens pass: the leading fields of
// `helio_pass_lens::ResolvedLens`. Optics are not part of the PP lens block.
struct LensOptics {
    f_number: f32,
    focal_length_mm: f32,
    sensor_width_mm: f32,
    aperture_blades: u32,
    aperture_rotation: f32,
    // Amount of natural cos^4 falloff the lens already applied to the image.
    natural_vignette: f32,
}
@group(1) @binding(9) var<storage, read> optics: LensOptics;

const PI: f32 = 3.141592653589793;
const HALF_MAX: f32 = 65504.0;
const LIGHT_DIRECTIONAL: u32 = 0u;
const LIGHT_POINT: u32 = 1u;
const LIGHT_SPOT: u32 = 2u;
const NO_SHADOW: u32 = 4294967295u;

// Synthetic reference lens at 50 mm / 36 mm / f2.8, calibrated to bounded
// energy fractions, not measured glass prescriptions. Each row stores signed
// image magnification, reflected energy, aperture radius, spectral sensitivity.
// Signed magnification produces reflected images across the optical axis.
const SPHERICAL = array<vec4<f32>, 8>(
    vec4(-0.42, 0.0090, 0.010, 1.00), vec4(-0.78, 0.0060, 0.014, 0.80),
    vec4( 0.63, 0.0040, 0.008, 0.60), vec4(-1.24, 0.0030, 0.019, 1.20),
    vec4( 1.46, 0.0020, 0.024, 0.70), vec4(-1.80, 0.0015, 0.028, 1.40),
    vec4( 0.28, 0.0010, 0.006, 0.50), vec4(-2.10, 0.0005, 0.032, 1.60),
);
const ANAMORPHIC = array<vec4<f32>, 8>(
    vec4(-0.35, 0.0090, 0.008, 1.20), vec4(-0.68, 0.0060, 0.012, 1.00),
    vec4( 0.54, 0.0040, 0.007, 0.80), vec4(-1.10, 0.0030, 0.016, 1.40),
    vec4( 1.32, 0.0020, 0.020, 0.90), vec4(-1.65, 0.0015, 0.024, 1.60),
    vec4( 0.25, 0.0010, 0.005, 0.60), vec4(-1.95, 0.0005, 0.028, 1.80),
);
// Residual reflectance colour of each ghost's surface pair. Multilayer AR
// coatings are tuned for green, so what they reflect is weak and tinted
// toward the ends of the spectrum: magenta, amber, violet, cyan-green.
// Luminance-normalised; `coating_strength` mixes from neutral.
const COATING = array<vec3<f32>, 8>(
    vec3(1.20, 0.80, 1.25), vec3(1.25, 0.95, 0.55), vec3(0.70, 1.10, 1.05),
    vec3(1.10, 0.85, 1.35), vec3(0.85, 1.12, 0.80), vec3(1.30, 0.90, 0.75),
    vec3(0.75, 1.05, 1.35), vec3(1.15, 1.00, 0.70),
);
// Relative wavelength (650/550/450 nm) for diffraction and dispersion.
const WAVELENGTH = vec3<f32>(1.18, 1.0, 0.82);

fn finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}
fn bounded(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if !finite(x) { return fallback; }
    return clamp(x, lo, hi);
}
fn radiance(c: vec3<f32>) -> vec3<f32> {
    return vec3(bounded(c.r, 0.0, HALF_MAX, 0.0),
                bounded(c.g, 0.0, HALF_MAX, 0.0),
                bounded(c.b, 0.0, HALF_MAX, 0.0));
}
fn luminance(c: vec3<f32>) -> f32 { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }
fn gain(x: f32) -> f32 { return bounded(x, 0.0, 4.0, 0.0); }
fn lens_active() -> bool {
    return pp.lens.enabled != 0u
        && bounded(pp.lens.intensity, 0.0, 16.0, 0.0) > 0.0
        && (gain(pp.lens.glare_intensity) > 0.0 || gain(pp.lens.halo_intensity) > 0.0
            || gain(pp.lens.streak_intensity) > 0.0 || gain(pp.lens.starburst_intensity) > 0.0
            || gain(pp.lens.dirt_intensity) > 0.0
            || (gain(pp.lens.ghost_intensity) > 0.0 && pp.lens.ghost_count > 0u));
}

// ── Iris ─────────────────────────────────────────────────────────────────────

fn blades() -> u32 {
    let n = optics.aperture_blades;
    return select(clamp(n, 3u, 16u), 0u, n == 0u);
}
/// Radial extent of the iris polygon along `angle` (circumradius 1).
fn iris_extent(angle: f32) -> f32 {
    let n = blades();
    if n == 0u { return 1.0; }
    let sector_angle = 2.0 * PI / f32(n);
    let a = angle - bounded(optics.aperture_rotation, -100.0, 100.0, 0.0);
    let sector = a - floor(a / sector_angle) * sector_angle - 0.5 * sector_angle;
    return cos(0.5 * sector_angle) / cos(sector);
}
/// Area of the iris polygon with circumradius 1.
fn iris_area() -> f32 {
    let n = blades();
    if n == 0u { return PI; }
    return 0.5 * f32(n) * sin(2.0 * PI / f32(n));
}
/// Deterministic area quadrature over the iris. Returns the offset and its
/// normalised radius (0 centre, 1 edge) for the rim profile.
fn pupil(i: u32, count: u32) -> vec3<f32> {
    if i == 0u { return vec3(0.0); }
    let r = sqrt((f32(i) - 0.5) / f32(count - 1u));
    let angle = f32(i) * 2.39996323;
    return vec3(vec2(cos(angle), sin(angle)) * r * iris_extent(angle), r);
}
/// Spherical aberration concentrates a defocused ghost's light toward its
/// edge. Mean over the disk is 1, so the rim redistributes, never adds.
fn rim_profile(r: f32) -> f32 {
    return max(1.0 + bounded(pp.lens.ghost_rim, 0.0, 1.0, 0.0) * (2.0 * r * r - 1.0), 0.0);
}
fn coating(g: u32) -> vec3<f32> {
    return mix(vec3(1.0), COATING[g], bounded(pp.lens.coating_strength, 0.0, 1.0, 0.0));
}

@compute @workgroup_size(1)
fn cs_control() {
    let size = textureDimensions(source);
    let on = select(0u, 1u, lens_active());
    for (var k = 0u; k < LEVELS; k++) {
        let level = max(size >> vec2(k), vec2(1u));
        dispatch[3u * k] = ((level.x + 7u) / 8u) * on;
        dispatch[3u * k + 1u] = (level.y + 7u) / 8u;
        dispatch[3u * k + 2u] = 1u;
    }
}

// 2x2 box average: preserves mean radiance, so a pupil average taken at a
// coarser level carries the same energy as one taken at the finest level.
@compute @workgroup_size(8, 8)
fn cs_downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(destination);
    if any(id.xy >= size) { return; }
    let input_size = textureDimensions(source);
    var color = vec3(0.0);
    for (var y = 0u; y < 2u; y++) {
        for (var x = 0u; x < 2u; x++) {
            let p = min(id.xy * 2u + vec2(x, y), input_size - 1u);
            color += textureLoad(source, vec2<i32>(p), 0).rgb;
        }
    }
    textureStore(destination, vec2<i32>(id.xy), vec4(color * 0.25, 0.0));
}

// Threshold is a scene-linear radiance selector, not tone mapping. Zero
// threshold/knee is a linear optical operator: every photon scatters, so
// sources fade in and out smoothly instead of switching at a cutoff.
fn extract(c: vec3<f32>) -> vec3<f32> {
    let y = luminance(c);
    let threshold = bounded(pp.lens.threshold, 0.0, HALF_MAX, 0.0);
    if threshold <= 0.0 { return c; }
    let knee = threshold * bounded(pp.lens.soft_knee, 0.0, 1.0, 0.0);
    var contribution = max(y - threshold, 0.0);
    if knee > 0.0 {
        let soft = clamp(y - threshold + knee, 0.0, 2.0 * knee);
        contribution = max(contribution, soft * soft / (4.0 * knee));
    }
    return c * (contribution / max(y, 1e-6));
}

// cos^4 natural falloff at the sensor for image position `uv`, as the lens
// applies it to the image (`natural_vignette` is the lens's amount; 0 without
// one). The extracted image already carries it, so only analytic sources,
// which are not in the image, take it here.
fn field_falloff(uv: vec2<f32>, aspect: f32) -> f32 {
    let focal = bounded(optics.focal_length_mm, 8.0, 600.0, 50.0);
    let sensor = bounded(optics.sensor_width_mm, 4.0, 70.0, 36.0);
    let field = (uv - 0.5) * vec2(sensor, sensor / aspect) / focal;
    let cos_squared = 1.0 / (1.0 + dot(field, field));
    return mix(1.0, cos_squared * cos_squared, bounded(optics.natural_vignette, 0.0, 1.0, 0.0));
}

@compute @workgroup_size(8, 8)
fn cs_extract(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(destination);
    if any(id.xy >= size) { return; }
    let input_size = textureDimensions(source);
    var color = vec3(0.0);
    var count = 0.0;
    // Every input texel participates, including tiny emissive/specular peaks.
    // Bounds checks exclude padding on odd image edges from normalization.
    for (var y = 0u; y < 4u; y++) {
        for (var x = 0u; x < 4u; x++) {
            let p = id.xy * 4u + vec2(x, y);
            if all(p < input_size) {
                color += extract(radiance(textureLoad(source, vec2<i32>(p), 0).rgb));
                count += 1.0;
            }
        }
    }
    textureStore(destination, vec2<i32>(id.xy), vec4(color / max(count, 1.0), 0.0));
}

// ── Analytic sources ─────────────────────────────────────────────────────────

fn point_light_face(dir: vec3<f32>) -> u32 {
    let a = abs(dir);
    if a.x >= a.y && a.x >= a.z { return select(0u, 1u, dir.x < 0.0); }
    if a.y >= a.x && a.y >= a.z { return select(2u, 3u, dir.y < 0.0); }
    return select(4u, 5u, dir.z < 0.0);
}

/// Does `light` reach the lens at `p`? Reciprocity: this is also whether the
/// lens sees the light, whether or not the light is inside the frame.
/// Lights without a shadow map give no occlusion information and count as
/// visible; the field margin still limits how far off-frame they reach.
fn shadow_visibility(light: GpuLight, p: vec3<f32>) -> f32 {
    if light.shadow_index == NO_SHADOW || textureDimensions(shadow_atlas).x <= 1u { return 1.0; }
    var layer = light.shadow_index;
    if light.light_type == LIGHT_POINT {
        layer = light.shadow_index + point_light_face(p - light.position_range.xyz);
    }
    // Directional: the lens sits at distance zero, which is always cascade 0.
    if layer >= arrayLength(&shadow_matrices) { return 1.0; }
    let proj = helio_shadow_project(shadow_matrices[layer].mat, p);
    if !proj.valid { return 1.0; }
    // Four taps around the lens position soften the handover at shadow edges.
    let texel = 1.0 / vec2<f32>(textureDimensions(shadow_atlas));
    var lit = 0.0;
    for (var k = 0u; k < 4u; k++) {
        let o = vec2(f32(k & 1u) - 0.5, f32(k >> 1u) - 0.5) * texel;
        lit += budget_compare_dynamic(proj.uv + o, u32(layer), proj.depth);
    }
    return lit * 0.25;
}

/// Fraction of the source disk not hidden by opaque geometry on screen.
fn depth_visibility(uv: vec2<f32>, radius: f32, light_depth: f32, aspect: f32) -> f32 {
    let dims = vec2<f32>(textureDimensions(scene_depth));
    var visible = 0.0;
    var taps = 0.0;
    for (var k = 0u; k < 9u; k++) {
        let p = pupil(k, 9u).xy * vec2(1.0, aspect) * radius;
        let tap = uv + p;
        if any(tap < vec2(0.0)) || any(tap >= vec2(1.0)) { continue; }
        let depth = textureLoad(scene_depth, vec2<i32>(tap * dims), 0);
        visible += select(0.0, 1.0, !helio_depth_is_nearer(depth, light_depth));
        taps += 1.0;
    }
    return select(1.0, visible / taps, taps > 0.0);
}

// One workgroup classifies every light. SceneDB rows are indexed by entity,
// so the scan strides over the whole buffer and reads headers first.
@compute @workgroup_size(64)
fn cs_sources(@builtin(local_invocation_index) lid: u32) {
    if lid == 0u { atomicStore(&sources.count, 0u); }
    storageBarrier();
    if !(lens_active() && pp.lens.light_sources != 0u) { return; }
    let camera = cameras[0];
    let eye = camera.position_near.xyz;
    let forward = normalize(camera.forward_far.xyz);
    let size = vec2<f32>(textureDimensions(source));
    let aspect = size.x / size.y;
    // Solid angle of one quarter-res texel near the axis: converts irradiance
    // at the lens into the texel-sum energy the image path would extract.
    let texel_angle = vec2(2.0 / (camera.proj[0][0] * size.x), 2.0 / (camera.proj[1][1] * size.y));
    let texel_solid_angle = texel_angle.x * texel_angle.y;
    let margin = bounded(pp.lens.field_margin, 0.0, 4.0, 0.35);
    let light_gain = bounded(pp.lens.light_intensity, 0.0, 16.0, 0.0);
    for (var i = lid; i < arrayLength(&lights); i += 64u) {
        let intensity = lights[i].color_intensity.w;
        if !(intensity > 0.0) || !finite(intensity) { continue; }
        let light = lights[i];
        var to_light: vec3<f32>;
        var irradiance = intensity;
        // A directional light is infinitely far: the depth clear value.
        var light_depth = 0.0;
        if light.light_type == LIGHT_DIRECTIONAL {
            if dot(light.direction_outer.xyz, light.direction_outer.xyz) < 1e-12 { continue; }
            to_light = normalize(-light.direction_outer.xyz);
        } else {
            let delta = light.position_range.xyz - eye;
            let dist = length(delta);
            let range = max(light.position_range.w, 1e-4);
            if dist > range || dist < 1e-3 { continue; }
            to_light = delta / dist;
            // Same windowed inverse square as surface and fog lighting.
            let window = clamp(1.0 - pow(dist / range, 4.0), 0.0, 1.0);
            irradiance *= window * window / (dist * dist);
            if light.light_type == LIGHT_SPOT {
                if dot(light.direction_outer.xyz, light.direction_outer.xyz) < 1e-12 { continue; }
                let cd = dot(-to_light, normalize(light.direction_outer.xyz));
                let spot = clamp((cd - light.direction_outer.w) / max(light.inner_angle - light.direction_outer.w, 1e-4), 0.0, 1.0);
                irradiance *= spot * spot;
            }
            let clip_depth = camera.view_proj * vec4(light.position_range.xyz, 1.0);
            light_depth = clip_depth.z / max(clip_depth.w, 1e-6);
        }
        // The front element only receives light from ahead of the camera.
        let cos_axis = dot(to_light, forward);
        if cos_axis <= 0.02 || !(irradiance > 0.0) { continue; }
        let clip = camera.view_proj * vec4(to_light, 0.0);
        let ndc = clip.xy / clip.w;
        let uv = helio_ndc_to_uv(ndc);
        // Signed distance outside the frame, in half-frame (NDC) units.
        let outside = max(abs(ndc) - vec2(1.0), vec2(0.0));
        let inside = min(1.0 - abs(ndc.x), 1.0 - abs(ndc.y));
        let edge_distance = select(-inside, length(outside), inside <= 0.0);
        // The barrel vignettes light arriving further off-axis than the frame.
        let field = 1.0 - smoothstep(0.0, max(margin, 1e-3), edge_distance);
        if field <= 0.0 { continue; }
        // Handover: on screen the image path renders the light's visible
        // emitter; the analytic share rises as its disk leaves the frame.
        // Analytic lights are point-like: one quarter-res texel of apparent
        // radius, and a fixed handover band either side of the frame edge.
        let band = 0.03;
        let off_frame = smoothstep(-band, band, edge_distance);
        if off_frame <= 0.0 { continue; }
        let on_screen_vis = depth_visibility(uv, 1.0 / size.x, light_depth, aspect);
        let lens_vis = shadow_visibility(light, eye);
        let visibility = mix(on_screen_vis, lens_vis, off_frame);
        let weight = off_frame * visibility * field * light_gain * field_falloff(uv, aspect);
        let energy = light.color_intensity.rgb * irradiance / texel_solid_angle;
        if weight * luminance(energy) <= 1e-6 { continue; }
        let slot = atomicAdd(&sources.count, 1u);
        if slot < MAX_SOURCES {
            sources.items[slot] = LensSource(uv, 1.0 / size.x, weight, energy, 0.0);
        }
    }
}

// ── Image sampling ───────────────────────────────────────────────────────────

// Explicit zero extension, including individual bilinear taps. Clamp-to-edge
// sampling would replicate bright border pixels into a false unlimited source.
fn load_zero(p: vec2<i32>, level: i32) -> vec3<f32> {
    if any(p < vec2(0)) || any(p >= vec2<i32>(textureDimensions(source, level))) {
        return vec3(0.0);
    }
    return textureLoad(source, p, level).rgb;
}
fn sample_level(uv: vec2<f32>, level: i32) -> vec3<f32> {
    let p = uv * vec2<f32>(textureDimensions(source, level)) - 0.5;
    let base = vec2<i32>(floor(p));
    let t = fract(p);
    return mix(mix(load_zero(base, level), load_zero(base + vec2(1, 0), level), t.x),
               mix(load_zero(base + vec2(0, 1), level), load_zero(base + vec2(1, 1), level), t.x), t.y);
}
// Trilinear over the extracted-light pyramid. Every level holds the same mean
// radiance, so the level only sets how much each tap is prefiltered.
fn sample_zero(uv: vec2<f32>, lod: f32) -> vec3<f32> {
    let l = clamp(lod, 0.0, f32(LEVELS - 1u));
    let l0 = i32(floor(l));
    let l1 = min(l0 + 1, i32(LEVELS) - 1);
    return mix(sample_level(uv, l0), sample_level(uv, l1), l - f32(l0));
}
fn spectral(uv: vec2<f32>, shift: vec2<f32>, lod: f32) -> vec3<f32> {
    if dot(shift, shift) < 1e-12 { return sample_zero(uv, lod); }
    return vec3(sample_zero(uv + shift, lod).r, sample_zero(uv, lod).g, sample_zero(uv - shift, lod).b);
}
// Prefilter level so neighbouring taps overlap: texel footprint ~ tap spacing.
// `spacing` is in UV-x units; level-0 texels are 1/width wide.
fn footprint_lod(spacing: f32, width: f32) -> f32 {
    return log2(max(spacing * width, 1.0));
}

/// Normalised iris membership of `d` (UV-x metric) for an iris of radius `r`,
/// with a one-texel soft edge; returns (inside, radial position 0..1).
fn iris_disk(d: vec2<f32>, r: f32, texel: f32) -> vec2<f32> {
    let dist = length(d);
    let extent = r * iris_extent(atan2(d.y, d.x));
    let inside = 1.0 - smoothstep(extent - texel, extent + texel, dist);
    return vec2(inside, clamp(dist / max(extent, 1e-6), 0.0, 1.0));
}

@compute @workgroup_size(8, 8)
fn cs_response(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(destination);
    if any(id.xy >= size) { return; }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let width = f32(size.x);
    let texel = 1.0 / width;
    let high = pp.lens.quality == 1u;
    let anamorphic = pp.lens.profile == 1u;
    let taps = select(8u, 24u, high);
    let aspect = f32(size.x) / f32(size.y);
    // Offsets are authored in UV-x units; scale y so shapes stay round.
    let metric = vec2(1.0, aspect);
    let squeeze = select(vec2(1.0), vec2(2.0, 0.5), anamorphic);
    let f_number = bounded(optics.f_number, 0.7, 32.0, 2.8);
    let focal = bounded(optics.focal_length_mm, 8.0, 600.0, 50.0);
    let sensor = bounded(optics.sensor_width_mm, 4.0, 70.0, 36.0);
    let field_scale = clamp((sensor / 36.0) * (50.0 / focal), 0.35, 2.0);
    let dispersion = bounded(pp.lens.dispersion, 0.0, 0.05, 0.0);
    // Airy first-zero scale at 550nm plus a compact surface-scatter surrogate.
    // Kernel weights are normalized: aperture changes shape, not exposure.
    let diffraction = 1.22 * 0.00055 * f_number / sensor;
    let glare_radius = min(0.08, 0.02 * sqrt(f_number / 2.8) + diffraction);
    let source_count = min(atomicLoad(&sources.count), MAX_SOURCES);
    var response = vec3(0.0);

    if gain(pp.lens.glare_intensity) > 0.0 {
        var glare = vec3(0.0);
        var weight_sum = 0.0;
        let lod = footprint_lod(2.0 * glare_radius / sqrt(f32(taps)), width);
        for (var i = 0u; i < taps; i++) {
            let p = pupil(i, taps);
            // Smooth falloff: glare is veiling scatter, brightest at the source.
            let weight = pow(max(1.0 - p.z * p.z, 0.0), 2.0);
            glare += sample_zero(uv + p.xy * metric * squeeze * glare_radius, lod) * weight;
            weight_sum += weight;
        }
        response += glare * (0.06 * gain(pp.lens.glare_intensity) / weight_sum);
    }

    if gain(pp.lens.ghost_intensity) > 0.0 {
        let ghosts = min(pp.lens.ghost_count, select(4u, 8u, high));
        let pupil_taps = select(7u, 19u, high);
        let rim_fringe = min(0.3, dispersion * 25.0);
        for (var g = 0u; g < ghosts; g++) {
            let profile = select(SPHERICAL[g], ANAMORPHIC[g], anamorphic);
            let m = profile.x;
            // Ghosts are strongly defocused images of the iris, so their blur
            // is the pupil scaled well past the source size: a small bright
            // source reads as a soft iris-shaped disk, not its own shape.
            let radius = profile.z * 4.0 * clamp(2.8 / f_number, 0.25, 2.0) * field_scale;
            let reflect = profile.y * gain(pp.lens.ghost_intensity) * coating(g);

            // Image path: convolve the extracted light with the iris.
            let center = vec2(0.5) + (uv - 0.5) / m;
            let lod = footprint_lod(2.0 * radius / sqrt(f32(pupil_taps)), width);
            let shift = (center - 0.5) * dispersion * profile.w;
            var ghost = vec3(0.0);
            var weight_sum = 0.0;
            for (var i = 0u; i < pupil_taps; i++) {
                let p = pupil(i, pupil_taps);
                let w = rim_profile(p.z);
                ghost += spectral(center + p.xy * radius * metric * squeeze, shift, lod) * w;
                weight_sum += w;
            }
            // Inverse area Jacobian maintains the calibrated reflected energy
            // when ghost magnification changes. Off-image energy is discarded.
            response += ghost * reflect / (weight_sum * m * m);

            // Analytic path: the same iris disk, placed where this ghost images
            // each light, with per-wavelength radius for chromatic edges.
            for (var s = 0u; s < source_count; s++) {
                let src = sources.items[s];
                let ghost_uv = vec2(0.5) + m * (src.uv - vec2(0.5));
                let d = (uv - ghost_uv) / metric / squeeze;
                let r = abs(m) * (radius + src.radius);
                // Output-texel area of the ghost disk: its energy is spread
                // over exactly the area the image path would cover.
                let area = iris_area() * (r * width) * (r * width) * squeeze.x * squeeze.y;
                // Spell out the channels: FXC cannot safely lower dynamic
                // writes to a vector component inside this three-iteration
                // loop and may reject the entire compute pipeline.
                let red_radius = r * (1.0 + rim_fringe * profile.w * (1.0 - WAVELENGTH.r) * 2.0);
                let green_radius = r * (1.0 + rim_fringe * profile.w * (1.0 - WAVELENGTH.g) * 2.0);
                let blue_radius = r * (1.0 + rim_fringe * profile.w * (1.0 - WAVELENGTH.b) * 2.0);
                let red_disk = iris_disk(d, red_radius, texel);
                let green_disk = iris_disk(d, green_radius, texel);
                let blue_disk = iris_disk(d, blue_radius, texel);
                let c = vec3(
                    red_disk.x * rim_profile(red_disk.y),
                    green_disk.x * rim_profile(green_disk.y),
                    blue_disk.x * rim_profile(blue_disk.y),
                );
                response += c * src.energy * src.weight * reflect / max(area, 1e-6);
            }
        }
    }

    if gain(pp.lens.halo_intensity) > 0.0 {
        // Radial halo: each output pixel gathers from a fixed distance along
        // its direction to the optical axis, so a source maps to a continuous
        // arc of a circle about the axis (no per-tap copies of the source).
        let axis = (vec2(0.5) - uv) * vec2(aspect, 1.0);
        let axis_length = length(axis);
        if axis_length > 1e-4 {
            let direction = axis / axis_length / vec2(aspect, 1.0);
            let radius = 0.32 * field_scale;
            let bands = select(4u, 8u, high);
            let lod = footprint_lod(0.16 * radius / f32(bands), width) + 1.0;
            var halo = vec3(0.0);
            var weight_sum = 0.0;
            for (var i = 0u; i < bands; i++) {
                let t = (f32(i) + 0.5) / f32(bands) * 2.0 - 1.0;
                let weight = 1.0 - t * t;
                let offset = direction * radius * (1.0 + 0.08 * t);
                halo += spectral(uv + offset, offset * dispersion, lod) * weight;
                weight_sum += weight;
            }
            // Fade toward the image corners, where the reflecting surfaces
            // vignette the halo path.
            let fade = clamp(1.0 - axis_length / 0.9, 0.0, 1.0);
            response += halo * (0.02 * gain(pp.lens.halo_intensity) * fade * fade / weight_sum);
        }
    }

    if gain(pp.lens.streak_intensity) > 0.0 {
        var streak = vec3(0.0);
        var weight_sum = 0.0;
        let radius = select(0.06, 0.18, anamorphic) * field_scale;
        // Streaks stay thin vertically: prefilter only as far as tap spacing.
        let lod = footprint_lod(2.0 * radius / f32(taps), width);
        for (var i = 0u; i <= taps; i++) {
            let t = 2.0 * f32(i) / f32(taps) - 1.0;
            let weight = pow(max(1.0 - abs(t), 0.0), 2.0);
            streak += spectral(uv + vec2(t * radius, 0.0),
                vec2(t * radius * dispersion, 0.0), lod) * weight;
            weight_sum += weight;
        }
        response += streak * (0.03 * gain(pp.lens.streak_intensity) / weight_sum);
    }

    let n = blades();
    if gain(pp.lens.starburst_intensity) > 0.0 && n > 0u {
        // Fraunhofer diffraction by straight iris blades: each blade edge
        // throws a spike perpendicular to it. Opposite edges of an even iris
        // share a line, so n blades give n spikes (n even) or 2n (n odd).
        // Spike length scales with wavelength, which spreads colour outward.
        let lines = select(n, n / 2u, n % 2u == 0u);
        let length_uv = bounded(pp.lens.starburst_length, 0.0, 1.0, 0.25) * field_scale;
        let spike_taps = select(10u, 20u, high);
        let lod = footprint_lod(length_uv / f32(spike_taps), width);
        let rotation = bounded(optics.aperture_rotation, -100.0, 100.0, 0.0);
        var burst = vec3(0.0);
        var weight_sum = 0.0;
        for (var l = 0u; l < lines; l++) {
            // Perpendicular to blade edge l.
            let angle = rotation + (f32(l) + 0.5) * PI / f32(lines) * select(1.0, 2.0, n % 2u == 1u);
            let dir = vec2(cos(angle), sin(angle)) * metric;
            for (var i = 1u; i <= spike_taps; i++) {
                let t = f32(i) / f32(spike_taps);
                // Diffraction envelope falls steeply away from the source.
                let w = pow(1.0 - t, 3.0) / (0.05 + t);
                let offset = dir * t * length_uv;
                for (var side = 0u; side < 2u; side++) {
                    let o = select(offset, -offset, side == 1u);
                    burst += vec3(sample_zero(uv + o * WAVELENGTH.r, lod).r,
                                  sample_zero(uv + o, lod).g,
                                  sample_zero(uv + o * WAVELENGTH.b, lod).b) * w;
                    weight_sum += w;
                }
            }
        }
        response += burst * (0.12 * gain(pp.lens.starburst_intensity) / max(weight_sum, 1e-6));

        // Off-frame lights still throw spikes into the frame.
        for (var s = 0u; s < source_count; s++) {
            let src = sources.items[s];
            let d = (uv - src.uv) / metric;
            let dist = length(d);
            if dist < 1e-5 { continue; }
            var spikes = vec3(0.0);
            for (var l = 0u; l < lines; l++) {
                let angle = rotation + (f32(l) + 0.5) * PI / f32(lines) * select(1.0, 2.0, n % 2u == 1u);
                let dir = vec2(cos(angle), sin(angle));
                let along = abs(dot(d, dir));
                let across = abs(dot(d, vec2(-dir.y, dir.x)));
                let line = exp(-across * across / (2.0 * texel * texel));
                let red_t = along / max(length_uv * WAVELENGTH.r, 1e-5);
                let green_t = along / max(length_uv * WAVELENGTH.g, 1e-5);
                let blue_t = along / max(length_uv * WAVELENGTH.b, 1e-5);
                let red_envelope = select(0.0, pow(1.0 - red_t, 3.0) / (0.05 + red_t), red_t < 1.0);
                let green_envelope = select(0.0, pow(1.0 - green_t, 3.0) / (0.05 + green_t), green_t < 1.0);
                let blue_envelope = select(0.0, pow(1.0 - blue_t, 3.0) / (0.05 + blue_t), blue_t < 1.0);
                spikes += line * vec3(red_envelope, green_envelope, blue_envelope);
            }
            // Same normalisation as the image path's tap weights, per texel.
            let norm = 0.12 * gain(pp.lens.starburst_intensity) / (f32(lines) * 2.0 * length_uv * width * 3.0);
            response += spikes * src.energy * src.weight * norm;
        }
    }

    if gain(pp.lens.dirt_intensity) > 0.0 {
        // Dirt on the front element scatters whatever light reaches it: the
        // broad low-frequency light across the frame plus off-frame sources.
        var veil = sample_zero(uv, f32(LEVELS - 1u));
        for (var s = 0u; s < source_count; s++) {
            let src = sources.items[s];
            let d = (uv - src.uv) / metric;
            veil += src.energy * src.weight / (1.0 + dot(d, d) * 40.0) / (width * width * 0.25);
        }
        let dirt = textureSampleLevel(dirt_tex, dirt_samp, uv, 0.0).rgb;
        response += veil * dirt * gain(pp.lens.dirt_intensity);
    }

    textureStore(destination, vec2<i32>(id.xy),
        vec4(radiance(response * bounded(pp.lens.intensity, 0.0, 16.0, 0.0)), 0.0));
}

// ── Temporal source response ─────────────────────────────────────────────────
//
// Fades the *light entering the lens*, not the lens image. The extracted
// bright image is blended with last frame's, reprojected through scene depth
// and the previous view-projection (as TAA does), so a static lamp stays
// registered while the camera moves: no trails behind ghosts. When a source is
// occluded, enters the frame or the lens is enabled, its brightness follows
// 1 - exp(-t / response_time) instead of popping. History starts black.
fn history_bilinear(uv: vec2<f32>) -> vec3<f32> {
    let dims = vec2<i32>(textureDimensions(history));
    let p = uv * vec2<f32>(dims) - 0.5;
    let base = vec2<i32>(floor(p));
    let t = fract(p);
    var c = array<vec3<f32>, 4>();
    for (var k = 0; k < 4; k++) {
        let q = base + vec2(k & 1, k >> 1);
        c[k] = select(vec3(0.0), textureLoad(history, clamp(q, vec2(0), dims - 1), 0).rgb,
            all(q >= vec2(0)) && all(q < dims));
    }
    return mix(mix(c[0], c[1], t.x), mix(c[2], c[3], t.x), t.y);
}

@compute @workgroup_size(8, 8)
fn cs_temporal(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(destination);
    if any(id.xy >= size) { return; }
    let current = textureLoad(source, vec2<i32>(id.xy), 0).rgb;
    let tau = bounded(pp.lens.response_time, 0.0, 2.0, 0.0);
    let dt = bounded(temporal.dt, 0.0, 1.0, 0.0);
    var blend = select(1.0 - exp(-dt / max(tau, 1e-4)), 1.0, tau <= 1e-4 || temporal.valid < 0.5);
    var previous = vec3(0.0);
    if blend < 1.0 {
        let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
        let depth_dims = vec2<f32>(textureDimensions(scene_depth));
        let depth = textureLoad(scene_depth, vec2<i32>(min(uv * depth_dims, depth_dims - 1.0)), 0);
        let camera = cameras[0];
        // Sky texels reproject as directions.
        let world = camera.view_proj_inv * vec4(helio_uv_to_ndc(uv), depth, 1.0);
        var point = vec4(world.xyz / world.w, 1.0);
        // Homogeneous difference: exact at sky's w == 0 (depth 0 is
        // infinitely far), where dividing by w gives NaN.
        if helio_is_sky(depth) { point = vec4(normalize(world.xyz - world.w * camera.position_near.xyz), 0.0); }
        let prev = camera.prev_view_proj * point;
        let prev_uv = helio_ndc_to_uv(prev.xy / prev.w);
        if prev.w > 1e-6 && all(prev_uv >= vec2(0.0)) && all(prev_uv <= vec2(1.0)) {
            previous = history_bilinear(prev_uv);
        }
        // Light arriving from outside last frame's view has black history, so a
        // source entering the frame fades in as well.
    }
    textureStore(destination, vec2<i32>(id.xy), vec4(radiance(mix(previous, current, blend)), 0.0));
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
