// Box-filter the displacement and the first/second slope moments. Averaging
// `(sx, sz, sx², sz²)` retains LEAN-style unresolved variance at every mip.
@group(0) @binding(0) var displacement_src: texture_2d<f32>;
@group(0) @binding(1) var slopes_src: texture_2d<f32>;

struct MipUniform {
    source_size: vec2u,
    _pad: vec2u,
}
@group(0) @binding(2) var<uniform> u: MipUniform;

struct FragmentOutput {
    @location(0) displacement: vec4f,
    @location(1) slope_moments: vec4f,
}

fn box4_displacement(p: vec2i, extent: vec2u) -> vec4f {
    let hi = vec2i(extent) - vec2i(1);
    let p0 = min(p, hi);
    let p1 = min(p + vec2i(1, 0), hi);
    let p2 = min(p + vec2i(0, 1), hi);
    let p3 = min(p + vec2i(1, 1), hi);
    return (textureLoad(displacement_src, p0, 0) + textureLoad(displacement_src, p1, 0)
        + textureLoad(displacement_src, p2, 0) + textureLoad(displacement_src, p3, 0)) * 0.25;
}

fn box4_slopes(p: vec2i, extent: vec2u) -> vec4f {
    let hi = vec2i(extent) - vec2i(1);
    let p0 = min(p, hi);
    let p1 = min(p + vec2i(1, 0), hi);
    let p2 = min(p + vec2i(0, 1), hi);
    let p3 = min(p + vec2i(1, 1), hi);
    return (textureLoad(slopes_src, p0, 0) + textureLoad(slopes_src, p1, 0)
        + textureLoad(slopes_src, p2, 0) + textureLoad(slopes_src, p3, 0)) * 0.25;
}

@fragment
fn fs_main(@builtin(position) position: vec4f) -> FragmentOutput {
    let p = vec2i(position.xy) * 2;
    var out: FragmentOutput;
    out.displacement = box4_displacement(p, u.source_size);
    out.slope_moments = box4_slopes(p, u.source_size);
    return out;
}
