//!use light_list
// The lights' directional lights (`helio_light_api::DIRECTIONAL_LIGHTS`):
// every sun of the light list (`light_is_sun`), in list order, whoever wrote
// the list (the lights' join, or a host's own rows). One thread.

@group(0) @binding(0) var<storage, read> lights: array<GpuLight>;
@group(0) @binding(1) var<storage, read_write> directional: DirectionalLights;

@compute @workgroup_size(1)
fn cs_directional_lights() {
    var count = 0u;
    for (var i = 0u; i < arrayLength(&lights) && count < MAX_DIRECTIONAL_LIGHTS; i++) {
        let light = lights[i];
        if light_is_sun(light) {
            directional.lights[count] = DirectionalLight(
                vec4<f32>(-normalize(light.direction_outer.xyz), 1.0),
                vec4<f32>(light.color_intensity.rgb * light.color_intensity.w, 0.0),
                vec4<f32>(
                    light.flare_scale,
                    light.flare_intensity,
                    f32(light.flare_type),
                    f32(light.flare_enabled),
                ),
            );
            count += 1u;
        }
    }
    // The primary sun's slot reads zero without one (readers binding the
    // row as `array<vec4<f32>>` test its w).
    if count == 0u {
        directional.lights[0] = DirectionalLight(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    }
    directional.count = vec4<u32>(count, 0u, 0u, 0u);
}
