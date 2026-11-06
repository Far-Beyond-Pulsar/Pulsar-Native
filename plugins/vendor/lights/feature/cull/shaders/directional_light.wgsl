//!use light_list
// The lights' directional light (`helio_light_api::DIRECTIONAL_LIGHT`): the
// light list's first sun (`light_is_sun`), whoever wrote the list (the
// lights' join, or a host's own rows). [0] toward it with w 1, [1] its
// radiance (colour times intensity); zero without one. One thread.

@group(0) @binding(0) var<storage, read> lights: array<GpuLight>;
@group(0) @binding(1) var<storage, read_write> directional: array<vec4<f32>>;

@compute @workgroup_size(1)
fn cs_directional_light() {
    var toward = vec4<f32>(0.0);
    var radiance = vec4<f32>(0.0);
    for (var i = 0u; i < arrayLength(&lights); i++) {
        let light = lights[i];
        if light_is_sun(light) {
            toward = vec4<f32>(-normalize(light.direction_outer.xyz), 1.0);
            radiance = vec4<f32>(light.color_intensity.rgb * light.color_intensity.w, 0.0);
            break;
        }
    }
    directional[0] = toward;
    directional[1] = radiance;
}
