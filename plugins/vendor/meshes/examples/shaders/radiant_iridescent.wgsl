// Radiant template: iridescent thin-film surface (a custom material class,
// registered by `radiant_demo`). The default PBR evaluation with thin-film
// interference added to F0: class_params.x = thin-film frequency,
// class_params.y = thin-film intensity. Like every template it is only the
// surface function, over the materials' snippets; a drawer composes it into
// its program (`RadiantTemplate::compose`).

fn thin_film_color(cos_theta: f32, frequency: f32, intensity: f32) -> vec3<f32> {
    let path_diff = 2.0 * cos_theta;
    return vec3<f32>(
        sin(path_diff * frequency * 1.0) * 0.5 + 0.5,
        sin(path_diff * frequency * 1.1) * 0.5 + 0.5,
        sin(path_diff * frequency * 1.2) * 0.5 + 0.5,
    ) * intensity + (1.0 - intensity);
}

fn radiant_eval_surface(material: GpuMaterial, material_tex: MaterialTextureData, input: MaterialInput) -> SurfaceData {
    let uv = input.tex_coords;
    let base_sample = sample_texture(material_tex.base_color, uv, vec4<f32>(1.0));
    let albedo = material.base_color * base_sample;
    let alpha = albedo.a;
    let N_geom = normalize(input.world_normal);

    var N: vec3<f32>;
    if (material.flags & MATERIAL_FLAG_HAS_NORMAL_MAP) != 0u && material_tex.normal.texture_index != MATERIAL_NO_TEXTURE {
        let T = normalize(input.world_tangent - dot(input.world_tangent, N_geom) * N_geom);
        let B = cross(N_geom, T) * input.bitangent_sign;
        var norm_ts = sample_texture(material_tex.normal, uv, vec4<f32>(0.5, 0.5, 1.0, 1.0)).rgb * 2.0 - 1.0;
        norm_ts = vec3<f32>(norm_ts.x * material_tex.params.x, norm_ts.y * material_tex.params.x, norm_ts.z);
        N = normalize(T * norm_ts.x + B * norm_ts.y + N_geom * norm_ts.z);
    } else {
        N = N_geom;
    }

    let orm_sample = sample_texture(material_tex.roughness_metallic, uv, vec4<f32>(1.0));
    let occlusion_sample = sample_texture(material_tex.occlusion, uv, vec4<f32>(1.0));
    let emissive_sample = sample_texture(material_tex.emissive, uv, vec4<f32>(1.0));

    var ao: f32 = 1.0 + (occlusion_sample.r - 1.0) * material_tex.params.y;
    var roughness: f32 = clamp(material.roughness_metallic.x * orm_sample.g, 0.045, 1.0);
    var metallic: f32 = clamp(material.roughness_metallic.y * orm_sample.b, 0.0, 1.0);
    var emissive: vec3<f32> = material.emissive.rgb * material.emissive.w * emissive_sample.rgb;

    let V = normalize(input.view_position - input.world_position);
    let NdotV = max(dot(N, V), 0.0);
    let film_freq = material.class_params.x;
    let film_intensity = material.class_params.y;
    let film_color = thin_film_color(NdotV, film_freq, film_intensity);

    let base_f0 = resolve_specular_f0(material, material_tex, albedo.rgb, metallic, uv);
    var specular_f0: vec3<f32> = mix(base_f0 * film_color, base_f0, 0.2);

    // RADIANT_OVERRIDE_SURFACE
    // RADIANT_OVERRIDE_END

    return SurfaceData(albedo, N, ao, roughness, metallic, specular_f0, emissive, alpha,
                       0u, vec3<f32>(0.0), 0.0, 0.0, 0.0, 0.0);
}
