enable wgpu_binding_array;

// Radiant template: iridescent thin-film surface.
// Based on the default PBR template with thin-film interference added to F0.
// class_params.x = thin-film frequency, class_params.y = thin-film intensity.

struct Camera {
    view:           mat4x4<f32>,
    proj:           mat4x4<f32>,
    view_proj:      mat4x4<f32>,
    view_proj_inv:  mat4x4<f32>,
    position_near:  vec4<f32>,
    forward_far:    vec4<f32>,
    jitter_frame:   vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

struct Globals {
    frame: u32,
    delta_time: f32,
    light_count: u32,
    ambient_intensity: f32,
    ambient_color: vec4<f32>,
    csm_splits: vec4<f32>,
    debug_mode: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct GpuMaterial {
    base_color:         vec4<f32>,
    emissive:           vec4<f32>,
    roughness_metallic: vec4<f32>,
    tex_base_color:     u32,
    tex_normal:         u32,
    tex_roughness:      u32,
    tex_emissive:       u32,
    tex_occlusion:      u32,
    workflow:           u32,
    flags:              u32,
    material_class:     u32,
    class_params:       vec4<f32>,
}

struct MaterialTextureSlot {
    texture_index: u32,
    uv_channel:    u32,
    _pad0:         u32,
    _pad1:         u32,
    offset_scale:  vec4<f32>,
    rotation:      vec4<f32>,
}

struct MaterialTextureData {
    base_color:         MaterialTextureSlot,
    normal:             MaterialTextureSlot,
    roughness_metallic: MaterialTextureSlot,
    emissive:           MaterialTextureSlot,
    occlusion:          MaterialTextureSlot,
    specular_color:     MaterialTextureSlot,
    specular_weight:    MaterialTextureSlot,
    params:             vec4<f32>,
}

struct GpuInstanceData {
    transform:      mat4x4<f32>,
    normal_mat_0:   vec4<f32>,
    normal_mat_1:   vec4<f32>,
    normal_mat_2:   vec4<f32>,
    bounds:         vec4<f32>,
    prev_model:     mat4x4<f32>,
    mesh_id:        u32,
    material_id:    u32,
    flags:          u32,
    lightmap_index: u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}

struct LightmapAtlasRegion {
    uv_offset:    vec2<f32>,
    uv_scale:     vec2<f32>,
    uv_clamp_min: vec2<f32>,
    uv_clamp_max: vec2<f32>,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform>          globals:               Globals;
@group(0) @binding(2) var<storage, read>    instance_data:         array<GpuInstanceData>;
@group(0) @binding(3) var<storage, read>    lightmap_atlas_regions: array<LightmapAtlasRegion>;

@group(1) @binding(0) var<storage, read>    materials:         array<GpuMaterial>;
@group(1) @binding(1) var<storage, read>    material_textures: array<MaterialTextureData>;
@group(1) @binding(2) var                   scene_textures:    binding_array<texture_2d<f32>, 256>;
@group(1) @binding(3) var                   scene_samplers:    binding_array<sampler, 256>;

struct Vertex {
    @location(0) position:       vec3<f32>,
    @location(1) bitangent_sign: f32,
    @location(2) tex_coords:     vec2<f32>,
    @location(3) normal:         u32,
    @location(4) tangent:        u32,
    @location(5) lightmap_uv:    vec2<f32>,
}

struct VertexOutput {
    @invariant @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal:   vec3<f32>,
    @location(2) tex_coords:     vec2<f32>,
    @location(3) world_tangent:  vec3<f32>,
    @location(4) bitangent_sign: f32,
    @location(5) @interpolate(flat) material_id: u32,
    @location(6) lightmap_uv:    vec2<f32>,
}

fn decode_snorm8x4(packed: u32) -> vec3<f32> {
    return unpack4x8snorm(packed).xyz;
}

// `slot` is per-instance vertex data, which every backend offsets by the
// indirect draw's `first_instance` (`helio_pass_gbuffer::draw_slots`).
@vertex
fn vs_main(v: Vertex, @location(15) slot: u32) -> VertexOutput {
    let inst       = instance_data[slot];
    let world_pos  = inst.transform * vec4<f32>(v.position, 1.0);
    let normal_mat = mat3x3<f32>(inst.normal_mat_0.xyz, inst.normal_mat_1.xyz, inst.normal_mat_2.xyz);
    let model_mat3 = mat3x3<f32>(inst.transform[0].xyz, inst.transform[1].xyz, inst.transform[2].xyz);
    var out: VertexOutput;
    out.clip_position  = cameras[0].view_proj * world_pos;
    out.world_position = world_pos.xyz;
    out.world_normal   = normalize(normal_mat * decode_snorm8x4(v.normal));
    out.world_tangent  = normalize(model_mat3 * decode_snorm8x4(v.tangent));
    out.bitangent_sign = v.bitangent_sign;
    out.tex_coords     = v.tex_coords;
    out.material_id    = inst.material_id;
    let lightmap_idx = inst.lightmap_index;
    if lightmap_idx != 0xFFFFFFFFu {
        let region = lightmap_atlas_regions[lightmap_idx];
        let use_uv1 = any(abs(v.lightmap_uv) > vec2<f32>(0.001));
        let lm_input = select(clamp(v.tex_coords, vec2<f32>(0.0), vec2<f32>(1.0)), v.lightmap_uv, use_uv1);
        let raw_uv = region.uv_offset + lm_input * region.uv_scale;
        out.lightmap_uv = clamp(raw_uv, region.uv_clamp_min, region.uv_clamp_max);
    } else {
        out.lightmap_uv = vec2<f32>(-1.0, -1.0);
    }
    return out;
}

// ── Fragment ─────────────────────────────────────────────────────────────────

struct GBufferOutput {
    @location(0) albedo:      vec4<f32>,
    @location(1) normal:      vec4<f32>,
    @location(2) orm:         vec4<f32>,
    @location(3) emissive:    vec4<f32>,
    @location(4) lightmap_uv: vec2<f32>,
    @location(5) sss:         vec4<f32>,
    @location(6) extra:       vec4<f32>,
}

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

const NO_TEXTURE: u32 = 0xffffffffu;
const MATERIAL_WORKFLOW_METALLIC: u32 = 0u;
const MATERIAL_WORKFLOW_SPECULAR: u32 = 1u;
const FLAG_HAS_NORMAL_MAP: u32 = 1u << 3u;

fn select_uv(slot: MaterialTextureSlot, base_uv: vec2<f32>) -> vec2<f32> {
    let scaled = base_uv * slot.offset_scale.zw;
    let s = slot.rotation.x;
    let c = slot.rotation.y;
    let rotated = vec2<f32>(scaled.x * c - scaled.y * s, scaled.x * s + scaled.y * c);
    return rotated + slot.offset_scale.xy;
}

fn sample_texture(slot: MaterialTextureSlot, base_uv: vec2<f32>, fallback: vec4<f32>) -> vec4<f32> {
    if slot.texture_index == NO_TEXTURE { return fallback; }
    let uv = select_uv(slot, base_uv);
    return textureSample(scene_textures[slot.texture_index], scene_samplers[slot.texture_index], uv);
}

fn resolve_specular_f0(material: GpuMaterial, material_tex: MaterialTextureData, albedo: vec3<f32>, metallic: f32, uv: vec2<f32>) -> vec3<f32> {
    if material.workflow == MATERIAL_WORKFLOW_SPECULAR {
        let specular_color = sample_texture(material_tex.specular_color, uv, vec4<f32>(1.0)).rgb;
        let specular_weight = sample_texture(material_tex.specular_weight, uv, vec4<f32>(1.0)).a;
        let ior = max(material.roughness_metallic.z, 1.0);
        let dielectric_f0 = pow((ior - 1.0) / (ior + 1.0), 2.0);
        return material.roughness_metallic.w * specular_weight * specular_color * dielectric_f0;
    }
    return clamp(mix(vec3<f32>(0.04), albedo, metallic), vec3<f32>(0.0), vec3<f32>(0.999));
}

fn thin_film_color(cos_theta: f32, frequency: f32, intensity: f32) -> vec3<f32> {
    let path_diff = 2.0 * cos_theta;
    return vec3<f32>(
        sin(path_diff * frequency * 1.0) * 0.5 + 0.5,
        sin(path_diff * frequency * 1.1) * 0.5 + 0.5,
        sin(path_diff * frequency * 1.2) * 0.5 + 0.5,
    ) * intensity + (1.0 - intensity);
}

fn radiant_eval_surface(material: GpuMaterial, material_tex: MaterialTextureData, input: VertexOutput) -> SurfaceData {
    let uv = input.tex_coords;
    let base_sample = sample_texture(material_tex.base_color, uv, vec4<f32>(1.0));
    let albedo = material.base_color * base_sample;
    let alpha = albedo.a;
    let N_geom = normalize(input.world_normal);

    var N: vec3<f32>;
    if (material.flags & FLAG_HAS_NORMAL_MAP) != 0u && material_tex.normal.texture_index != NO_TEXTURE {
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

    let V = normalize(cameras[0].position_near.xyz - input.world_position);
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

@fragment
fn fs_main(input: VertexOutput) -> GBufferOutput {
    let material = materials[input.material_id];
    let material_tex = material_textures[input.material_id];

    if globals.debug_mode == 1u {
        let uv = input.tex_coords;
        return GBufferOutput(
            vec4<f32>(uv.x, uv.y, 0.0, 1.0),
            vec4<f32>(0.0, 0.0, 1.0, 0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0),
            vec2<f32>(0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0)
        );
    }

    if globals.debug_mode == 2u {
        let base_sample = sample_texture(material_tex.base_color, input.tex_coords, vec4<f32>(1.0));
        return GBufferOutput(
            vec4<f32>(base_sample.rgb, 1.0),
            vec4<f32>(0.0, 0.0, 1.0, 0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0),
            vec2<f32>(0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0)
        );
    }

    let surface = radiant_eval_surface(material, material_tex, input);
    if surface.alpha <= 0.001 { discard; }
    if surface.alpha < material_tex.params.z { discard; }

    var out: GBufferOutput;
    out.albedo = vec4<f32>(surface.albedo.rgb, surface.alpha);
    out.normal = vec4<f32>(surface.normal, surface.specular_f0.r);
    out.orm = vec4<f32>(surface.ao, surface.roughness, surface.metallic, surface.specular_f0.g);
    out.emissive = vec4<f32>(surface.emissive, surface.specular_f0.b);
    out.lightmap_uv = input.lightmap_uv;
    out.sss = vec4<f32>(surface.subsurface_color, surface.subsurface_radius);
    out.extra = vec4<f32>(surface.roughness_aniso_x, surface.roughness_aniso_y, surface.aniso_rotation, bitcast<f32>(surface.flags));
    return out;
}
