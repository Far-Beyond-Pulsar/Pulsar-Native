// Helio Skies - Atmosphere Shader Functions
// Provides functions for sky rendering, aerial perspective, and volumetric effects

// Atmosphere constants
const PLANET_RADIUS: f32 = 6371000.0; // meters
const ATMOSPHERE_HEIGHT: f32 = 60000.0; // meters
const RAYLEIGH_SCALE: f32 = 8000.0;
const MIE_SCALE: f32 = 1200.0;

// Sun parameters (hardcoded for now)
const SUN_DIRECTION: vec3<f32> = vec3<f32>(0.3, 0.7, 0.5);
const SUN_INTENSITY: f32 = 20.0;

/// Apply sky color with smart blending based on surface normal
fn apply_sky_color(color: vec3<f32>, world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    let view_dir = normalize(world_pos - camera_pos);
    let sky_color = calculate_sky_color(world_pos, camera_pos, SUN_DIRECTION);
    
    // Very subtle blend - just add atmospheric tint without destroying lighting
    // Only 5% sky influence so we don't overpower the scene
    let blend_factor = 0.05;
    return mix(color, sky_color, blend_factor);
}

/// Calculate procedural sky color based on view direction
fn calculate_sky_color(world_pos: vec3<f32>, camera_pos: vec3<f32>, sun_dir: vec3<f32>) -> vec3<f32> {
    let view_dir = normalize(world_pos - camera_pos);
    let sun_dot = dot(view_dir, normalize(sun_dir));
    
    // Sky gradient based on altitude (Y component)
    let altitude = view_dir.y;
    let altitude_factor = saturate(altitude);
    
    // Color palette
    let horizon_color = vec3<f32>(0.8, 0.6, 0.4);
    let zenith_color = vec3<f32>(0.2, 0.4, 0.8);
    let ground_color = vec3<f32>(0.1, 0.1, 0.12);
    
    // Interpolate based on altitude
    var sky_color: vec3<f32>;
    if (altitude < 0.0) {
        let ground_factor = saturate(-altitude * 2.0);
        sky_color = mix(horizon_color, ground_color, ground_factor);
    } else {
        let sky_factor = pow(altitude_factor, 0.5);
        sky_color = mix(horizon_color, zenith_color, sky_factor);
    }
    
    // Add sun glow (Mie scattering approximation)
    let sun_glow = pow(saturate(sun_dot), 32.0) * 2.0;
    let sun_halo = pow(saturate(sun_dot), 8.0) * 0.3;
    let sun_color = vec3<f32>(1.0, 0.95, 0.8);
    
    sky_color = sky_color + sun_color * (sun_glow + sun_halo);
    
    return sky_color;
}

/// Apply aerial perspective (atmospheric scattering over distance)
fn apply_aerial_perspective(color: vec3<f32>, world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    let distance = length(world_pos - camera_pos);
    
    // Simple exponential fog for aerial perspective
    let fog_amount = 1.0 - exp(-distance * 0.0001);
    let sky_color = calculate_sky_color(world_pos, camera_pos, SUN_DIRECTION);
    
    return mix(color, sky_color, fog_amount * 0.3);
}

/// Apply volumetric fog effect
fn apply_volumetric_fog(color: vec3<f32>, world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    let distance = length(world_pos - camera_pos);
    let height = world_pos.y;
    
    // Height-based fog density
    let height_factor = exp(-height * 0.2);
    let fog_density = 0.001 * height_factor;
    
    // Distance fog
    let fog_amount = 1.0 - exp(-distance * fog_density);
    let fog_color = vec3<f32>(0.7, 0.8, 0.9);
    
    return mix(color, fog_color, saturate(fog_amount));
}
