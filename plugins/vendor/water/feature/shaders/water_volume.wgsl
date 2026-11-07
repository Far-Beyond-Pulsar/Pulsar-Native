// A water volume's row (`helio_pass_water_sim::WATER_VOLUME_SNIPPET`,
// marker `water_volume`): `GpuWaterVolume` (gpu_types.rs) field for field,
// 16 vec4s, 256 bytes. Every water shader that reads the volumes' rows (the
// surface, caustics, simulation, spray, underwater passes, transmission)
// declares its own binding of `array<WaterVolume>` and takes the row from
// here; it never declares the row.

struct WaterVolume {
    bounds_min:            vec4f,  // xyz = min corner
    bounds_max:            vec4f,  // xyz = max corner, w = surface (rest) height
    wave_params:           vec4f,  // x = amplitude (metres), y = frequency, z = speed, w = steepness
    wave_direction:        vec4f,  // xy = direction (XZ)
    water_color:           vec4f,  // xyz = medium colour, w = Jacobian foam bias
    extinction:            vec4f,  // xyz = absorption per metre (Beer-Lambert), w = Jacobian foam scale
    reflection_refraction: vec4f,  // x = reflection, y = refraction, z = fresnel power, w = foam brightness
    caustics_params:       vec4f,  // x = enabled, y = intensity, z = scale, w = speed
    fog_params:            vec4f,  // x = density, y = god rays intensity
    sim_params:            vec4f,  // x = ior, y = caustic intensity, z = fresnel min, w = density
    shadow_params:         vec4f,  // x = rim, y = hitbox shadow, z = ambient occlusion
    sun_direction:         vec4f,  // xyz = toward the sun, w = 1 when the scene has one
    ssr_params:            vec4f,  // x = enable, y = max steps, z = step size, w = thickness
    sim_dynamics:          vec4f,  // x = spring, y = damping, z = wave scale
    wind_params:           vec4f,  // xy = direction (XZ), z = strength, w = own wind
    world_phase:           vec4f,  // xy = the frame's world origin XZ mod WATER_WORLD_PERIOD, z = 1 bounded
}

// Whether the water of `vol` reaches the world point `xz`: everywhere for an
// ocean, inside its XZ box for a bounded volume (a lake or pool).
fn water_volume_covers(vol: WaterVolume, xz: vec2f) -> bool {
    return vol.world_phase.z < 0.5
        || (all(xz >= vol.bounds_min.xz) && all(xz <= vol.bounds_max.xz));
}

// Whether `point` is inside the water body's extent: an ocean has no floor
// or sides; a bounded volume ends at its box.
fn water_volume_contains(vol: WaterVolume, point: vec3f) -> bool {
    return water_volume_covers(vol, point.xz)
        && (vol.world_phase.z < 0.5 || point.y >= vol.bounds_min.y);
}
