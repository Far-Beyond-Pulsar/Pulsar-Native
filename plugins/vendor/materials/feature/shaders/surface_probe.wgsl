// The materials' surface for the frame's shading-cost probe
// (`helio_frame_api::diagnostics::SurfaceProbe`): the default PBR's
// Cook-Torrance through `pbr_eval`, the reflected radiance per unit light.
fn surface_probe(
    n: vec3<f32>,
    v: vec3<f32>,
    l: vec3<f32>,
    base_color: vec3<f32>,
    roughness: f32,
    metallic: f32,
) -> vec3<f32> {
    let h = normalize(v + l);
    let n_dot_v = max(dot(n, v), 0.001);
    let n_dot_l = max(dot(n, l), 0.001);
    let f0 = mix(vec3<f32>(0.04), base_color, metallic);
    let f = fresnel_schlick(max(dot(h, v), 0.0), f0);
    let specular = distribution_ggx(n, h, roughness) * geometry_smith(n, v, l, roughness) * f
        / max(4.0 * n_dot_v * n_dot_l, 0.001);
    let diffuse = (vec3<f32>(1.0) - f) * (1.0 - metallic) * base_color / PI;
    return (diffuse + specular) * n_dot_l;
}
