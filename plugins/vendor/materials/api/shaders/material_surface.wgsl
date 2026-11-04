// Material evaluation in WGSL (helio-mats `MATERIAL_SURFACE_SNIPPET`,
// `//!use material_surface`): what a drawer of any kind calls to shade a
// point of its surface with a material. Use it with `//!use material_row`,
// `//!use specular_aa` and `//!use pbr_eval` (the `SURFACE_FLAG_*` a
// surface's `flags` carries to the lighting).
//
// The drawer hands the material a `MaterialInput` (`material_row`'s: the
// point as a material sees it: where it is, its frame, its texture
// coordinate, the view and the frame),
// and gets a `SurfaceData` back, which it writes to the G-buffer (or
// lights, forward). Templates (`radiant_eval_surface`, a material class's
// shading) and material graphs read only `MaterialInput`'s fields and the
// frame clock (`radiant_graph_time`, which a drawer whose `Globals` carries
// `time` supplies), so a material shades the same on a mesh, a terrain or a
// portal's view.
//
// The drawer declares the material table's bindings under these names
// (the WebGPU lowering, `apply_webgpu_material_bindings`, finds the texture
// and sampler arrays by them, so keep their declarations out of comments):
//   `materials`, an array of GpuMaterial (`helio_mats::MATERIALS`);
//   `material_textures`, an array of MaterialTextureData
//   (`MaterialTextureBindings::material_textures`);
//   `scene_textures` and `scene_samplers`, binding arrays of N 2D textures
//   and N samplers (`MaterialTextureBindings`).

// A material texture's slot in the table and its UV transform.
struct MaterialTextureSlot {
    texture_index: u32,
    uv_channel:    u32,
    _pad0:         u32,
    _pad1:         u32,
    offset_scale:  vec4<f32>,
    rotation:      vec4<f32>,
}

// Per-material texture metadata: seven 48-byte slots and 16-byte parameters
// (`helio_pass_materials::MATERIAL_TEXTURE_DATA_WORDS`).
struct MaterialTextureData {
    base_color:         MaterialTextureSlot,
    normal:             MaterialTextureSlot,
    roughness_metallic: MaterialTextureSlot,
    emissive:           MaterialTextureSlot,
    occlusion:          MaterialTextureSlot,
    specular_color:     MaterialTextureSlot,
    specular_weight:    MaterialTextureSlot,
    params:             vec4<f32>,  // x=normal_scale, y=occlusion_strength, z=alpha_cutoff
}

// What a material makes of a point: what the drawer writes.
struct SurfaceData {
    albedo:              vec4<f32>,
    normal:              vec3<f32>,
    ao:                  f32,
    roughness:           f32,
    metallic:            f32,
    specular_f0:         vec3<f32>,
    emissive:            vec3<f32>,
    alpha:               f32,
    flags:               u32,
    subsurface_color:    vec3<f32>,
    subsurface_radius:   f32,
    roughness_aniso_x:   f32,
    roughness_aniso_y:   f32,
    aniso_rotation:      f32,
}

// Material `id`'s row, or the stand-in where the table holds none (a
// material not loaded yet, or no material table: the materials absent).
fn material_at(id: u32) -> GpuMaterial {
    if id < arrayLength(&materials) {
        return materials[id];
    }
    return material_stand_in();
}

/// Select UV channel and apply texture transform
fn select_uv(slot: MaterialTextureSlot, base_uv: vec2<f32>) -> vec2<f32> {
    // TODO: support uv_channel when we have tex_coords1
    let scaled = base_uv * slot.offset_scale.zw;
    let s = slot.rotation.x;
    let c = slot.rotation.y;
    let rotated = vec2<f32>(
        scaled.x * c - scaled.y * s,
        scaled.x * s + scaled.y * c,
    );
    return rotated + slot.offset_scale.xy;
}

/// Sample texture from bindless array, or return fallback if MATERIAL_NO_TEXTURE
fn sample_texture(slot: MaterialTextureSlot, base_uv: vec2<f32>, fallback: vec4<f32>) -> vec4<f32> {
    if slot.texture_index == MATERIAL_NO_TEXTURE {
        return fallback;
    }
    let uv = select_uv(slot, base_uv);
    return textureSample(scene_textures[slot.texture_index], scene_samplers[slot.texture_index], uv);
}

// Basic SceneDB materials carry texture-store indices directly. An explicit
// metadata table can still supply transformed UVs and extension textures.
fn material_slot(index: u32) -> MaterialTextureSlot {
    return MaterialTextureSlot(index,0u,0u,0u,vec4<f32>(0.0,0.0,1.0,1.0),vec4<f32>(0.0,1.0,0.0,0.0));
}

