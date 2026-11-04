fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    let coat_strength = material.class_params.x;
    let coat_roughness = material.class_params.y;

    let coat_F0 = mix(vec3<f32>(0.04), s.specular_f0, coat_strength);
    s.specular_f0 = mix(s.specular_f0, coat_F0, coat_strength);
    s.roughness = mix(s.roughness, coat_roughness, coat_strength);

    // RADIANT_OVERRIDE_SURFACE
    // RADIANT_OVERRIDE_END

    return s;
}
