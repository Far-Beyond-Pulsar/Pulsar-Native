fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    let V = normalize(input.view_position - input.world_position);
    let NdV = max(dot(s.normal, V), 0.0001);
    let fresnel = pow(1.0 - NdV, 4.0);

    // Glass color from material base (tinted glass)
    let glass_color = material.base_color.rgb;
    let clear = vec3<f32>(0.97, 0.98, 0.99);

    // Fresnel: reflective at grazing, transparent straight-on
    let reflect_col = vec3<f32>(0.12, 0.15, 0.20);
    let body_col = mix(clear, glass_color, 0.15);
    s.albedo = vec4<f32>(mix(body_col, reflect_col, fresnel), 1.0);

    // Subtle normal perturbation
    let perturb = sin(input.world_position * 12.0 + f32(input.frame) * 0.01) * 0.008;
    let T = normalize(cross(s.normal, vec3<f32>(0.0, 1.0, 0.0)));
    if length(T) > 0.001 {
        s.normal = normalize(s.normal + T * perturb.x + cross(s.normal, T) * perturb.y);
    }

    // Subsurface scattering for light transmission
    s.flags = s.flags | SURFACE_FLAG_SUBSURFACE;
    s.subsurface_color = mix(glass_color, vec3<f32>(0.9, 0.9, 1.0), 0.5) * 0.3;
    s.subsurface_radius = 0.8;

    // Glass specular: glossy with high Fresnel reflection
    s.roughness = 0.015;
    s.metallic = 0.0;
    s.specular_f0 = mix(vec3<f32>(0.04), vec3<f32>(0.15), fresnel);
    s.roughness_aniso_x = 0.015;
    s.roughness_aniso_y = 0.030;

    return s;
}