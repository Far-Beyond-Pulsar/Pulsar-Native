// Jump-flood seed propagation — ports rc.js's `JFA` class. Every occluder
// texel seeds itself (its own UV, stored in .xy); `cs_jfa_step` is run
// `passes = ceil(log2(max(w,h))) + 1` times with a halving offset each
// time, 9-tap (3x3 including center) nearest-seed propagation, ping-ponging
// between two textures — after all passes every texel holds the UV of its
// nearest occluder, which `rc_distance.wgsl` turns into a scalar distance
// field radiance cascades sphere-traces against.
//
// Seed sentinel: rc.js encodes "no seed yet" as `vec2(0,0)` (relying on UV
// (0,0) never legitimately being a seed in their canvas-drawing demo); this
// port uses `vec2(-1,-1)` instead since an occluder tile genuinely can sit
// at UV (0,0) here — checked via `sample.x >= 0.0` rather than `sample.x >
// 0.0 || sample.y > 0.0`.

struct DimsUniform {
    dims: vec2<f32>,
}

struct JfaUniform {
    offset: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> d: DimsUniform;
@group(0) @binding(1) var scene_tex: texture_2d<f32>;
@group(0) @binding(2) var seed_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn cs_jfa_seed(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (f32(gid.x) >= d.dims.x || f32(gid.y) >= d.dims.y) {
        return;
    }
    let a = textureLoad(scene_tex, vec2<i32>(gid.xy), 0).a;
    let uv = (vec2<f32>(gid.xy) + 0.5) / d.dims;
    var seed = vec2<f32>(-1.0, -1.0);
    if (a > 0.0) {
        seed = uv;
    }
    textureStore(seed_out, vec2<i32>(gid.xy), vec4<f32>(seed, 0.0, 0.0));
}

@group(0) @binding(0) var<uniform> jd: DimsUniform;
@group(0) @binding(1) var<uniform> ju: JfaUniform;
@group(0) @binding(2) var jfa_in: texture_2d<f32>;
@group(0) @binding(3) var jfa_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn cs_jfa_step(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (f32(gid.x) >= jd.dims.x || f32(gid.y) >= jd.dims.y) {
        return;
    }
    let uv = (vec2<f32>(gid.xy) + 0.5) / jd.dims;

    var best_seed = vec2<f32>(-1.0, -1.0);
    var best_dist = 1e9;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let suv = uv + vec2<f32>(f32(dx), f32(dy)) * ju.offset / jd.dims;
            if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0) {
                continue;
            }
            let scoord = vec2<i32>(suv * jd.dims);
            let sample = textureLoad(jfa_in, scoord, 0).xy;
            if (sample.x >= 0.0) {
                let diff = sample - uv;
                let dist = dot(diff, diff);
                if (dist < best_dist) {
                    best_dist = dist;
                    best_seed = sample;
                }
            }
        }
    }

    textureStore(jfa_out, vec2<i32>(gid.xy), vec4<f32>(best_seed, 0.0, 0.0));
}
