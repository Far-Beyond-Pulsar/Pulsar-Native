struct Uniforms {
    viewport_x: f32, viewport_y: f32,
    pan_x: f32, pan_y: f32,
    zoom: f32, thread_label_width: f32,
    y_adj: f32, row_h: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: u32,
    @location(4) _pad: vec3<u32>,
) -> VertexOut {
    var px: vec2<f32>;
    if kind == 0u {
        // Axis-aligned quad (glyph / ruler / grid line). pos = top-left.
        let corners = array<vec2<f32>, 6>(
            vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
            vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
        );
        let uv = corners[vi];
        px = pos + uv * size;
    } else {
        // Rotated shape (curved arrow segments & arrowheads).
        // The rotation angle comes from the f32 bit pattern in _pad.x.
        let angle = bitcast<f32>(_pad.x);
        let sa = sin(angle);
        let ca = cos(angle);
        var off: vec2<f32>;
        if kind == 1u {
            // Rotated quad centered at pos; size = (length, thickness).
            let sq = array<vec2<f32>, 6>(
                vec2(-0.5, -0.5), vec2(0.5, -0.5), vec2(0.5, 0.5),
                vec2(-0.5, -0.5), vec2(0.5, 0.5), vec2(-0.5, 0.5),
            );
            off = sq[vi] * size;
        } else {
            // Triangle (arrowhead) centered at pos, tip toward +x;
            // size = (head_len, half_head_width).
            let tri = array<vec2<f32>, 6>(
                vec2(0.0, -0.5), vec2(1.0, 0.0), vec2(0.0, 0.5),
                vec2(0.0, -0.5), vec2(1.0, 0.0), vec2(0.0, 0.5),
            );
            off = tri[vi] * size;
        }
        let rot = vec2(off.x * ca - off.y * sa, off.x * sa + off.y * ca);
        px = pos + rot;
    }
    let ndc_x = (px.x / u.viewport_x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (px.y / u.viewport_y) * 2.0;
    var out: VertexOut;
    out.clip_pos = vec4(ndc_x, ndc_y, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    return input.color;
}
