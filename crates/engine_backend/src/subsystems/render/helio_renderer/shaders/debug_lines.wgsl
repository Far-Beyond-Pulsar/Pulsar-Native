// Debug line shader - renders colored line quads as an overlay.
// Structured identically to gizmo.wgsl: single position vertex attribute,
// color passed as a uniform.  No @location needed — blade-graphics assigns
// locations from field order in the blade_macros::Vertex derive.

struct CameraUniforms {
    view_proj: mat4x4<f32>,
}

struct LineUniforms {
    color: vec4<f32>,
}

var<uniform> camera: CameraUniforms;
var<uniform> line_data: LineUniforms;

struct VertexInput {
    position: vec3<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.color = line_data.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
