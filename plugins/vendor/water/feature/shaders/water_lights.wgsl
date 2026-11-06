// The water's local lights: the lights' light list (`GpuLight` rows,
// camera-relative, `//!use light_list` in the composing shader, bound as
// `water_lights`). Empty and disabled rows have zero intensity.

struct WaterLocalLighting {
    specular: vec3f,
    diffuse: vec3f,
}

fn water_local_lighting(
    world_pos: vec3f, normal: vec3f, foam_normal: vec3f,
    view_dir: vec3f, alpha: f32, sun_direction: vec3f,
) -> WaterLocalLighting {
    var result = WaterLocalLighting(vec3f(0.0), vec3f(0.0));
    let no_v = max(dot(normal, -view_dir), 1e-4);
    let a = max(alpha, 0.015);
    let a2 = a * a;
    let primary_sun = normalize(sun_direction);
    for (var i = 0u; i < arrayLength(&water_lights); i++) {
        let light = water_lights[i];
        if light.color_intensity.w <= 0.0 {
            continue;
        }
        var l: vec3f;
        var attenuation = 1.0;
        if light.light_type == LIGHT_DIRECTIONAL {
            // The atmospheric surface path handles every directional light
            // together. Keep this fallback for water scenes without sky.
            if atmosphere.planet.w >= 0.5 { continue; }
            l = -normalize(light.direction_outer.xyz);
            if dot(l, primary_sun) > 0.9999 { continue; }
        } else {
            let to_light = light.position_range.xyz - world_pos;
            let dist = length(to_light);
            let range = light.position_range.w;
            if range <= 0.0 || dist >= range { continue; }
            l = to_light / max(dist, 1e-4);
            let relative_dist = dist / range;
            attenuation = max(1.0 - pow(relative_dist, 4.0), 0.0)
                / (dist * dist + 0.0001);
            if light.light_type == LIGHT_SPOT {
                let cos_angle = dot(-l, light.direction_outer.xyz);
                let cone = clamp((cos_angle - light.direction_outer.w)
                    / max(light.inner_angle - light.direction_outer.w, 1e-5), 0.0, 1.0);
                attenuation *= cone * cone * (3.0 - 2.0 * cone);
            }
        }
        let radiance = light.color_intensity.rgb * light.color_intensity.w * attenuation;
        result.diffuse += radiance * max(dot(foam_normal, l), 0.0) / HELIO_PI;
        let no_l = max(dot(normal, l), 0.0);
        if no_l <= 0.0 { continue; }
        let h = (l - view_dir) / max(length(l - view_dir), 1e-4);
        let no_h = max(dot(normal, h), 0.0);
        let vo_h = max(dot(-view_dir, h), 0.0);
        let d0 = no_h * no_h * (a2 - 1.0) + 1.0;
        let distribution = a2 / (HELIO_PI * d0 * d0);
        let visibility = 0.5 / (no_l * sqrt(no_v * no_v * (1.0 - a2) + a2)
            + no_v * sqrt(no_l * no_l * (1.0 - a2) + a2) + 1e-5);
        let fresnel = 0.02 + 0.98 * pow(1.0 - vo_h, 5.0);
        result.specular += radiance * distribution * visibility * fresnel * no_l;
    }
    return result;
}
