// The default material class (`MATERIAL_CLASS_DEFAULT`, the `default_pbr`
// template): the default PBR evaluation, with the point material graphs
// override its fields at. Every other template replaces this function.

// ── Radiant surface evaluation (tier-2 template entry point) ─────────────────
//
// NOTE: Individual local variables are kept in scope here (not `s.field`)
// for backward compatibility with graph-generated WGSL snippets that
// reference `emissive`, `roughness`, etc. as bare identifiers.
// Template overrides use `default_pbr_surface()` + `s.field` access.

fn radiant_eval_surface(material: GpuMaterial, material_tex: MaterialTextureData, input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    // Unpack for graph snippet access (local vars visible in the override block)
    var albedo: vec4<f32> = s.albedo;
    var N: vec3<f32> = s.normal;
    var ao: f32 = s.ao;
    var roughness: f32 = s.roughness;
    var metallic: f32 = s.metallic;
    var specular_f0: vec3<f32> = s.specular_f0;
    var emissive: vec3<f32> = s.emissive;
    var alpha: f32 = s.alpha;
    var surface_flags: u32 = s.flags;
    var subsurface_color: vec3<f32> = s.subsurface_color;
    var subsurface_radius: f32 = s.subsurface_radius;
    var roughness_aniso_x: f32 = s.roughness_aniso_x;
    var roughness_aniso_y: f32 = s.roughness_aniso_y;
    var aniso_rotation: f32 = s.aniso_rotation;

    // Radiant override point: graph-generated WGSL replaces this section to
    // override any SurfaceData field. When no graph is present the passthrough
    // below is used (the default PBR result).
    // RADIANT_OVERRIDE_SURFACE
    // RADIANT_OVERRIDE_END

    // Repack — fields that were not touched by the graph keep their default values.
    s.albedo = albedo;
    s.normal = N;
    s.ao = ao;
    s.roughness = roughness;
    s.metallic = metallic;
    s.specular_f0 = specular_f0;
    s.emissive = emissive;
    s.alpha = alpha;
    s.flags = surface_flags;
    s.subsurface_color = subsurface_color;
    s.subsurface_radius = subsurface_radius;
    s.roughness_aniso_x = roughness_aniso_x;
    s.roughness_aniso_y = roughness_aniso_y;
    s.aniso_rotation = aniso_rotation;

    return s;
}
