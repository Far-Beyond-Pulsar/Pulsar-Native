//!use helio_prelude
// Wave-driven caustics by refracted triangle area-ratio splatting, following
// Ultra-Water's three-depth raster pipeline. The source grid samples the 12 m
// FFT cascade with cubic B-spline filtering; 4x MSAA resolves thin folds by
// coverage. Near energy is written to R; mid/deep energy shares an independent
// 256² texture in R/G. Each tile has its own repeat/mip chain.

struct WaterVolume {
    bounds_min:            vec4f,
    bounds_max:            vec4f,
    wave_params:           vec4f,
    wave_direction:        vec4f,
    water_color:           vec4f,
    extinction:            vec4f,
    reflection_refraction: vec4f,
    caustics_params:       vec4f,
    fog_params:            vec4f,
    sim_params:            vec4f,
    shadow_params:         vec4f,
    sun_direction:         vec4f,
    ssr_params:            vec4f,
    sim_dynamics:          vec4f,
    wind_params:           vec4f,
    world_phase:           vec4f,
}

struct SliceParameters {
    // x depth (m), y source cells, z slope mip, w source-space landing margin.
    source: vec4f,
    // x = native target resolution; y = deep output channel (0 -> R, 1 -> G).
    target_info: vec4f,
}

@group(0) @binding(0) var<storage, read> water_volumes: array<WaterVolume>;
@group(0) @binding(1) var ocean_slopes: texture_2d_array<f32>;
@group(0) @binding(2) var water_samp: sampler;
@group(0) @binding(3) var<uniform> slice: SliceParameters;

const FFT_PATCH: f32 = 12.0;
const MAX_AREA_RATIO: f32 = 16.0;
const SLOPE_CASCADE: i32 = 2;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) old_position: vec2f,
}

fn slope_bspline(uv: vec2f, lod: f32) -> vec2f {
    let size = vec2f(textureDimensions(ocean_slopes, i32(lod)));
    let st = uv * size - vec2f(0.5);
    let i = floor(st);
    let f = st - i;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (vec2f(1.0) - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = (vec2f(4.0) - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (vec2f(1.0) + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    let h0 = (i - vec2f(0.5) + w1 / g0) / size;
    let h1 = (i + vec2f(1.5) + w3 / g1) / size;
    let s00 = textureSampleLevel(ocean_slopes, water_samp, h0, SLOPE_CASCADE, lod).xy;
    let s10 = textureSampleLevel(ocean_slopes, water_samp, vec2f(h1.x, h0.y), SLOPE_CASCADE, lod).xy;
    let s01 = textureSampleLevel(ocean_slopes, water_samp, vec2f(h0.x, h1.y), SLOPE_CASCADE, lod).xy;
    let s11 = textureSampleLevel(ocean_slopes, water_samp, h1, SLOPE_CASCADE, lod).xy;
    return g0.y * (g0.x * s00 + g1.x * s10)
         + g1.y * (g0.x * s01 + g1.x * s11);
}

@vertex
fn vs_main(@location(0) position: vec4f) -> VertexOutput {
    let cells = slice.source.y;
    let t = (position.xy + vec2f(0.5 * cells + 0.5)) / cells;
    var out: VertexOutput;
    out.old_position = t * slice.target_info.x;
    let slope = slope_bspline(t, slice.source.z);
    let volume = water_volumes[0];
    let sun = normalize(volume.sun_direction.xyz);
    if sun.y <= 0.0 {
        // A flat unit field is the neutral direct-light multiplier at night.
        out.position = vec4f(t * 2.0 - 1.0, 0.0, 1.0);
        return out;
    }
    let eta = 1.0 / max(volume.sim_params.x, 1.0);
    let surface_normal = normalize(vec3f(-slope.x, 1.0, -slope.y));
    let refracted = refract(-sun, surface_normal, eta);
    let flat_refracted = refract(-sun, vec3f(0.0, 1.0, 0.0), eta);
    let safe_y = max(-refracted.y, 0.05);
    let flat_y = max(-flat_refracted.y, 0.05);
    var landing_delta = (
        refracted.xz / safe_y - flat_refracted.xz / flat_y
    ) * (slice.source.x / FFT_PATCH);
    landing_delta = clamp(landing_delta, vec2f(-slice.source.w), vec2f(slice.source.w));

    out.position = vec4f((t + landing_delta) * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Derivatives convert the source area, in target texels, to its landed
    // area, in raster pixels. MSAA averages partially covered fold slivers.
    let dx = dpdx(in.old_position);
    let dy = dpdy(in.old_position);
    let area_ratio = min(abs(dx.x * dy.y - dx.y * dy.x), MAX_AREA_RATIO);
    if slice.target_info.y < 0.5 {
        return vec4f(area_ratio, 0.0, 0.0, 1.0);
    }
    return vec4f(0.0, area_ratio, 0.0, 1.0);
}
