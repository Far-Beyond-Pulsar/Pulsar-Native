fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    let subsurface_color = material.class_params.xyz;
    let subsurface_radius = material.class_params.w;

    s.flags |= SURFACE_FLAG_SUBSURFACE;
    s.subsurface_color = subsurface_color;
    s.subsurface_radius = subsurface_radius;
    s.specular_f0 = mix(vec3<f32>(0.028), s.specular_f0, material.roughness_metallic.y);

    // RADIANT_OVERRIDE_SURFACE
    // RADIANT_OVERRIDE_END

    return s;
}
