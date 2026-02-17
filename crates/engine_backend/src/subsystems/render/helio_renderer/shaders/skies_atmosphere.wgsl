// Helio Skies - Atmospheric Scattering Shader
// Full-screen post-process for aerial perspective and atmospheric scattering
// TODO: Implement full Rayleigh + Mie scattering integration

struct AtmosphereUniforms {
    planet_radius: f32,
    atmosphere_radius: f32,
    rayleigh_coefficient: vec3<f32>,
    mie_coefficient: f32,
    rayleigh_scale_height: f32,
    mie_scale_height: f32,
    sun_intensity: f32,
    scatter_samples: u32,
}

struct CameraUniforms {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    sun_direction: vec3<f32>,
}

@group(0) @binding(0)
var<uniform> atmosphere: AtmosphereUniforms;

@group(1) @binding(0)
var<uniform> camera: CameraUniforms;

@group(2) @binding(0)
var scene_color: texture_2d<f32>;

@group(2) @binding(1)
var scene_depth: texture_depth_2d;

@group(2) @binding(2)
var color_sampler: sampler;

// Fullscreen triangle vertex shader
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    // Generate fullscreen triangle
    let x = f32((vertex_index << 1u) & 2u);
    let y = f32(vertex_index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    // TODO: Implement atmospheric scattering
    // For now, just pass through the scene color
    let uv = frag_coord.xy / vec2<f32>(1600.0, 900.0);
    let scene = textureSample(scene_color, color_sampler, uv);
    return scene;
}
