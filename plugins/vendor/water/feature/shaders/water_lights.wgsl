// SceneDB's joined, camera-relative GpuLight rows (128 bytes). Empty and
// disabled rows have zero intensity. Keep the trailing optical metadata so
// array stride matches the light publisher, even though water doesn't use it.
struct WaterLight {
    position_range: vec4f,
    direction_outer: vec4f,
    color_intensity: vec4f,
    shadow_index: u32,
    light_type: u32,
    inner_angle: f32,
    flags: u32,
    metadata: array<u32, 16>,
}

struct WaterLocalLighting {
    specular: vec3f,
    diffuse: vec3f,
}

// The surface target is rgba16float. Near-field lights and sun glints can
// exceed its finite range even when all shading arithmetic is finite.
// Saturate at the storage boundary, before AA/bloom can spread infinities.
fn water_hdr_color(color: vec4f) -> vec4f {
    let magnitude = bitcast<vec3u>(color.rgb) & vec3u(0x7fffffffu);
    let clean = select(color.rgb, vec3f(0.0), magnitude > vec3u(0x7f800000u));
    return vec4f(clamp(clean, vec3f(0.0), vec3f(65504.0)), color.a);
}

fn water_local_lighting(
    world_pos: vec3f, normal: vec3f, foam_normal: vec3f,
    view_dir: vec3f, alpha: f32,
) -> WaterLocalLighting {
    var result = WaterLocalLighting(vec3f(0.0), vec3f(0.0));
    let no_v = max(dot(normal, -view_dir), 1e-4);
    let a = max(alpha, 0.015);
    let a2 = a * a;
    for (var i = 0u; i < arrayLength(&water_lights); i++) {
        let light = water_lights[i];
        // The atmosphere already supplies the sun. Do not count it twice.
        if light.light_type == 0u || light.color_intensity.w <= 0.0 {
            continue;
        }
        let to_light = light.position_range.xyz - world_pos;
        let dist = length(to_light);
        let range = light.position_range.w;
        if range <= 0.0 || dist >= range { continue; }
        let l = to_light / max(dist, 1e-4);
        let relative_dist = dist / range;
        var attenuation = max(1.0 - pow(relative_dist, 4.0), 0.0)
            / (dist * dist + 0.0001);
        if light.light_type == 2u {
            let cos_angle = dot(-l, light.direction_outer.xyz);
            let cone = clamp((cos_angle - light.direction_outer.w)
                / max(light.inner_angle - light.direction_outer.w, 1e-5), 0.0, 1.0);
            attenuation *= cone * cone * (3.0 - 2.0 * cone);
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
