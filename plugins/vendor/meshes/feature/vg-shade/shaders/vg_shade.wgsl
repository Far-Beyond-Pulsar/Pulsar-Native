// vg_shade: material evaluation of resolved virtual-geometry pixels, writing
// the classic G-buffer layout. This file is the MIDDLE of the shader module;
// `lib.rs::shade_base_source` splices, around it,
//   - the classic G-buffer shader's Camera/Globals/GpuMaterial/MaterialTexture*
//     declarations (before), and
//   - its SurfaceData, texture sampling, default_pbr_surface,
//     radiant_eval_surface, compute_velocity and material_texture_data
//     (after, unmodified apart from the gradient sampling substitution),
// so the material evaluator IS the classic one, template overrides included.

struct Params {
    width: u32,
    height: u32,
    flags: u32,
    bin_count: u32,
    pixel_count: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
}

// Same fields as the classic fragment input, minus the rasterizer builtin:
// `clip_position.xy` is the pixel centre in pixels and `.z` the resolved depth.
struct VertexOutput {
    clip_position:     vec4<f32>,
    world_position:    vec3<f32>,
    world_normal:      vec3<f32>,
    tex_coords:        vec2<f32>,
    world_tangent:     vec3<f32>,
    bitangent_sign:    f32,
    material_id:       u32,
    lightmap_uv:       vec2<f32>,
    prev_clip_position: vec4<f32>,
    // Moves by itself this frame (`compute_velocity`: the classic stage's
    // rule, its transform or its coordinate space changed).
    moving:            u32,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform>       globals: Globals;
@group(0) @binding(2) var<storage, read> static_objects: array<u32>;
@group(0) @binding(3) var<storage, read> coordinate_spaces: array<mat4x4<f32>>;
@group(0) @binding(4) var<storage, read> coordinate_spaces_prev: array<mat4x4<f32>>;
@group(0) @binding(5) var<storage, read> visible: array<VgVisibleCluster>;
// Shared decode inputs (helio_vg_format::DECODE_WGSL).
@group(0) @binding(6) var<storage, read> vg_asset_quant: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read> vg_instances: array<VgInstance>;
@group(0) @binding(8) var<storage, read> vg_page_pool: array<u32>;
@group(0) @binding(9) var<uniform>       params: Params;
@group(0) @binding(10) var<storage, read> pixel_list: array<u32>;
// Per bin: [offset, count, cursor, _].
@group(0) @binding(11) var<storage, read> bins: array<vec4<u32>>;
@group(0) @binding(12) var<storage, read> rt_visible: array<VgVisibleCluster>;

@group(1) @binding(0) var<storage, read>    materials:          array<GpuMaterial>;
@group(1) @binding(1) var<storage, read>    material_textures:  array<MaterialTextureData>;
@group(1) @binding(2) var                   scene_textures:     binding_array<texture_2d<f32>, 256>;
@group(1) @binding(3) var                   scene_samplers:     binding_array<sampler, 256>;

@group(2) @binding(0) var payload_tex: texture_2d<u32>;
@group(2) @binding(1) var resolved_depth: texture_2d<f32>;
@group(2) @binding(2) var out_albedo: texture_storage_2d<rgba8unorm, write>;
@group(2) @binding(3) var out_normal: texture_storage_2d<rgba16float, write>;
@group(2) @binding(4) var out_orm: texture_storage_2d<rgba8unorm, write>;
@group(2) @binding(5) var out_emissive: texture_storage_2d<rgba16float, write>;
@group(2) @binding(6) var out_sss: texture_storage_2d<rgba16float, write>;
@group(2) @binding(7) var out_extra: texture_storage_2d<rgba16float, write>;
@group(2) @binding(8) var out_velocity: texture_storage_2d<rgba16float, write>;

// Screen-space UV derivatives of the pixel being shaded (analytic, from the
// exact barycentrics of the neighbouring pixel centres). `sample_texture`
// reads them through `vg_slot_grad`: compute shaders have no implicit
// derivatives, so the sampling call is rewritten to `textureSampleGrad`.
var<private> vg_duv_dx: vec2<f32>;
var<private> vg_duv_dy: vec2<f32>;
// The same for the geometric normal (`specular_aa.wgsl`'s `dpdx(n)` /
// `dpdy(n)`, rewritten to these: its turn across the pixel).
var<private> vg_dn_dx: vec3<f32>;
var<private> vg_dn_dy: vec3<f32>;

fn vg_slot_grad(slot: MaterialTextureSlot, d: vec2<f32>) -> vec2<f32> {
    // The linear part of `select_uv` (scale, rotation); the offset drops out.
    let scaled = d * slot.offset_scale.zw;
    let s = slot.rotation.x;
    let c = slot.rotation.y;
    return vec2<f32>(scaled.x * c - scaled.y * s, scaled.x * s + scaled.y * c);
}

override BIN: u32 = 0u;

@compute @workgroup_size(64)
fn vg_shade(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let bin = bins[BIN];
    let local = (wg.y * 32768u + wg.x) * 64u + li;
    if local >= bin.y {
        return;
    }
    let pidx = pixel_list[bin.x + local];
    let px = vec2<u32>(pidx % params.width, pidx / params.width);
    let ipx = vec2<i32>(px);

    let payload = textureLoad(payload_tex, ipx, 0).xy;
    if payload.x == VGS_INVALID {
        return;
    }
    let uv_b = unpack2x16unorm(payload.y);
    let b = vec3<f32>(1.0 - uv_b.x - uv_b.y, uv_b.x, uv_b.y);

    let vis_index = vg_vis_cluster(payload.x);
    var vis: VgVisibleCluster;
    if vis_index >= VGS_RT_VISIBLE_BASE {
        vis = rt_visible[vis_index - VGS_RT_VISIBLE_BASE];
    } else {
        vis = visible[vis_index];
    }
    let cs = vis.cluster_slot;
    let cluster = vgd_load_cluster(cs);
    let inst = vg_instances[vis.instance];
    let row_word = (inst.object_row + (cluster.counts_material >> 16u)) * VGS_OBJ_WORDS;

    let blk = vgd_pos_block(cs, cluster, inst.asset_slot);
    let tri = vgd_tri(cs, cluster, vg_vis_tri(payload.x));
    let p0 = vgd_pos(blk, tri.x);
    let p1 = vgd_pos(blk, tri.y);
    let p2 = vgd_pos(blk, tri.z);
    let a0 = vgd_attr(cs, blk, tri.x);
    let a1 = vgd_attr(cs, blk, tri.y);
    let a2 = vgd_attr(cs, blk, tri.z);

    let cam = cameras[0];
    let space_id = vgs_space_id(row_word);
    let space = coordinate_spaces[space_id];
    let space_prev = coordinate_spaces_prev[space_id];
    let space_rot = mat3x3<f32>(space[0].xyz, space[1].xyz, space[2].xyz);
    let transform = vgs_obj_mat4(row_word + VGS_OBJ_TRANSFORM);
    let prev_model = vgs_obj_mat4(row_word + VGS_OBJ_PREV_TRANSFORM);

    let w0 = (space * (transform * vec4<f32>(p0, 1.0))).xyz;
    let w1 = (space * (transform * vec4<f32>(p1, 1.0))).xyz;
    let w2 = (space * (transform * vec4<f32>(p2, 1.0))).xyz;
    let c0 = cam.proj * (cam.view * vec4<f32>(w0, 1.0));
    let c1 = cam.proj * (cam.view * vec4<f32>(w1, 1.0));
    let c2 = cam.proj * (cam.view * vec4<f32>(w2, 1.0));

    // Previous-frame clip positions, exactly as the classic vertex stage
    // derives them (the coordinate space's own previous transform included).
    let pc0 = cam.prev_view_proj * (space_prev * (prev_model * vec4<f32>(p0, 1.0)));
    let pc1 = cam.prev_view_proj * (space_prev * (prev_model * vec4<f32>(p1, 1.0)));
    let pc2 = cam.prev_view_proj * (space_prev * (prev_model * vec4<f32>(p2, 1.0)));

    // Analytic UV derivatives from the exact barycentrics of the pixel and its
    // +x / +y neighbours (the pixel ray differentials).
    let size = vec2<f32>(f32(params.width), f32(params.height));
    let centre = vec2<f32>(px) + vec2<f32>(0.5);
    let bc = vgs_bary(c0, c1, c2, vgs_pixel_ndc(centre, size));
    let bx = vgs_bary(c0, c1, c2, vgs_pixel_ndc(centre + vec2<f32>(1.0, 0.0), size));
    let by = vgs_bary(c0, c1, c2, vgs_pixel_ndc(centre + vec2<f32>(0.0, 1.0), size));
    let uv_c = a0.uv0 * bc.x + a1.uv0 * bc.y + a2.uv0 * bc.z;
    vg_duv_dx = (a0.uv0 * bx.x + a1.uv0 * bx.y + a2.uv0 * bx.z) - uv_c;
    vg_duv_dy = (a0.uv0 * by.x + a1.uv0 * by.y + a2.uv0 * by.z) - uv_c;

    let nm0 = vgs_obj_vec4(row_word + VGS_OBJ_NORMAL_MAT);
    let nm1 = vgs_obj_vec4(row_word + VGS_OBJ_NORMAL_MAT + 4u);
    let nm2 = vgs_obj_vec4(row_word + VGS_OBJ_NORMAL_MAT + 8u);
    let normal_mat = space_rot * mat3x3<f32>(nm0.xyz, nm1.xyz, nm2.xyz);
    let model_mat3 = space_rot * mat3x3<f32>(transform[0].xyz, transform[1].xyz, transform[2].xyz);

    var input: VertexOutput;
    input.clip_position = vec4<f32>(centre, textureLoad(resolved_depth, ipx, 0).x, 1.0);
    input.world_position = w0 * b.x + w1 * b.y + w2 * b.z;
    input.world_normal = normalize(normal_mat * (a0.normal * b.x + a1.normal * b.y + a2.normal * b.z));
    let n_c = normalize(normal_mat * (a0.normal * bc.x + a1.normal * bc.y + a2.normal * bc.z));
    vg_dn_dx = normalize(normal_mat * (a0.normal * bx.x + a1.normal * bx.y + a2.normal * bx.z)) - n_c;
    vg_dn_dy = normalize(normal_mat * (a0.normal * by.x + a1.normal * by.y + a2.normal * by.z)) - n_c;
    input.world_tangent = normalize(model_mat3 * (a0.tangent * b.x + a1.tangent * b.y + a2.tangent * b.z));
    input.bitangent_sign = a0.bitangent_sign;
    input.tex_coords = a0.uv0 * b.x + a1.uv0 * b.y + a2.uv0 * b.z;
    input.material_id = static_objects[row_word + VGS_OBJ_MATERIAL_SLOT];
    input.lightmap_uv = vec2<f32>(-1.0, -1.0);
    input.prev_clip_position = pc0 * b.x + pc1 * b.y + pc2 * b.z;
    let moved = any(prev_model[0] != transform[0]) || any(prev_model[1] != transform[1])
        || any(prev_model[2] != transform[2]) || any(prev_model[3] != transform[3])
        || any(space_prev[0] != space[0]) || any(space_prev[1] != space[1])
        || any(space_prev[2] != space[2]) || any(space_prev[3] != space[3]);
    input.moving = select(0u, 1u, moved);

    let material = materials[input.material_id];
    let material_tex = material_texture_data(material, input.material_id);
    let surface = radiant_eval_surface(material, material_tex, input);

    // Debug views (cluster/triangle ids included) are written by `vg_debug.wgsl`
    // after this pass, only while one is selected.
    textureStore(out_albedo, px, vec4<f32>(surface.albedo.rgb, surface.alpha));
    textureStore(out_normal, px, vec4<f32>(surface.normal, surface.specular_f0.r));
    textureStore(out_orm, px, vec4<f32>(surface.ao, surface.roughness, surface.metallic, surface.specular_f0.g));
    textureStore(out_emissive, px, vec4<f32>(surface.emissive, surface.specular_f0.b));
    textureStore(out_sss, px, vec4<f32>(surface.subsurface_color, surface.subsurface_radius));
    textureStore(out_extra, px, vec4<f32>(surface.roughness_aniso_x, surface.roughness_aniso_y,
                                          surface.aniso_rotation, f32(surface.flags)));
    textureStore(out_velocity, px, compute_velocity(input));
}
