// Helio Skies - Volumetric Atmospheric Sky with Clouds
// Creates a realistic emissive sky with volumetric clouds

// Sky parameters
const SKY_DISTANCE_THRESHOLD: f32 = 400.0; // Sky sphere is at 500 units
const SUN_DIRECTION: vec3<f32> = vec3<f32>(0.3, 0.8, 0.4);

// Cloud parameters
const CLOUD_ALTITUDE_MIN: f32 = 0.3;  // Start clouds at 30% altitude
const CLOUD_ALTITUDE_MAX: f32 = 0.7;  // End clouds at 70% altitude
const CLOUD_DENSITY: f32 = 0.8;
const CLOUD_COVERAGE: f32 = 0.5;      // 0-1, how much sky is covered

/// Simple 3D noise function for clouds
fn noise3d(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    
    // Hash-based noise (simplified)
    let n = i.x + i.y * 57.0 + i.z * 113.0;
    let hash = fract(sin(n) * 43758.5453);
    
    return hash;
}

/// Fractal Brownian Motion for realistic cloud shapes
fn fbm(p: vec3<f32>) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var frequency = 1.0;
    var pp = p;
    
    for (var i = 0; i < 4; i++) {
        value += amplitude * noise3d(pp * frequency);
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    
    return value;
}

/// Calculate volumetric clouds
fn calculate_clouds(view_dir: vec3<f32>, time: f32) -> f32 {
    let altitude = view_dir.y;
    
    // Only render clouds in specific altitude range
    if (altitude < CLOUD_ALTITUDE_MIN || altitude > CLOUD_ALTITUDE_MAX) {
        return 0.0;
    }
    
    // Normalize altitude within cloud range
    let cloud_factor = (altitude - CLOUD_ALTITUDE_MIN) / (CLOUD_ALTITUDE_MAX - CLOUD_ALTITUDE_MIN);
    let cloud_fade = sin(cloud_factor * 3.14159);
    
    // Sample noise for cloud shapes (animate slowly)
    let cloud_pos = view_dir * 5.0 + vec3<f32>(time * 0.01, 0.0, 0.0);
    let cloud_noise = fbm(cloud_pos);
    
    // Apply coverage threshold
    let cloud = saturate((cloud_noise - (1.0 - CLOUD_COVERAGE)) * 3.0);
    
    return cloud * cloud_fade * CLOUD_DENSITY;
}

/// Calculate dynamic sky color with atmospheric scattering
fn calculate_sky_color(view_dir: vec3<f32>) -> vec3<f32> {
    let sun_dir = normalize(SUN_DIRECTION);
    let sun_dot = max(dot(view_dir, sun_dir), 0.0);
    
    let altitude = view_dir.y;
    let altitude_factor = saturate(altitude);
    
    // Sky colors (bright and vibrant for emissive look)
    let horizon_color = vec3<f32>(0.9, 0.7, 0.5);    // Bright warm horizon
    let zenith_color = vec3<f32>(0.4, 0.6, 1.0);     // Bright blue zenith
    let ground_color = vec3<f32>(0.15, 0.12, 0.1);   // Dark ground
    
    // Base sky gradient
    var sky_color: vec3<f32>;
    if (altitude < 0.0) {
        // Below horizon - dark ground
        let ground_factor = saturate(-altitude * 2.0);
        sky_color = mix(horizon_color * 0.5, ground_color, ground_factor);
    } else {
        // Above horizon - blue sky gradient
        let sky_factor = pow(altitude_factor, 0.35);
        sky_color = mix(horizon_color, zenith_color, sky_factor);
    }
    
    // Add bright sun
    let sun_disk = smoothstep(0.9996, 0.9999, sun_dot);
    let sun_glow = pow(sun_dot, 8.0) * 0.4;
    let sun_halo = pow(sun_dot, 3.0) * 0.15;
    let sun_color = vec3<f32>(1.5, 1.4, 1.2); // Bright emissive sun
    
    sky_color += sun_color * (sun_disk * 5.0 + sun_glow + sun_halo);
    
    // Atmospheric scattering near horizon
    let horizon_glow = pow(1.0 - abs(altitude), 3.0) * 0.4;
    sky_color += vec3<f32>(1.0, 0.8, 0.6) * horizon_glow;
    
    // Add volumetric clouds
    let cloud_amount = calculate_clouds(view_dir, 0.0); // TODO: pass time
    let cloud_color = vec3<f32>(1.2, 1.2, 1.3); // Bright white clouds
    
    // Sun lighting on clouds
    let cloud_sun_light = pow(sun_dot, 2.0) * 0.5 + 0.5;
    let lit_cloud_color = cloud_color * cloud_sun_light;
    
    // Blend clouds with sky
    sky_color = mix(sky_color, lit_cloud_color, cloud_amount);
    
    // Boost overall brightness for emissive look
    sky_color *= 1.2;
    
    return sky_color;
}

/// Main function: Apply volumetric sky to distant geometry only
fn apply_volumetric_sky(color: vec3<f32>, world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    let to_fragment = world_pos - camera_pos;
    let distance = length(to_fragment);
    let view_dir = normalize(to_fragment);
    
    // Only apply to sky sphere (very distant geometry)
    if (distance > SKY_DISTANCE_THRESHOLD) {
        // Full emissive sky color with clouds
        return calculate_sky_color(view_dir);
    } else {
        // Scene geometry - return original color unchanged
        return color;
    }
}
