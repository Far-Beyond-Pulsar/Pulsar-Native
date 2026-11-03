// Post-process volume blending: the camera's baseline (its
// `camera_postprocess` row, else the defaults) blended with every active
// volume around it, written as the frame's post settings.

// Kept equal to `PostProcessVolumeComponent`'s SceneDB packed-layout
// auto-register capacity -- see `MAX_PP_VOLUMES`.
const MAX_PP_VOLUMES: u32 = 64u;

struct CameraUniforms {
    view:           mat4x4<f32>,
    proj:           mat4x4<f32>,
    view_proj:      mat4x4<f32>,
    inv_view_proj:  mat4x4<f32>,
    position_near:  vec4<f32>,
    forward_far:    vec4<f32>,
    jitter_frame:   vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

// ── GpuPostProcessVolume (matches CPU-side layout) ─────────────────────────────

struct GpuPostProcessVolume {
    bounds_min:     vec4<f32>,   // 0
    bounds_max:     vec4<f32>,   // 16
    priority:       f32,         // 32
    blend_radius:   f32,         // 36
    blend_weight:   f32,         // 40
    unbound:        u32,         // 44
    // vec4, not vec2: `settings` contains vec3s so it aligns to 16 and lands at
    // 64 regardless. Spelling the pad out to 64 keeps this struct honest about
    // where settings actually starts — the CPU side has to pad to match, and a
    // vec2 here silently hid an 8-byte hole that #[repr(C)] did not reproduce.
    override_mask:  vec4<u32>,   // 48..64
    settings:       GpuPostProcessUniforms,  // 64
}

struct CameraPostProcessComponent {
    view_id: u32,
    enabled: u32,
    _pad: vec2<u32>,
    settings: GpuPostProcessUniforms,
}
@group(0) @binding(20) var<storage, read> camera_postprocess: array<CameraPostProcessComponent>;

@group(0) @binding(0)  var<uniform>            postprocess:  GpuPostProcessUniforms;
@group(0) @binding(1)  var<storage, read> cameras: array<CameraUniforms, 2>;
@group(0) @binding(15) var<storage, read>      pp_volumes:   array<GpuPostProcessVolume>;
@group(0) @binding(16) var<storage, read_write> blend_output: GpuPostProcessUniforms;

// ── cs_volume_blend: GPU post-process volume blending ─────────────────────────
// Single workgroup (1 thread) that reads all active volumes and blends them
// with camera defaults, writing the result to blend_output.

// The four mask words use the indices exported by PostProcessProperty.
fn overridden(mask: vec4<u32>, property: u32) -> bool {
    return (mask[property / 32u] & (1u << (property % 32u))) != 0u;
}
fn blend_settings(base: GpuPostProcessUniforms, vol: GpuPostProcessUniforms, t: f32, mask: vec4<u32>) -> GpuPostProcessUniforms {
    var r = base;
    if overridden(mask, 0u) { r.exposure_mode = select(base.exposure_mode, vol.exposure_mode, t >= 0.5); }
    if overridden(mask, 1u) { r.exposure_compensation = base.exposure_compensation + (vol.exposure_compensation - base.exposure_compensation) * t; }
    if overridden(mask, 2u) { r.exposure_min = base.exposure_min + (vol.exposure_min - base.exposure_min) * t; }
    if overridden(mask, 3u) { r.exposure_max = base.exposure_max + (vol.exposure_max - base.exposure_max) * t; }
    if overridden(mask, 4u) { r.bloom_intensity = base.bloom_intensity + (vol.bloom_intensity - base.bloom_intensity) * t; }
    if overridden(mask, 5u) { r.bloom_threshold = base.bloom_threshold + (vol.bloom_threshold - base.bloom_threshold) * t; }
    if overridden(mask, 6u) { r.bloom_knee = base.bloom_knee + (vol.bloom_knee - base.bloom_knee) * t; }
    if overridden(mask, 7u) { r.bloom_radius = base.bloom_radius + (vol.bloom_radius - base.bloom_radius) * t; }
    if overridden(mask, 8u) { r.bloom_tint = base.bloom_tint + (vol.bloom_tint - base.bloom_tint) * t; }
    if overridden(mask, 9u) { r.bloom_enabled = select(base.bloom_enabled, vol.bloom_enabled, t >= 0.5); }
    if overridden(mask, 10u) { r.color_saturation = base.color_saturation + (vol.color_saturation - base.color_saturation) * t; }
    if overridden(mask, 11u) { r.exposure_speed_up = base.exposure_speed_up + (vol.exposure_speed_up - base.exposure_speed_up) * t; }
    if overridden(mask, 12u) { r.color_contrast = base.color_contrast + (vol.color_contrast - base.color_contrast) * t; }
    if overridden(mask, 13u) { r.exposure_speed_down = base.exposure_speed_down + (vol.exposure_speed_down - base.exposure_speed_down) * t; }
    if overridden(mask, 14u) { r.color_gamma = base.color_gamma + (vol.color_gamma - base.color_gamma) * t; }
    if overridden(mask, 15u) { r.color_gain = base.color_gain + (vol.color_gain - base.color_gain) * t; }
    if overridden(mask, 16u) { r.color_offset = base.color_offset + (vol.color_offset - base.color_offset) * t; }
    if overridden(mask, 17u) { r.white_temp = base.white_temp + (vol.white_temp - base.white_temp) * t; }
    if overridden(mask, 18u) { r.white_tint = base.white_tint + (vol.white_tint - base.white_tint) * t; }
    if overridden(mask, 19u) { r.white_balance_enabled = select(base.white_balance_enabled, vol.white_balance_enabled, t >= 0.5); }
    if overridden(mask, 20u) { r.tonemap_operator = select(base.tonemap_operator, vol.tonemap_operator, t >= 0.5); }
    if overridden(mask, 21u) { r.tonemap_exposure = base.tonemap_exposure + (vol.tonemap_exposure - base.tonemap_exposure) * t; }
    if overridden(mask, 22u) { r.tonemap_white_point = base.tonemap_white_point + (vol.tonemap_white_point - base.tonemap_white_point) * t; }
    if overridden(mask, 23u) { r.vignette_intensity = base.vignette_intensity + (vol.vignette_intensity - base.vignette_intensity) * t; }
    if overridden(mask, 24u) { r.vignette_smoothness = base.vignette_smoothness + (vol.vignette_smoothness - base.vignette_smoothness) * t; }
    if overridden(mask, 25u) { r.vignette_roundness = base.vignette_roundness + (vol.vignette_roundness - base.vignette_roundness) * t; }
    if overridden(mask, 26u) { r.vignette_color = base.vignette_color + (vol.vignette_color - base.vignette_color) * t; }
    if overridden(mask, 27u) { r.vignette_enabled = select(base.vignette_enabled, vol.vignette_enabled, t >= 0.5); }
    if overridden(mask, 28u) { r.grain_intensity = base.grain_intensity + (vol.grain_intensity - base.grain_intensity) * t; }
    if overridden(mask, 29u) { r.grain_response = base.grain_response + (vol.grain_response - base.grain_response) * t; }
    if overridden(mask, 30u) { r.grain_size = base.grain_size + (vol.grain_size - base.grain_size) * t; }
    if overridden(mask, 31u) { r.grain_enabled = select(base.grain_enabled, vol.grain_enabled, t >= 0.5); }
    if overridden(mask, 32u) { r.motion_blur_amount = base.motion_blur_amount + (vol.motion_blur_amount - base.motion_blur_amount) * t; }
    if overridden(mask, 33u) { r.motion_blur_max = base.motion_blur_max + (vol.motion_blur_max - base.motion_blur_max) * t; }
    if overridden(mask, 34u) { r.motion_blur_enabled = select(base.motion_blur_enabled, vol.motion_blur_enabled, t >= 0.5); }
    if overridden(mask, 35u) { r.blend_weight_bloom = base.blend_weight_bloom + (vol.blend_weight_bloom - base.blend_weight_bloom) * t; }
    if overridden(mask, 36u) { r.blend_weight_motion_blur = base.blend_weight_motion_blur + (vol.blend_weight_motion_blur - base.blend_weight_motion_blur) * t; }
    if overridden(mask, 37u) { r.blend_weight_vignette = base.blend_weight_vignette + (vol.blend_weight_vignette - base.blend_weight_vignette) * t; }
    if overridden(mask, 38u) { r.blend_weight_grain = base.blend_weight_grain + (vol.blend_weight_grain - base.blend_weight_grain) * t; }
    if overridden(mask, 39u) { r.blend_weight_exposure = base.blend_weight_exposure + (vol.blend_weight_exposure - base.blend_weight_exposure) * t; }
    if overridden(mask, 40u) { r.fog_enabled = select(base.fog_enabled, vol.fog_enabled, t >= 0.5); }
    if overridden(mask, 41u) { r.fog_mode = select(base.fog_mode, vol.fog_mode, t >= 0.5); }
    if overridden(mask, 42u) { r.fog_density = base.fog_density + (vol.fog_density - base.fog_density) * t; }
    if overridden(mask, 43u) { r.fog_height_falloff = base.fog_height_falloff + (vol.fog_height_falloff - base.fog_height_falloff) * t; }
    if overridden(mask, 44u) { r.fog_start_distance = base.fog_start_distance + (vol.fog_start_distance - base.fog_start_distance) * t; }
    if overridden(mask, 45u) { r.fog_max_distance = base.fog_max_distance + (vol.fog_max_distance - base.fog_max_distance) * t; }
    if overridden(mask, 46u) { r.fog_height = base.fog_height + (vol.fog_height - base.fog_height) * t; }
    if overridden(mask, 47u) { r.fog_scattering_anisotropy = base.fog_scattering_anisotropy + (vol.fog_scattering_anisotropy - base.fog_scattering_anisotropy) * t; }
    if overridden(mask, 48u) { r.fog_color = base.fog_color + (vol.fog_color - base.fog_color) * t; }
    if overridden(mask, 49u) { r.fog_emissive = base.fog_emissive + (vol.fog_emissive - base.fog_emissive) * t; }
    if overridden(mask, 50u) { r.hdr_output_mode = select(base.hdr_output_mode, vol.hdr_output_mode, t >= 0.5); }
    if overridden(mask, 51u) { r.hdr_max_nits = base.hdr_max_nits + (vol.hdr_max_nits - base.hdr_max_nits) * t; }
    if overridden(mask, 52u) { r.hdr_ui_brightness = base.hdr_ui_brightness + (vol.hdr_ui_brightness - base.hdr_ui_brightness) * t; }
    if overridden(mask, 53u) { r.lift_color = base.lift_color + (vol.lift_color - base.lift_color) * t; }
    if overridden(mask, 54u) { r.gamma_color = base.gamma_color + (vol.gamma_color - base.gamma_color) * t; }
    if overridden(mask, 55u) { r.gain_color = base.gain_color + (vol.gain_color - base.gain_color) * t; }
    if overridden(mask, 56u) { r.shadows_max = base.shadows_max + (vol.shadows_max - base.shadows_max) * t; }
    if overridden(mask, 57u) { r.highlights_min = base.highlights_min + (vol.highlights_min - base.highlights_min) * t; }
    if overridden(mask, 58u) { r.shadow_highlight_balance = base.shadow_highlight_balance + (vol.shadow_highlight_balance - base.shadow_highlight_balance) * t; }
    if overridden(mask, 59u) { r.hue_shift = base.hue_shift + (vol.hue_shift - base.hue_shift) * t; }
    if overridden(mask, 60u) { r.lut_generation = select(base.lut_generation, vol.lut_generation, t >= 0.5); }
    if overridden(mask, 61u) { r.lut_intensity = base.lut_intensity + (vol.lut_intensity - base.lut_intensity) * t; }
    if overridden(mask, 62u) { r.lut_platform = select(base.lut_platform, vol.lut_platform, t >= 0.5); }
    if overridden(mask, 63u) { r.lens_enabled = select(base.lens_enabled, vol.lens_enabled, t >= 0.5); }
    if overridden(mask, 64u) { r.lens_quality = select(base.lens_quality, vol.lens_quality, t >= 0.5); }
    if overridden(mask, 65u) { r.lens_profile = select(base.lens_profile, vol.lens_profile, t >= 0.5); }
    if overridden(mask, 66u) { r.lens_ghost_count = select(base.lens_ghost_count, vol.lens_ghost_count, t >= 0.5); }
    if overridden(mask, 67u) { r.lens_intensity = base.lens_intensity + (vol.lens_intensity - base.lens_intensity) * t; }
    if overridden(mask, 68u) { r.lens_threshold = base.lens_threshold + (vol.lens_threshold - base.lens_threshold) * t; }
    if overridden(mask, 69u) { r.lens_soft_knee = base.lens_soft_knee + (vol.lens_soft_knee - base.lens_soft_knee) * t; }
    if overridden(mask, 70u) { r.lens_ghost_intensity = base.lens_ghost_intensity + (vol.lens_ghost_intensity - base.lens_ghost_intensity) * t; }
    if overridden(mask, 71u) { r.lens_halo_intensity = base.lens_halo_intensity + (vol.lens_halo_intensity - base.lens_halo_intensity) * t; }
    if overridden(mask, 72u) { r.lens_glare_intensity = base.lens_glare_intensity + (vol.lens_glare_intensity - base.lens_glare_intensity) * t; }
    if overridden(mask, 73u) { r.lens_streak_intensity = base.lens_streak_intensity + (vol.lens_streak_intensity - base.lens_streak_intensity) * t; }
    if overridden(mask, 74u) { r.lens_dispersion = base.lens_dispersion + (vol.lens_dispersion - base.lens_dispersion) * t; }
    if overridden(mask, 75u) { r.lens_starburst_intensity = base.lens_starburst_intensity + (vol.lens_starburst_intensity - base.lens_starburst_intensity) * t; }
    if overridden(mask, 76u) { r.lens_starburst_length = base.lens_starburst_length + (vol.lens_starburst_length - base.lens_starburst_length) * t; }
    if overridden(mask, 77u) { r.lens_coating_strength = base.lens_coating_strength + (vol.lens_coating_strength - base.lens_coating_strength) * t; }
    if overridden(mask, 78u) { r.lens_ghost_rim = base.lens_ghost_rim + (vol.lens_ghost_rim - base.lens_ghost_rim) * t; }
    if overridden(mask, 79u) { r.lens_dirt_intensity = base.lens_dirt_intensity + (vol.lens_dirt_intensity - base.lens_dirt_intensity) * t; }
    if overridden(mask, 80u) { r.lens_light_sources = select(base.lens_light_sources, vol.lens_light_sources, t >= 0.5); }
    if overridden(mask, 81u) { r.lens_light_intensity = base.lens_light_intensity + (vol.lens_light_intensity - base.lens_light_intensity) * t; }
    if overridden(mask, 82u) { r.lens_field_margin = base.lens_field_margin + (vol.lens_field_margin - base.lens_field_margin) * t; }
    if overridden(mask, 83u) { r.lens_response_time = base.lens_response_time + (vol.lens_response_time - base.lens_response_time) * t; }
    if overridden(mask, 84u) { r.local_exposure_highlight_contrast = base.local_exposure_highlight_contrast + (vol.local_exposure_highlight_contrast - base.local_exposure_highlight_contrast) * t; }
    if overridden(mask, 85u) { r.local_exposure_shadow_contrast = base.local_exposure_shadow_contrast + (vol.local_exposure_shadow_contrast - base.local_exposure_shadow_contrast) * t; }
    if overridden(mask, 86u) { r.local_exposure_detail_strength = base.local_exposure_detail_strength + (vol.local_exposure_detail_strength - base.local_exposure_detail_strength) * t; }
    return r;
}
fn volume_weight(pos: vec3<f32>, v: GpuPostProcessVolume) -> f32 {
    let weight = clamp(v.blend_weight, 0.0, 1.0);
    if v.unbound != 0u { return weight; }
    if !all(pos >= v.bounds_min.xyz) || !all(pos <= v.bounds_max.xyz) { return 0.0; }
    let d = min(pos - v.bounds_min.xyz, v.bounds_max.xyz - pos);
    let boundary = min(d.x, min(d.y, d.z));
    if v.blend_radius > 0.0 { return weight * clamp(boundary / v.blend_radius, 0.0, 1.0); }
    return weight;
}

// SceneDB rows are indexed by entity, so these arrays span the whole entity
// range and are mostly empty. 256 threads compact the live rows reading only
// their small header fields; one thread then blends just the active volumes.
const RESOLVE_THREADS: u32 = 256u;
/// Active volumes blended per frame; further rows are ignored (lowest rows win).
const MAX_ACTIVE_VOLUMES: u32 = 256u;
var<workgroup> active_rows: array<u32, 256>;
var<workgroup> active_count: atomic<u32>;
var<workgroup> camera_row: atomic<u32>;

fn finite_f32(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

@compute @workgroup_size(256)
fn cs_volume_blend(@builtin(local_invocation_index) lid: u32) {
    if lid == 0u {
        atomicStore(&active_count, 0u);
        atomicStore(&camera_row, 0xffffffffu);
    }
    workgroupBarrier();
    let view_id = bitcast<u32>(cameras[0].jitter_frame.w);
    let camera_rows = arrayLength(&camera_postprocess);
    for (var i = lid; i < camera_rows; i += RESOLVE_THREADS) {
        if camera_postprocess[i].enabled != 0u && camera_postprocess[i].view_id == view_id {
            atomicMin(&camera_row, i);
        }
    }
    let volume_rows = arrayLength(&pp_volumes);
    for (var i = lid; i < volume_rows; i += RESOLVE_THREADS) {
        let w = pp_volumes[i].blend_weight;
        if w > 0.0 && finite_f32(w) && finite_f32(pp_volumes[i].priority) {
            let slot = atomicAdd(&active_count, 1u);
            if slot < MAX_ACTIVE_VOLUMES { active_rows[slot] = i; }
        }
    }
    workgroupBarrier();
    if lid != 0u { return; }

    var baseline = postprocess;
    let camera = atomicLoad(&camera_row);
    if camera != 0xffffffffu { baseline = camera_postprocess[camera].settings; }
    let count = min(atomicLoad(&active_count), MAX_ACTIVE_VOLUMES);
    // Stable ascending (priority, row): insertion sort of the compact list.
    for (var i = 1u; i < count; i++) {
        let row = active_rows[i];
        let priority = pp_volumes[row].priority;
        var j = i;
        loop {
            if j == 0u { break; }
            let other = active_rows[j - 1u];
            let other_priority = pp_volumes[other].priority;
            if other_priority < priority || (other_priority == priority && other < row) { break; }
            active_rows[j] = other;
            j--;
        }
        active_rows[j] = row;
    }

    var result = baseline;
    var medium = baseline;
    var bounded_range = 0.0;
    var bounded_fog = false;
    let position = cameras[0].position_near.xyz;
    for (var n = 0u; n < count; n++) {
        // Bounds in the frame's space, as the environment join writes them.
        let v = pp_volumes[active_rows[n]];
        let weight = volume_weight(position, v);
        if weight > 0.0 { result = blend_settings(result, v.settings, weight, v.override_mask); }
        if v.unbound != 0u {
            medium = blend_settings(medium, v.settings, clamp(v.blend_weight, 0.0, 1.0), v.override_mask);
        } else if overridden(v.override_mask, 53u) && v.settings.fog_enabled != 0u && v.settings.fog_density > 0.0 {
            // Bounded media are evaluated in world space by the fog pass, not
            // at the camera; they only extend the integration range here.
            bounded_fog = true;
            bounded_range = max(bounded_range, v.settings.fog_max_distance);
        }
    }
    result.fog_enabled = medium.fog_enabled;
    result.fog_mode = medium.fog_mode;
    result.fog_height_falloff = medium.fog_height_falloff;
    result.fog_start_distance = medium.fog_start_distance;
    result.fog_height = medium.fog_height;
    result.fog_scattering_anisotropy = medium.fog_scattering_anisotropy;
    result.fog_color = medium.fog_color;
    result.fog_emissive = medium.fog_emissive;
    result.fog_density = select(0.0, medium.fog_density, medium.fog_enabled != 0u);
    var range = select(0.0, medium.fog_max_distance, medium.fog_enabled != 0u);
    if bounded_fog {
        result.fog_enabled = 1u;
        range = max(range, bounded_range);
    }
    result.fog_max_distance = max(range, 1.0);
    blend_output = result;
}

