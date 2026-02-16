// Gizmo shader - simple unlit overlay rendering
// blade-graphics uses name-based binding: variable names must match
// the field names in the Rust ShaderData structs. No @group/@binding needed.

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    position: vec3<f32>,
}

struct GizmoInstance {
    world_position: vec3<f32>,
    color: vec4<f32>,
    axis_direction: vec3<f32>,
    scale: f32,
}

var<uniform> camera: CameraUniforms;
var<uniform> gizmo: GizmoInstance;

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
    
    // Create rotation matrix to align arrow with axis
    let axis = normalize(gizmo.axis_direction);
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(axis.y) > 0.9);
    let right = normalize(cross(up, axis));
    let actual_up = cross(axis, right);
    
    let rotation = mat3x3<f32>(
        right,
        axis,
        actual_up
    );
    
    // Apply rotation and scale
    let rotated_pos = rotation * (in.position * gizmo.scale);
    let world_pos = gizmo.world_position + rotated_pos;
    
    out.clip_position = camera.view_proj * vec4<f32>(world_pos, 1.0);
    out.color = gizmo.color;
    
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Simple unlit color
    return in.color;
}
