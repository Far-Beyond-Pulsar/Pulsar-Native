// The radiance cascades merge + raymarch — a fairly direct port of the
// `rcShader` fragment shader from
// https://raw.githubusercontent.com/radiance-cascades/radiance-cascades.com/refs/heads/main/public/js/rc.js
// to a WGSL compute shader (one thread per output texel instead of one
// fragment-shader invocation per pixel; `textureLod`/`texelFetch` become
// `textureSampleLevel`/`textureLoad`).
//
// Dispatched once per cascade level, from the coarsest (highest index) down
// to 0, ping-ponging between two `cascadeExtent`-sized textures: each level
// raymarches its own probes against the distance field, then *merges* in
// the next-coarser cascade's already-computed result (`last_tex`) wherever
// its own raymarch didn't hit anything — this is what makes farther
// cascades (fewer, sparser probes; each covering a longer, outward-shifted
// ray interval so consecutive cascades' intervals tile without gaps) supply
// distant light for near cascades without every level needing to trace all
// the way to the horizon itself.
//
// The merge is the "bilinear fix" (unlike the reference's single bilinear
// tap of the upper cascade): each ray is traced once per surrounding upper
// probe, from this probe's interval start to *that* probe's interval start,
// merged with that probe's radiance, and the four results are bilinearly
// weighted. A single interpolated tap mixes in upper probes on the far side
// of an occluder, which leaks light through walls and leaves light/dark
// bands near bright emitters (Helio#174).
//
// `cascadeExtent` is fixed at construction (`scene_size`), matching every
// level to the same texture resolution: a level's `spacing` (probe grid
// cell size in texels) grows with its index while the number of angular
// "ray buckets" tiled across that same texel budget grows to match, so the
// total resolution needed stays constant across levels — see `size`/
// `ray_pos`/`probe_relative_position` below.

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

fn fmod(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}
fn fmod2(v: vec2<f32>, y: f32) -> vec2<f32> {
    return vec2<f32>(fmod(v.x, y), fmod(v.y, y));
}
fn fmod2v(v: vec2<f32>, m: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(fmod(v.x, m.x), fmod(v.y, m.y));
}

