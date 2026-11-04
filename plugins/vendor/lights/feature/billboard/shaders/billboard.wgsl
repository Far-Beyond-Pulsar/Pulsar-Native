//! Billboard shader - camera-facing instanced quads
//!
//! Vertex inputs:
//!   Slot 0 (per-vertex):   position (vec2), uv (vec2)
//!   Slot 1 (per-instance): world_pos_pad (vec4), scale_flags (vec4), color (vec4)

// Group 0: global uniforms (camera + globals)
// Layout must match GpuCameraUniforms in libhelio/src/camera.rs (368 bytes).
struct Camera {
    view:          mat4x4<f32>,   // offset   0 — world→view
    proj:          mat4x4<f32>,   // offset  64 — view→clip
    view_proj:     mat4x4<f32>,   // offset 128 — combined VP
    inv_view_proj: mat4x4<f32>,   // offset 192 — clip→world
    position_near: vec4<f32>,     // offset 256 — xyz=world pos, w=near
    forward_far:   vec4<f32>,     // offset 272 — xyz=forward, w=far
    jitter_frame:  vec4<f32>,     // offset 288
    prev_view_proj: mat4x4<f32>,  // offset 304
}
struct Globals {
    frame: u32,
    delta_time: f32,
    ambient_intensity: f32,
    _padding: f32,
}
@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform> globals: Globals;
@group(0) @binding(2) var<storage, read> instances: array<BillboardInstance>;

// Group 1: sprite texture
@group(1) @binding(0) var sprite_tex:     texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;

// ── Vertex inputs ──────────────────────────────────────────────────────────

struct QuadVertex {
    @location(0) position: vec2<f32>,
    @location(1) uv:       vec2<f32>,
}

struct BillboardInstance {
    world_pos_pad: vec4<f32>,
    scale_flags:   vec4<f32>,
    color:         vec4<f32>,
}

// ── Vertex output ───────────────────────────────────────────────────────────

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       uv:       vec2<f32>,
    @location(1)       color:    vec4<f32>,
}

// ── Vertex shader ───────────────────────────────────────────────────────────

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOut {
    let quad_positions = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, -0.5), vec2<f32>(-0.5, 0.5),
        vec2<f32>(-0.5, 0.5), vec2<f32>(0.5, -0.5), vec2<f32>(0.5, 0.5));
    let quad_uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    let quad_position = quad_positions[vertex_index];
    let quad_uv = quad_uvs[vertex_index];
    let inst = instances[instance_index];
    // In the frame's space, like the camera.
    let world_pos    = inst.world_pos_pad.xyz;
    let scale        = inst.scale_flags.xy;
    let screen_scale = inst.scale_flags.z > 0.5;

    // Build camera-facing (billboard) basis vectors
    let cam_pos = cameras[0].position_near.xyz;
    let to_cam  = normalize(cam_pos - world_pos);

    // Right and up vectors perpendicular to the view direction
    let world_up = vec3<f32>(0.0, 1.0, 0.0);
    let right    = normalize(cross(world_up, to_cam));
    let up       = cross(to_cam, right);

    // Offset in world space using the quad's local position
    var offset = right * quad_position.x * scale.x
               + up    * quad_position.y * scale.y;

    // Optional: constant screen-space scaling.
    // We must scale by VIEW-AXIS depth (dot with forward), NOT Euclidean distance.
    // Perspective division uses view-depth, so: world_size = scale * view_depth
    // → projected NDC size = scale * view_depth / view_depth = scale (constant).
    // Using Euclidean distance instead causes off-axis billboards to appear larger.
    // TODO: Can we use a branchless multiplication here somehow?
    if screen_scale {
        let view_depth = max(dot(cameras[0].forward_far.xyz, world_pos - cam_pos), 0.001);
        offset        *= view_depth;
    }

    let final_pos = world_pos + offset;

    var out: VertexOut;
    out.clip_pos = cameras[0].view_proj * vec4<f32>(final_pos, 1.0); // view_proj at offset 128
    // An instance at the eye has no facing direction (normalize(0) is NaN):
    // collapse it outside the clip volume instead of emitting NaN vertices.
    let rel = cam_pos - world_pos;
    if !(dot(rel, rel) > 1e-8) {
        out.clip_pos = vec4<f32>(0.0, 0.0, -1.0, 1.0);
    }
    out.uv       = quad_uv;
    out.color    = inst.color;
    return out;
}

// ── Fragment shader ─────────────────────────────────────────────────────────

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let tex_color = textureSample(sprite_tex, sprite_sampler, in.uv);
    // Tint the sprite by the per-instance color; use texture alpha for transparency
    let rgb   = tex_color.rgb * in.color.rgb;
    let alpha = tex_color.a   * in.color.a;
    if alpha < 0.01 { discard; }
    return vec4<f32>(rgb, alpha);
}
