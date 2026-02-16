// Simple unlit shader for gizmo overlay rendering
// No lighting, just flat axis colors

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    position: vec3<f32>,
    _padding: f32,
};

struct GizmoUniforms {
    model_matrix: mat4x4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: CameraUniforms;
@group(0) @binding(1) var<uniform> gizmo: GizmoUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    
    // Transform vertex to world space
    let world_pos = gizmo.model_matrix * vec4<f32>(in.position, 1.0);
    
    // Transform to clip space
    out.clip_position = camera.view_proj * world_pos;
    
    // Pass through unlit color
    out.color = gizmo.color;
    
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Simple unlit output - just the axis color
    return in.color;
}