// Material `id`'s texture slots: its metadata row, or (the table's sentinel,
// `params.w = -1`) its row's `tex_*` slots with identity UVs.
fn material_texture_data(material: GpuMaterial, id: u32) -> MaterialTextureData {
    if material_textures[0].params.w!=-1.0 && id<arrayLength(&material_textures) { return material_textures[id]; }
    return MaterialTextureData(material_slot(material.tex_base_color),material_slot(material.tex_normal),
        material_slot(material.tex_roughness),material_slot(material.tex_emissive),material_slot(material.tex_occlusion),
        material_slot(MATERIAL_NO_TEXTURE),material_slot(MATERIAL_NO_TEXTURE),vec4<f32>(1.0,1.0,select(0.0,0.5,(material.flags & MATERIAL_FLAG_ALPHA_TEST) != 0u),0.0));
}

fn resolve_specular_f0(
    material: GpuMaterial,
    material_tex: MaterialTextureData,
    albedo: vec3<f32>,
    metallic: f32,
    uv: vec2<f32>,
) -> vec3<f32> {
    if material.workflow == MATERIAL_WORKFLOW_SPECULAR {
        let specular_color = sample_texture(material_tex.specular_color, uv, vec4<f32>(1.0)).rgb;
        let specular_weight = sample_texture(material_tex.specular_weight, uv, vec4<f32>(1.0)).a;
        let ior = max(material.roughness_metallic.z, 1.0);
        let dielectric_f0 = pow((ior - 1.0) / (ior + 1.0), 2.0);
        return material.roughness_metallic.w * specular_weight * specular_color * dielectric_f0;
    }

    // Metallic workflow: F0 = mix(0.04, albedo, metallic)
    return clamp(
        mix(vec3<f32>(0.04), albedo, metallic),
        vec3<f32>(0.0),
        vec3<f32>(0.999),
    );
}

// ── Default PBR material evaluation ──────────────────────────────────────────

fn default_pbr_surface(material: GpuMaterial, material_tex: MaterialTextureData, input: MaterialInput) -> SurfaceData {
    let uv = input.tex_coords;
    let base_sample = sample_texture(material_tex.base_color, uv, vec4<f32>(1.0));
    let albedo = material.base_color * base_sample;
    let alpha = albedo.a;

    let N_geom = normalize(input.world_normal);

    var N: vec3<f32>;
    // The normal map's mean normal length (1: no spread, or no map).
    var normal_spread = 1.0;
    if (material.flags & MATERIAL_FLAG_HAS_NORMAL_MAP) != 0u && material_tex.normal.texture_index != MATERIAL_NO_TEXTURE {
        let T = normalize(input.world_tangent - dot(input.world_tangent, N_geom) * N_geom);
        let B = cross(N_geom, T) * input.bitangent_sign;
        let normal_sample = sample_texture(material_tex.normal, uv, vec4<f32>(0.5, 0.5, 1.0, 1.0));
        normal_spread = normal_sample.a;
        var norm_ts = normal_sample.rgb * 2.0 - 1.0;
        norm_ts = vec3<f32>(norm_ts.x * material_tex.params.x, norm_ts.y * material_tex.params.x, norm_ts.z);
        N = normalize(T * norm_ts.x + B * norm_ts.y + N_geom * norm_ts.z);
    } else {
        N = N_geom;
    }

    let orm_sample = sample_texture(material_tex.roughness_metallic, uv, vec4<f32>(1.0));
    let occlusion_sample = sample_texture(material_tex.occlusion, uv, vec4<f32>(1.0));
    let emissive_sample = sample_texture(material_tex.emissive, uv, vec4<f32>(1.0));

    var ao: f32 = 1.0 + (occlusion_sample.r - 1.0) * material_tex.params.y;
    var roughness: f32 = clamp(specular_aa_roughness(material.roughness_metallic.x * orm_sample.g, normal_spread, material_tex.params.x, N_geom), 0.045, 1.0);
    var metallic: f32 = clamp(material.roughness_metallic.y * orm_sample.b, 0.0, 1.0);
    var specular_f0: vec3<f32> = resolve_specular_f0(material, material_tex, albedo.rgb, metallic, uv);
    var emissive: vec3<f32> = material.emissive.rgb * material.emissive.w * emissive_sample.rgb;

    return SurfaceData(albedo, N, ao, roughness, metallic, specular_f0, emissive, alpha,
                       0u, vec3<f32>(0.0), 0.0, 0.0, 0.0, 0.0);
}

// Whether material `id`'s surface is cut out at `uv` (a cut-out material,
// or one whose metadata gives an alpha cutoff, whose alpha there is under
// the cutoff): what a depth or shadow drawer discards. Fragment stages only
// (implicit-derivative sampling).
fn material_cut_out(id: u32, uv: vec2<f32>) -> bool {
    let material = material_at(id);
    let tex = material_texture_data(material, id);
    if (material.flags & MATERIAL_FLAG_ALPHA_TEST) != 0u || tex.params.z > 0.0 {
        let alpha = material.base_color.a * sample_texture(tex.base_color, uv, vec4<f32>(1.0)).a;
        return alpha < tex.params.z;
    }
    return false;
}
