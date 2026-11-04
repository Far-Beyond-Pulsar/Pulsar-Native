// Two-sided foliage (`MATERIAL_CLASS_FOLIAGE`): thin, translucent leaves.
// Light from behind the surface passes through it, tinted by its colour
// (`SURFACE_FLAG_TWO_SIDED`, the transmitted share's colour in the
// G-buffer's subsurface target); `class_params.x` is the share of the light
// on one side that leaves the other (`material_transmission`), which what
// traces the light through the surface (shadows, GI) reads. A voxel
// terrain's leaves and a mesh's leaf cards shade the same.
fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    s.flags |= SURFACE_FLAG_TWO_SIDED;
    s.subsurface_color = s.albedo.rgb;

    // RADIANT_OVERRIDE_SURFACE
    // RADIANT_OVERRIDE_END

    return s;
}
