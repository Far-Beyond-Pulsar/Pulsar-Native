// Composites the final (cascade-0) radiance texture over whatever the
// preceding passes already drew, via a multiply blend (`src * dst`, no
// framebuffer read needed — `BlendFactor::Dst, BlendFactor::Zero`): an
// `ambient` floor plus the computed radiance darkens unlit areas and warms
// up anything near an emitter, without touching alpha or needing a second
// scene-color copy.

struct CompositeUniforms {
    ambient: vec3<f32>,
    exposure: f32,
}

@group(0) @binding(0) var<uniform> cu: CompositeUniforms;
@group(0) @binding(1) var radiance_tex: texture_2d<f32>;
@group(0) @binding(2) var radiance_sampler: sampler;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // Oversized fullscreen triangle from the vertex index alone (no vertex
    // buffer) — one vertex on each far side of clip space so the triangle's
    // hull covers the *entire* viewport after clipping, not just the unit
    // square inscribed between three on-screen corners.
    let u = f32((idx << 1u) & 2u);
    let v = f32(idx & 2u);
    var out: VsOut;
    out.uv = vec2<f32>(u, v);
    out.clip_pos = vec4<f32>(u * 2.0 - 1.0, 1.0 - v * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let radiance = textureSample(radiance_tex, radiance_sampler, in.uv).rgb;
    let lit = cu.ambient + radiance * cu.exposure;
    return vec4<f32>(lit, 1.0);
}
