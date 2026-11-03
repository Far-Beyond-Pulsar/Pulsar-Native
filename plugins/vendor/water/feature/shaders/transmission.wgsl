//!use helio_prelude
//!use view_remap

// The light water focuses onto what lies under it (caustics), at each pixel:
// the frame's `transmission` lighting input. A fullscreen draw rather than a
// compute pass, so the caustics' footprint (screen-space derivatives of the
// world position) is the one the lit pixels see.

// Matches `GpuWaterVolume` (gpu_types.rs).
struct GpuWaterVolume {
    bounds_min: vec4<f32>,
    bounds_max: vec4<f32>,
    wave_params: vec4<f32>,
    wave_direction: vec4<f32>,
    water_color: vec4<f32>,
    extinction: vec4<f32>,
    reflection_refraction: vec4<f32>,
    caustics_params: vec4<f32>,  // x=enabled, y=intensity, z=scale, w=speed
    fog_params: vec4<f32>,
    _pad0: vec4<f32>,
    _pad1: vec4<f32>,
    _pad2: vec4<f32>,
    _pad3: vec4<f32>,
    _pad4: vec4<f32>,
    _pad5: vec4<f32>,
    world_phase: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<storage, read> water_volumes: array<GpuWaterVolume>;
// The frame's world origin XZ reduced to the caustics' 12 m tile.
@group(0) @binding(2) var<uniform> water_caustics_phase: vec4<f32>;
// `helio_frame_api::lighting::VIEW_REMAPS`: a remapped pixel's caustics are
// where its content really is.
@group(0) @binding(3) var<uniform> view_remaps: array<mat4x4<f32>, 256>;
@group(1) @binding(0) var gbuf_depth: texture_depth_2d;
@group(1) @binding(1) var gbuf_lightmap_uv: texture_2d<f32>;
@group(1) @binding(2) var water_caustics: texture_2d<f32>;
@group(1) @binding(3) var water_caustics_deep: texture_2d<f32>;
@group(1) @binding(4) var caustics_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) clip_pos: vec4<f32>) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(i32(clip_pos.x), i32(clip_pos.y));
    let depth = textureLoad(gbuf_depth, pix, 0);
    let screen_size = vec2<f32>(textureDimensions(gbuf_depth));
    let screen_uv = clip_pos.xy / screen_size;
    let ndc_xy = vec2<f32>(screen_uv.x * 2.0 - 1.0, 1.0 - screen_uv.y * 2.0);
    let world_h = cameras[0].view_proj_inv * vec4<f32>(ndc_xy, depth, 1.0);
    var world_pos = world_h.xyz / world_h.w;
    let chain = view_remap_of(textureLoad(gbuf_lightmap_uv, pix, 0).rg);
    if chain >= 0 {
        world_pos = (view_remaps[chain] * vec4<f32>(world_pos, 1.0)).xyz;
    }
    // Every pixel of a quad evaluates (derivatives); the sky adds nothing.
    let light = water_caustics_light(world_pos);
    if helio_is_sky(depth) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(light, 0.0);
}

/// The periodic three-depth caustics field the water surface uses. The phase
/// uniform is reduced from the f64 world origin to this 12 m tile,
/// independently of the 270 m volume-relative simulation.
fn water_caustics_light(world_pos: vec3<f32>) -> vec3<f32> {
    if arrayLength(&water_volumes) == 0u {
        return vec3<f32>(0.0);
    }
    let vol = water_volumes[0];
    let depth = vol.bounds_max.w - world_pos.y;
    if depth <= 0.0 || vol.caustics_params.x <= 0.5 {
        return vec3<f32>(0.0);
    }
    let uv = fract((world_pos.xz + water_caustics_phase.xy) / 12.0);
    let footprint = max(length(dpdx(world_pos.xz)), length(dpdy(world_pos.xz)));
    let near_lod = clamp(log2(max(footprint * 512.0 / 12.0, 1.0)), 0.0, 9.0);
    let deep_lod = clamp(log2(max(footprint * 256.0 / 12.0, 1.0)), 0.0, 8.0);
    var caustic: f32;
    if depth <= 4.0 {
        let shallow = textureSampleLevel(water_caustics, caustics_sampler,
            uv, near_lod).r;
        let mid = textureSampleLevel(water_caustics_deep, caustics_sampler,
            uv, deep_lod).r;
        caustic = mix(shallow, mid, smoothstep(1.5, 4.0, clamp(depth, 1.5, 4.0)));
    } else {
        let mid = textureSampleLevel(water_caustics_deep, caustics_sampler,
            uv, deep_lod).r;
        let deep = textureSampleLevel(water_caustics_deep, caustics_sampler,
            uv, deep_lod).g;
        caustic = mix(mid, deep, smoothstep(4.0, 10.0, clamp(depth, 4.0, 10.0)));
    }
    let attenuation = exp(-max(depth - 1.5, 0.0) * 0.12);
    return vec3<f32>(0.7, 0.9, 1.0) * max(caustic - 1.0, 0.0)
         * vol.caustics_params.y * attenuation;
}
