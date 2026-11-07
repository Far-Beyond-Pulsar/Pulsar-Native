// fullscreen.vert.wgsl — produces a full-screen triangle pair from vertex_index
//
// The instance index is the water volume the draw simulates (passes that
// need it draw instance `volume..volume + 1`; the others ignore it).
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) volume: u32,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    var pos = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0, -1.0), vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0), vec2<f32>( 1.0, -1.0), vec2<f32>( 1.0,  1.0)
    );
    var output: VertexOutput;
    let p = pos[vertex_index];
    output.position = vec4<f32>(p, 0.0, 1.0);
    // uv: top-left = (0,0), bottom-right = (1,1) — y flipped from clip space
    output.uv = vec2<f32>((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    output.volume = instance_index;
    return output;
}
