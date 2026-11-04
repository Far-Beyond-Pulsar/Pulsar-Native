// The lights' forward light function (`helio_light_api`, `//!use
// light_forward`; needs `//!use light_list` and `//!use pbr_eval`): the
// direct light a surface point receives from the lights of its screen tile,
// for drawers that light what they draw (the forward and transparent
// phases). The drawer binds the light list as `lights: array<GpuLight>` and
// the binning as `tile_light_lists` / `tile_light_counts` (both
// `array<u32>`), which the lights publish.

// One light's GGX direct radiance at `world_pos`.
fn light_forward_one(
    light:     GpuLight,
    world_pos: vec3<f32>,
    N:         vec3<f32>,
    V:         vec3<f32>,
    F0:        vec3<f32>,
    albedo:    vec3<f32>,
    roughness: f32,
    metallic:  f32,
) -> vec3<f32> {
    var L:        vec3<f32>;
    var radiance: vec3<f32>;
    if light.light_type == LIGHT_DIRECTIONAL {
        L = normalize(-light.direction_outer.xyz);
        radiance = light.color_intensity.xyz * light.color_intensity.w;
    } else {
        let to_light = light.position_range.xyz - world_pos;
        let dist = length(to_light);
        if dist > light.position_range.w { return vec3<f32>(0.0); }
        L = to_light / dist;
        var atten = 1.0 / (dist * dist + 0.0001);
        let normalized_dist = dist / light.position_range.w;
        atten *= max(0.0, 1.0 - normalized_dist * normalized_dist * normalized_dist * normalized_dist);
        if light.light_type == LIGHT_SPOT {
            let cos_a = dot(-L, light.direction_outer.xyz);
            atten *= smoothstep(light.direction_outer.w, light.inner_angle, cos_a);
        }
        radiance = light.color_intensity.xyz * light.color_intensity.w * atten;
    }

    let NdL = max(dot(N, L), 0.0);
    if NdL == 0.0 { return vec3<f32>(0.0); }
    if all(radiance < vec3<f32>(0.002)) { return vec3<f32>(0.0); }

    let H = normalize(V + L);
    let F = fresnel_schlick(max(dot(H, V), 0.0), F0);
    let kD = (1.0 - F) * (1.0 - metallic);
    let NdV = max(dot(N, V), 0.0);
    let D = distribution_ggx(N, H, roughness);
    let G = geometry_smith(N, V, L, roughness);
    let specular = D * G * F / (4.0 * NdV * NdL + 0.0001);
    return (kD * albedo / PI + specular) * radiance * NdL;
}

// The direct light at `world_pos` (seen from `V`, normal `N`) from every
// light of the tile holding pixel `frag_xy`, `num_tiles_x` tiles a row.
fn light_forward(
    frag_xy:     vec2<f32>,
    num_tiles_x: u32,
    world_pos:   vec3<f32>,
    N:           vec3<f32>,
    V:           vec3<f32>,
    F0:          vec3<f32>,
    albedo:      vec3<f32>,
    roughness:   f32,
    metallic:    f32,
) -> vec3<f32> {
    let tile = light_tile_index(frag_xy, num_tiles_x);
    let count = min(tile_light_counts[tile], LIGHT_MAX_PER_TILE);
    var lit = vec3<f32>(0.0);
    for (var i = 0u; i < count; i++) {
        let light = lights[tile_light_lists[tile * LIGHT_MAX_PER_TILE + i]];
        lit += light_forward_one(light, world_pos, N, V, F0, albedo, roughness, metallic);
    }
    return lit;
}
