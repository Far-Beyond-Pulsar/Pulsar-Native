// drop.frag.wgsl — adds a cosine-falloff ripple to the water heightfield.
//
// Texture layout (Rgba16Float):
//   R = height
//   G = velocity
//   B = normal.x
//   A = normal.z

@group(0) @binding(0) var water_texture: texture_2d<f32>;
@group(0) @binding(1) var water_sampler: sampler;

struct DropUniforms {
    /// Drop position in [-1, 1] range (maps to sim-texture UV space via * 0.5 + 0.5)
    center: vec2<f32>,
    /// Drop radius (fraction of texture space, e.g. 0.05)
    radius: f32,
    /// Height increment to add at the drop center (can be negative)
    strength: f32,
}
@group(0) @binding(2) var<uniform> u: DropUniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    var info = textureSample(water_texture, water_sampler, uv);

    // Distance from drop centre in UV space (centre converted from [-1,1] to [0,1])
    let drop = max(0.0, 1.0 - length(u.center * 0.5 + 0.5 - uv) / u.radius);
    let drop_val = 0.5 - cos(drop * 3.14159265) * 0.5;

    info.r += drop_val * u.strength;
    return info;
}
