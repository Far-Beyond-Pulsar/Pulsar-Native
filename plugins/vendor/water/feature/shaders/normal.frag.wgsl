// normal.frag.wgsl — recomputes surface normals from the height gradient.
//
// Reads R (height) and writes the XZ components of the unit normal into BA.
// Texture layout (Rgba16Float):
//   R = height  (read only)
//   G = velocity (read only, pass through)
//   B = normal.x (written)
//   A = normal.z (written)

@group(0) @binding(0) var water_texture: texture_2d<f32>;
@group(0) @binding(1) var water_sampler: sampler;

struct NormalUniforms {
    /// Texel size: (1 / texture_width, 1 / texture_height)
    delta: vec2<f32>,
}
@group(0) @binding(2) var<uniform> u: NormalUniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    var info = textureSample(water_texture, water_sampler, uv);

    let h       = info.r;
    let h_right = textureSample(water_texture, water_sampler, vec2<f32>(uv.x + u.delta.x, uv.y)).r;
    let h_up    = textureSample(water_texture, water_sampler, vec2<f32>(uv.x, uv.y + u.delta.y)).r;

    // Tangent vectors in XYZ (Y is the height axis)
    let tangent_x = vec3<f32>(u.delta.x, h_right - h, 0.0);
    let tangent_z = vec3<f32>(0.0,       h_up    - h, u.delta.y);

    let normal = normalize(cross(tangent_z, tangent_x));
    info.b = normal.x;
    info.a = normal.z;

    return info;
}
