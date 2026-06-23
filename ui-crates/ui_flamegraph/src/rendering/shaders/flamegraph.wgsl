struct Uniforms {
    viewport_x: f32, viewport_y: f32,
    pan_x: f32, pan_y: f32,
    zoom: f32, thread_label_width: f32,
    y_adj: f32, row_h: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct GpuSpan {
    start_rel_ns: f32,
    end_rel_ns: f32,
    y: f32,
    color_index: u32,
    span_count: u32,
    depth: u32,
    _pad: vec2<u32>,
};

@group(0) @binding(1) var<storage, read> spans: array<GpuSpan>;
@group(0) @binding(2) var<storage, read> palette: array<vec4<f32>>;

const SPAN_FRAC: f32 = 0.8;
const PADDING: f32 = 1.0;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> VertexOut {
    let span = spans[ii];
    let x1 = span.start_rel_ns * u.zoom + u.pan_x + u.thread_label_width;
    let x2 = span.end_rel_ns * u.zoom + u.pan_x + u.thread_label_width;
    let w = x2 - x1;

    // Degenerate for off-screen or too-narrow
    if x2 < u.thread_label_width || x1 > u.viewport_x || w < 0.5 {
        var out: VertexOut;
        out.clip_pos = vec4(0.0, 0.0, 0.0, 0.0);
        return out;
    }

    let rw = select(select(w, max(w - PADDING * 2.0, 1.0), w >= 3.0), 1.0, w < 1.0);
    let sx = select(x1, x1 + PADDING, w >= 3.0);

    let col = palette[span.color_index & 15u];

    let qh = (u.row_h - PADDING) * SPAN_FRAC;
    let sy = span.y + u.y_adj + u.pan_y + PADDING;

    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
    );
    let uv = corners[vi];
    let px = sx + uv.x * rw;
    let py = sy + uv.y * qh;

    let ndc_x = (px / u.viewport_x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (py / u.viewport_y) * 2.0;

    var out: VertexOut;
    out.clip_pos = vec4(ndc_x, ndc_y, 0.0, 1.0);
    out.color = col;
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    return input.color;
}
