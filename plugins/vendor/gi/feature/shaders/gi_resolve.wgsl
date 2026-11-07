//!use helio_prelude
//!use gi_common
//!use gi_sample
// The probes' irradiance at every pixel (README.md, "Deferred lighting"):
// a pixel's word pair of `gi_irradiance` (rows of the frame) holds its
// irradiance / pi (rgb, half floats) and how much the probes hold it (the
// fourth half: 0 past the clipmap, or for the sky).

@group(0) @binding(0) var<uniform> frame_gi: GiFrame;
@group(0) @binding(1) var<storage, read> gi_probes: array<u32>;
@group(0) @binding(2) var<storage, read> cameras: array<Camera>;
@group(0) @binding(3) var gbuf_normal: texture_2d<f32>;
@group(0) @binding(4) var gbuf_depth: texture_depth_2d;
@group(0) @binding(5) var<storage, read_write> out_irradiance: array<vec2<u32>>;
// x: the renderer's debug mode (GI_PROBES_VIEW: the probes instead).
@group(0) @binding(6) var<uniform> resolve_view: vec4<u32>;

const GI_PROBES_VIEW: u32 = 36u;

// The probe nearest eye-relative point `p` in the finest level holding it,
// and its look: its irradiance toward `n`; dark red inside the ground,
// dark purple not yet updated; darker toward its cell's edges, so the
// probes' cells show.
fn probe_view(p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    for (var level = 0u; level < GI_LEVELS; level++) {
        let grid = frame_gi.levels[level];
        let local = (p - grid.origin.xyz) / grid.origin.w;
        if any(local < vec3<f32>(0.0)) || any(local > vec3<f32>(f32(GI_SIDE) - 1.0)) { continue; }
        let index = gi_probe_index(level, vec3<u32>(round(local)));
        let state = gi_probes[index * GI_PROBE_WORDS + GI_STATE_WORD];
        var colour = gi_probe_irradiance(index, n);
        if (state & GI_ACTIVE) == 0u { colour = vec3<f32>(0.35, 0.0, 0.0); }
        else if (state >> 8u) == 0u { colour = vec3<f32>(0.25, 0.0, 0.25); }
        let edge = max(abs(local - round(local)).x, max(abs(local - round(local)).y, abs(local - round(local)).z));
        return colour * mix(1.0, 0.4, smoothstep(0.42, 0.5, edge));
    }
    return vec3<f32>(0.0);
}

@compute @workgroup_size(8, 8)
fn gi_resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(gbuf_depth);
    if id.x >= size.x || id.y >= size.y { return; }
    let at = id.y * size.x + id.x;
    let depth = textureLoad(gbuf_depth, vec2<i32>(id.xy), 0);
    if helio_is_sky(depth) {
        out_irradiance[at] = vec2<u32>(0u);
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let h = cameras[0].view_proj_inv * vec4<f32>(helio_uv_to_ndc(uv), depth, 1.0);
    let p = h.xyz / h.w - cameras[0].position_near.xyz;
    let n = normalize(textureLoad(gbuf_normal, vec2<i32>(id.xy), 0).xyz);
    var e = gi_irradiance(p, n);
    if resolve_view.x == GI_PROBES_VIEW { e = vec4<f32>(probe_view(p, n), 1.0); }
    out_irradiance[at] = vec2<u32>(pack2x16float(e.rg), pack2x16float(vec2<f32>(e.b, e.a)));
}
