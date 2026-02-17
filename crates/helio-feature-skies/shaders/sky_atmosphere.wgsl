// Helio Skies - Volumetric Atmospheric Sky Shader
// This shader applies dynamic sky colors to distant geometry (sky sphere)

// Atmosphere constants
const ATMOSPHERE_DENSITY: f32 = 0.00025;
const SKY_DISTANCE_THRESHOLD: f32 = 100.0; // Objects beyond this get sky treatment

// Sun parameters
const SUN_DIRECTION: vec3<f32> = vec3<f32>(0.3, 0.7, 0.5);
const SUN_INTENSITY: f32 = 20.0;

/// Calculate dynamic sky color based on view direction
fn calculate_sky_color(view_dir: vec3<f32>) -> vec3<f32> {
    let sun_dir = normalize(SUN_DIRECTION);
    let sun_dot = dot(view_dir, sun_dir);
    
    // Sky gradient based on altitude (Y component)
    let altitude = view_dir.y;
    let altitude_factor = saturate(altitude);
    
    // Color palette - vibrant sky colors
    let horizon_color = vec3<f32>(0.85, 0.7, 0.5);  // Warm horizon
    let zenith_color = vec3<f32>(0.2, 0.5, 0.95);   // Deep blue zenith
    let ground_color = vec3<f32>(0.12, 0.12, 0.15); // Dark ground
    
    // Interpolate based on altitude
    var sky_color: vec3<f32>;
    if (altitude < 0.0) {
        // Below horizon
        let ground_factor = saturate(-altitude * 2.0);
        sky_color = mix(horizon_color, ground_color, ground_factor);
    } else {
        // Above horizon
        let sky_factor = pow(altitude_factor, 0.4);
        sky_color = mix(horizon_color, zenith_color, sky_factor);
    }
    
    // Add sun glow (Mie scattering approximation)
    let sun_disk = smoothstep(0.9998, 0.9999, sun_dot); // Sharp sun disk
    let sun_glow = pow(saturate(sun_dot), 32.0) * 3.0;  // Bright glow
    let sun_halo = pow(saturate(sun_dot), 8.0) * 0.5;   // Soft halo
    let sun_color = vec3<f32>(1.0, 0.98, 0.9);
    
    sky_color = sky_color + sun_color * (sun_disk + sun_glow + sun_halo);
    
    // Atmospheric scattering - make horizon brighter
    let horizon_glow = pow(1.0 - abs(altitude), 2.0) * 0.3;
    sky_color = sky_color + vec3<f32>(0.95, 0.85, 0.7) * horizon_glow;
    
    return saturate(sky_color);
}

/// Apply volumetric fog effect to distant objects
fn apply_atmospheric_fog(color: vec3<f32>, distance: f32, view_dir: vec3<f32>) -> vec3<f32> {
    if (distance < 10.0) {
        return color;
    }
    
    // Exponential fog
    let fog_amount = 1.0 - exp(-distance * ATMOSPHERE_DENSITY);
    let sky_color = calculate_sky_color(view_dir);
    
    return mix(color, sky_color, saturate(fog_amount));
}

/// Main function: Apply volumetric sky based on distance
/// Distant objects (sky sphere at 500+ units) get full sky color
/// Close objects get atmospheric fog
fn apply_volumetric_sky(color: vec3<f32>, world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    let to_fragment = world_pos - camera_pos;
    let distance = length(to_fragment);
    let view_dir = normalize(to_fragment);
    
    // Sky sphere detection: very distant geometry becomes pure sky
    if (distance > SKY_DISTANCE_THRESHOLD) {
        let sky_blend = saturate((distance - SKY_DISTANCE_THRESHOLD) / 50.0);
        let sky_color = calculate_sky_color(view_dir);
        return mix(color, sky_color, sky_blend);
    } else {
        // Close objects get atmospheric fog
        return apply_atmospheric_fog(color, distance, view_dir);
    }
}
