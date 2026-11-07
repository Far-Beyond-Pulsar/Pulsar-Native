fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    s.subsurface_color = material.class_params.xyz;
    s.subsurface_radius = material.class_params.w;
    s.flags |= SURFACE_FLAG_SUBSURFACE | SURFACE_FLAG_LOW_SPECULAR;
    s.specular_f0 = vec3<f32>(0.028);

    // RADIANT_OVERRIDE_SURFACE
    // RADIANT_OVERRIDE_END

    return s;
}
