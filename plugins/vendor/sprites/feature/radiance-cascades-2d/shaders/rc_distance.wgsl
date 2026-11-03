// Converts the final jump-flood seed texture into a scalar UV-space
// distance-to-nearest-occluder field (stored in `.r`), read by the cascade
// pass's `raymarch()` for sphere-tracing (skip empty space in one hop
// instead of fixed small steps).

struct DimsUniform {
    dims: vec2<f32>,
}

@group(0) @binding(0) var<uniform> d: DimsUniform;
@group(0) @binding(1) var jfa_tex: texture_2d<f32>;
@group(0) @binding(2) var dist_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn cs_distance_field(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (f32(gid.x) >= d.dims.x || f32(gid.y) >= d.dims.y) {
        return;
    }
    let uv = (vec2<f32>(gid.xy) + 0.5) / d.dims;
    let seed = textureLoad(jfa_tex, vec2<i32>(gid.xy), 0).xy;
    var dist = 1.0;
    if (seed.x >= 0.0) {
        dist = clamp(distance(uv, seed), 0.0, 1.0);
    }
    textureStore(dist_out, vec2<i32>(gid.xy), vec4<f32>(dist, 0.0, 0.0, 0.0));
}