struct CascadeUniforms {
    cascade_index: f32,
    cascade_count: f32,
    base_ray_count: f32,
    base_pixels_between_probes: f32,
    cascade_interval: f32,
    ray_interval: f32,
    _pad2: f32,
    is_top_cascade: f32,
    scene_size: vec2<f32>,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var<uniform> cu: CascadeUniforms;
@group(0) @binding(1) var scene_tex: texture_2d<f32>;
@group(0) @binding(2) var dist_tex: texture_2d<f32>;
@group(0) @binding(3) var last_tex: texture_2d<f32>;
@group(0) @binding(4) var rc_sampler: sampler;
@group(0) @binding(5) var out_tex: texture_storage_2d<rgba16float, write>;

// Sphere-traces `ray_start` -> `ray_end` (scene texels) against the
// distance field. Returns the hit occluder's emitted radiance with `a = 1`,
// or zero (`a = 0`) when the segment reaches its end unblocked.
fn raymarch(ray_start: vec2<f32>, ray_end: vec2<f32>, scale: f32, one_over_size: vec2<f32>, min_step: f32) -> vec4<f32> {
    let ray_length = length(ray_end - ray_start);
    if (ray_length <= 0.0) {
        return vec4<f32>(0.0);
    }
    let ray_dir = (ray_end - ray_start) / ray_length;
    var ray_uv = ray_start * one_over_size;
    var dist = 0.0;

    for (var i = 0; i < 256; i = i + 1) {
        if (dist >= ray_length) {
            break;
        }
        if (ray_uv.x < 0.0 || ray_uv.x > 1.0 || ray_uv.y < 0.0 || ray_uv.y > 1.0) {
            break;
        }
        let df = textureSampleLevel(dist_tex, rc_sampler, ray_uv, 0.0).r;
        if (df <= min_step) {
            return vec4<f32>(textureSampleLevel(scene_tex, rc_sampler, ray_uv, 0.0).rgb, 1.0);
        }
        dist += df * scale;
        ray_uv += ray_dir * (df * scale * one_over_size);
    }
    return vec4<f32>(0.0);
}

// Distance from a probe at which cascade `index`'s interval starts: 0 for
// cascade 0, then `unit * base^(index - 1)`. Interval `index` ends exactly
// where interval `index + 1` starts, so the levels tile each ray's length
// without gaps or overlap.
fn interval_start(index: f32, base: f32, unit: f32) -> f32 {
    if (index <= 0.0) {
        return 0.0;
    }
    return unit * pow(base, index - 1.0);
}

// The next-coarser cascade's merged radiance for ray bucket `bucket` (the
// `base` upper rays that subdivide this level's ray `bucket`, pre-averaged
// into one texel) at upper probe `probe`.
fn upper_radiance(bucket: f32, probe: vec2<f32>, upper_spacing: f32, upper_size: vec2<f32>) -> vec3<f32> {
    let tile = vec2<f32>(fmod(bucket, upper_spacing), floor(bucket / upper_spacing)) * upper_size;
    return textureLoad(last_tex, vec2<i32>(tile + probe), 0).rgb;
}

@compute @workgroup_size(8, 8)
fn cs_cascade(@builtin(global_invocation_id) gid: vec3<u32>) {
    let cascade_extent = cu.scene_size;
    if (f32(gid.x) >= cascade_extent.x || f32(gid.y) >= cascade_extent.y) {
        return;
    }
    let coord = vec2<f32>(f32(gid.x), f32(gid.y));

    let base = cu.base_ray_count;
    let cascade_index = cu.cascade_index;
    let ray_count = pow(base, cascade_index + 1.0);
    let spacing_base = sqrt(base);
    let spacing = pow(spacing_base, cascade_index);

    let size = floor(cascade_extent / spacing);
    let probe_relative_position = fmod2v(coord, size);
    let ray_pos = floor(coord / size);

    let unit = cu.base_pixels_between_probes * cu.ray_interval * cu.cascade_interval;
    let start = interval_start(cascade_index, base, unit);
    let end = interval_start(cascade_index + 1.0, base, unit);

    let probe_center = (probe_relative_position + 0.5) * cu.base_pixels_between_probes * spacing;
    let pre_avg_amt = base;
    let base_index = (ray_pos.x + spacing * ray_pos.y) * pre_avg_amt;
    let angle_step = TAU / ray_count;

    let scale = min(cascade_extent.x, cascade_extent.y);
    let one_over_size = 1.0 / cascade_extent;
    let min_step = min(one_over_size.x, one_over_size.y) * 0.5;
    let avg_recip = 1.0 / pre_avg_amt;
    let is_top = cu.is_top_cascade > 0.5;

    // Bilinear fix: the four upper probes around this one, with this probe's
    // bilinear weights. Each is merged along its own ray, from this probe's
    // interval start to that probe's interval start, so an occluder between
    // the two probes blocks that probe's light instead of letting it leak
    // through a single interpolated tap.
    let upper_spacing = spacing * spacing_base;
    let upper_size = floor(cascade_extent / upper_spacing);
    let upper_probe = probe_center / (cu.base_pixels_between_probes * upper_spacing) - 0.5;
    let upper_base = floor(upper_probe);
    let weights = upper_probe - upper_base;

    var total_radiance = vec3<f32>(0.0);
    let iters = i32(pre_avg_amt);
    for (var i = 0; i < iters; i = i + 1) {
        let index = base_index + f32(i);
        let angle = (index + 0.5) * angle_step;
        let ray_dir = vec2<f32>(cos(angle), -sin(angle));
        let ray_start = probe_center + ray_dir * start;
        var radiance = vec3<f32>(0.0);
        if (is_top) {
            radiance = raymarch(ray_start, probe_center + ray_dir * end, scale, one_over_size, min_step).rgb;
        } else {
            for (var k = 0u; k < 4u; k = k + 1u) {
                let corner = vec2<f32>(f32(k & 1u), f32(k >> 1u));
                let probe = clamp(upper_base + corner, vec2<f32>(0.0), upper_size - 1.0);
                let upper_center = (probe + 0.5) * cu.base_pixels_between_probes * upper_spacing;
                let hit = raymarch(ray_start, upper_center + ray_dir * end, scale, one_over_size, min_step);
                var merged = hit.rgb;
                if (hit.a <= 0.0) {
                    merged += upper_radiance(index, probe, upper_spacing, upper_size);
                }
                let weight = mix(1.0 - weights, weights, corner);
                radiance += merged * weight.x * weight.y;
            }
        }
        total_radiance += radiance * avg_recip;
    }

    textureStore(out_tex, vec2<i32>(gid.xy), vec4<f32>(total_radiance, 1.0));
}
