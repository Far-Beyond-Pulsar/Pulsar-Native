// Helio Skies - Sky Dome Shader
// Renders a procedural sky with Rayleigh scattering approximation
// blade-graphics uses name-based binding: variable names must match
// the field names in the Rust ShaderData structs. No @group/@binding needed.

struct SkyCameraUniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    sun_direction: vec3<f32>,
}

var<uniform> camera: SkyCameraUniforms;

struct VertexInput {
    position: vec3<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) view_direction: vec3<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    
    // Transform sky dome to world space
    let world_pos = in.position + camera.camera_position;
    out.clip_position = camera.view_proj * vec4<f32>(world_pos, 1.0);
    
    // TEST: Don't modify depth, render normally
    // out.clip_position.z = out.clip_position.w;
    
    out.world_position = world_pos;
    out.view_direction = normalize(in.position);
    
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // TEST: Output bright magenta to verify sky is rendering
    return vec4<f32>(1.0, 0.0, 1.0, 1.0);
    
    /* Original shader (disabled for testing)
    let view_dir = normalize(in.view_direction);
    let sun_dir = normalize(camera.sun_direction);
    
    // Calculate angle between view direction and sun
    let sun_dot = dot(view_dir, sun_dir);
    
    // Sky gradient based on altitude (Y component)
    let altitude = view_dir.y;
    let altitude_factor = saturate(altitude);
    
    // Horizon color (warm)
    let horizon_color = vec3<f32>(0.8, 0.6, 0.4);
    
    // Zenith color (blue sky)
    let zenith_color = vec3<f32>(0.2, 0.4, 0.8);
    
    // Ground color (darker)
    let ground_color = vec3<f32>(0.1, 0.1, 0.12);
    
    // Interpolate between ground, horizon, and zenith
    var sky_color: vec3<f32>;
    if (altitude < 0.0) {
        // Below horizon - ground color
        let ground_factor = saturate(-altitude * 2.0);
        sky_color = mix(horizon_color, ground_color, ground_factor);
    } else {
        // Above horizon - sky gradient
        let sky_factor = pow(altitude_factor, 0.5); // Non-linear gradient
        sky_color = mix(horizon_color, zenith_color, sky_factor);
    }
    
    // Add sun glow (Mie scattering approximation)
    let sun_glow = pow(saturate(sun_dot), 32.0) * 2.0;
    let sun_halo = pow(saturate(sun_dot), 8.0) * 0.3;
    let sun_color = vec3<f32>(1.0, 0.95, 0.8);
    
    // Add sun to sky
    sky_color = sky_color + sun_color * (sun_glow + sun_halo);
    
    // Add atmospheric scattering based on angle to sun
    let scatter_amount = pow(saturate(1.0 - altitude_factor), 2.0) * 0.3;
    let scatter_color = mix(sky_color, horizon_color, scatter_amount);
    
    return vec4<f32>(scatter_color, 1.0);
    */
}
