fn hash22(p: vec2f) -> vec2f {
    var p3 = fract(vec3f(p.xyx) * vec3f(0.1031, 0.1030, 0.0973));
    p3 += vec3f(dot(p3, p3.yzx));
    return fract((p3.xx + p3.yz) * p3.zy);
}

fn voronoi(p: vec2f, period: f32) -> vec2f {
    let i = floor(p);
    let f = fract(p);
    var f1 = 8.0;
    var f2 = 8.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let g = vec2f(f32(x), f32(y));
            let wrapped = i + g - floor((i + g) / period) * period;
            let offset = hash22(wrapped);
            let distance_to_cell = length(g + offset - f);
            if distance_to_cell < f1 {
                f2 = f1;
                f1 = distance_to_cell;
            } else if distance_to_cell < f2 {
                f2 = distance_to_cell;
            }
        }
    }
    return vec2f(f1, f2);
}

fn billow(uv: vec2f) -> f32 {
    let a = voronoi(uv * 7.0 + vec2f(0.3), 7.0);
    let b = voronoi(uv * 17.0 + vec2f(0.7), 17.0);
    let c = voronoi(uv * 41.0 + vec2f(0.1), 41.0);
    return 0.5 * sqrt(clamp((a.y - a.x) * 2.2, 0.0, 1.0))
         + 0.32 * sqrt(clamp((b.y - b.x) * 2.2, 0.0, 1.0))
         + 0.18 * sqrt(clamp((c.y - c.x) * 2.2, 0.0, 1.0));
}

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4f(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    out.uv = p;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let a = voronoi(in.uv * 9.0, 9.0);
    let b = voronoi(in.uv * 23.0 + vec2f(0.5), 23.0);
    let c = voronoi(in.uv * 61.0, 61.0);
    let lace = 1.0 - smoothstep(0.0, 0.22, a.y - a.x);
    let fine = 1.0 - smoothstep(0.0, 0.16, b.y - b.x);
    let bubbles = smoothstep(0.55, 0.15, c.x);
    let pattern = lace * 0.55 + fine * 0.3 + bubbles * 0.25 + (1.0 - a.x) * 0.25;
    let e = 1.0 / 1024.0;
    let gradient = vec2f(
        billow(in.uv + vec2f(e, 0.0)) - billow(in.uv - vec2f(e, 0.0)),
        billow(in.uv + vec2f(0.0, e)) - billow(in.uv - vec2f(0.0, e)),
    ) / (2.0 * e);
    return vec4f(clamp(pattern * 0.85, 0.0, 1.0), billow(in.uv),
                 clamp(vec2f(0.5) + gradient / 64.0, vec2f(0.0), vec2f(1.0)));
}
