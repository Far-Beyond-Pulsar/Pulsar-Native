fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    let anisotropy = material.class_params.x;
    let aniso_direction = material.class_params.y;

    let aniso_alpha = max(s.roughness * (1.0 - anisotropy), 0.001);
    let aniso_beta = max(s.roughness, 0.001);

    s.roughness_aniso_x = aniso_alpha;
    s.roughness_aniso_y = aniso_beta;
    s.aniso_rotation = aniso_direction;
    s.flags |= SURFACE_FLAG_ANISOTROPIC;

    // RADIANT_OVERRIDE_SURFACE
    // RADIANT_OVERRIDE_END

    return s;
}
