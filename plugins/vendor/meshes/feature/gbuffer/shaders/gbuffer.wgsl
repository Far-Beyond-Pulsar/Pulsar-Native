enable wgpu_binding_array;
//!use specular_aa
//!use pbr_eval
//!use material_row
//!use material_surface
//!use mesh_draw_rows

//! The meshes' G-buffer program (GPU-driven): the meshes' vertex layout and
//! instance rows, shaded by the material's template. A material class's
//! template (`radiant_eval_surface`, `helio_mats::radiant`) and a material
//! graph are composed after it (`RadiantTemplate::compose`); the material
//! row, its texture table and `default_pbr_surface` are the materials'
//! snippets.
//!
//! Rasterises scene geometry into screen-sized textures:
//!   target 0 – albedo    (Rgba8Unorm)
//!   target 1 – normal    (Rgba16Float)
//!   target 2 – orm       (Rgba8Unorm)
//!   target 3 – emissive  (Rgba16Float)
//!   target 7 – velocity  (Rgba16Float) — screen-space motion in pixels/frame
//!                                          (helio-pass-tsr MOTION_VECTORS)
//!
//! Resolved F0 is packed into unused alpha channels:
//!   normal.a   = F0.r
//!   orm.a      = F0.g
//!   emissive.a = F0.b

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
    screen_width: f32,
    screen_height: f32,
    time: f32, // RADIANT_GLOBALS_TIME: seconds since start, for graph time nodes
}

struct SkinVertex {
    joints: vec4<u32>,
    weights: vec4<f32>,
}

/// Lightmap atlas region for a mesh (32 bytes).
///
/// uv_clamp_min/max are precomputed half-texel-inset bounds that prevent bilinear
/// filtering from bleeding across neighbouring atlas region boundaries at runtime.
struct LightmapAtlasRegion {
    uv_offset:    vec2<f32>,  // Top-left corner in atlas [0,1] space
    uv_scale:     vec2<f32>,  // Extent in atlas [0,1] space
    uv_clamp_min: vec2<f32>,  // uv_offset + 0.5/atlas_size  (half-texel inner inset)
    uv_clamp_max: vec2<f32>,  // uv_offset + uv_scale - 0.5/atlas_size
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform>          globals:                Globals;
@group(0) @binding(2) var<storage, read>    instance_data:          array<GpuInstanceData>;
@group(0) @binding(3) var<storage, read>    lightmap_atlas_regions: array<LightmapAtlasRegion>;
// Per-draw-call-group compacted original instance slots, surviving both
// frustum culling (IndirectDispatchPass) and Hi-Z occlusion culling
// (OcclusionCullPass). The draw slot (`@location(15)`) ranges over the
// group's now-possibly-smaller compacted count, so it must be redirected
// through this buffer before indexing `instance_data` — it no longer equals
// the instance's real slot directly.
@group(0) @binding(4) var<storage, read>    compacted_indices:      array<u32>;
// Coordinate-space transforms — see `libhelio::{coordinate_space, set_coordinate_space}`.
// Slot 0 is always identity, so an untagged instance (the common case) is
// unaffected beyond one extra constant-buffer read + mat4x4 multiply.
// Sublevels and portals both work by tagging instances/draws with a non-zero
// slot here and moving the whole space with a single matrix write.
@group(0) @binding(5) var<storage, read>    coordinate_spaces:      array<mat4x4<f32>>;
@group(0) @binding(6) var<storage, read>    coordinate_spaces_prev: array<mat4x4<f32>>;
@group(0) @binding(7) var<storage, read>    skin_vertices: array<SkinVertex>;
@group(0) @binding(8) var<storage, read>    bone_palette: array<mat4x4<f32>>;

@group(1) @binding(0) var<storage, read>    materials:          array<GpuMaterial>;
@group(1) @binding(1) var<storage, read>    material_textures:  array<MaterialTextureData>;
@group(1) @binding(2) var                   scene_textures:     binding_array<texture_2d<f32>, 256>;
@group(1) @binding(3) var                   scene_samplers:     binding_array<sampler, 256>;

@group(2) @binding(0) var uv_reference_texture: texture_2d<f32>;
@group(2) @binding(1) var uv_reference_sampler: sampler;

struct Vertex {
    @location(0) position:       vec3<f32>,
    @location(1) bitangent_sign: f32,
    @location(2) tex_coords:     vec2<f32>,  // UV0 — material/albedo channel (may tile)
    @location(3) normal:         u32,
    @location(4) tangent:        u32,
    @location(5) lightmap_uv:    vec2<f32>,  // UV1 — dedicated lightmap channel, non-overlapping [0,1]
}

struct VertexOutput {
    @invariant @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position:     vec3<f32>,
    @location(1) world_normal:       vec3<f32>,
    @location(2) tex_coords:         vec2<f32>,
    @location(3) world_tangent:      vec3<f32>,
    @location(4) bitangent_sign:     f32,
    @location(5) @interpolate(flat) material_id:    u32,
    @location(6) lightmap_uv:        vec2<f32>,  // Lightmap atlas UV (or (0,0) if no lightmap)
    @location(7) prev_clip_position: vec4<f32>,  // Previous frame clip-space position (velocity)
    // 1 when the instance moves by itself (its transform, skin or coordinate
    // space changed since last frame), else 0.
    @location(8) @interpolate(flat) moving: u32,
}

fn decode_snorm8x4(packed: u32) -> vec3<f32> {
    return unpack4x8snorm(packed).xyz;
}

// `slot` is per-instance vertex data, which every backend offsets by the
// indirect draw's `first_instance` (`helio_pass_gbuffer::draw_slots`).
@vertex
fn vs_main(v: Vertex, @location(15) slot: u32, @builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let inst       = instance_data[compacted_indices[slot]];
    var local_position = v.position;
    var local_normal = decode_snorm8x4(v.normal);
    var local_tangent = decode_snorm8x4(v.tangent);
    if inst.bone_palette_count > 0u {
        let skin = skin_vertices[vertex_index + inst.skin_vertex_offset];
        var blended = mat4x4<f32>(
            vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
        var total_weight = 0.0;
        for (var influence = 0u; influence < 4u; influence = influence + 1u) {
            let weight = skin.weights[influence];
            let joint = skin.joints[influence];
            if weight > 0.0 && joint < inst.bone_palette_count {
                blended += bone_palette[inst.bone_palette_offset + joint] * weight;
                total_weight += weight;
            }
        }
        if total_weight > 0.00001 {
            blended = blended * (1.0 / total_weight);
            local_position = (blended * vec4<f32>(v.position, 1.0)).xyz;
            let skin_normal = mat3x3<f32>(blended[0].xyz, blended[1].xyz, blended[2].xyz);
            local_normal = normalize(skin_normal * local_normal);
            local_tangent = normalize(skin_normal * local_tangent);
        }
    }

    // Coordinate space: world space (slot 0, identity) for the overwhelming
    // common case, or wherever a sublevel/portal has placed this instance.
    // See the `coordinate_spaces` binding above.
    let space_id   = instance_coordinate_space(inst.flags);
    let space      = coordinate_spaces[space_id];
    let space_prev = coordinate_spaces_prev[space_id];
    let space_rot  = mat3x3<f32>(space[0].xyz, space[1].xyz, space[2].xyz);

    let world_pos  = space * (inst.transform * vec4<f32>(local_position, 1.0));

    // Normals transform by the inverse-transpose (stored in normal_mat), then
    // by the coordinate space's rotation. Coordinate spaces are always rigid
    // (translation + rotation, never scaled), so `space_rot` is its own
    // inverse-transpose and needs no separate normal-matrix treatment.
    let normal_mat = space_rot * mat3x3<f32>(
        inst.normal_mat_0.xyz,
        inst.normal_mat_1.xyz,
        inst.normal_mat_2.xyz,
    );

    // Tangents are NOT normals — they transform by the regular upper-3×3 of
    // the model matrix (no inverse-transpose), then the same rigid rotation.
    let model_mat3 = space_rot * mat3x3<f32>(
        inst.transform[0].xyz,
        inst.transform[1].xyz,
        inst.transform[2].xyz,
    );

    // Previous-frame clip position for velocity buffer. Uses the coordinate
    // space's own previous-frame transform so a moving sublevel/portal still
    // produces correct motion vectors instead of a false one-frame pop.
    var prev_local_position = v.position;
    if inst.bone_palette_count > 0u {
        let skin = skin_vertices[vertex_index + inst.skin_vertex_offset];
        var prev_blended = mat4x4<f32>(
            vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
        var prev_total_weight = 0.0;
        for (var influence = 0u; influence < 4u; influence = influence + 1u) {
            let weight = skin.weights[influence];
            let joint = skin.joints[influence];
            if weight > 0.0 && joint < inst.bone_palette_count {
                prev_blended += bone_palette[inst.bone_palette_offset + inst.bone_palette_count + joint] * weight;
                prev_total_weight += weight;
            }
        }
        if prev_total_weight > 0.00001 {
            prev_local_position = (prev_blended * (1.0 / prev_total_weight) * vec4<f32>(v.position, 1.0)).xyz;
        }
    }
    let prev_world  = space_prev * (inst.prev_model * vec4<f32>(prev_local_position, 1.0));
    let prev_clip   = cameras[0].prev_view_proj * prev_world;

    var out: VertexOutput;
    // Keep projection separate: precombining P*V rounds away depth precision
    // needed by world-position reconstruction and ray-query self-hit offsets.
    // The depth prepass and forward/transparent variants must match this order.
    out.clip_position      = cameras[0].proj * (cameras[0].view * world_pos);
    out.world_position     = world_pos.xyz;
    out.world_normal       = normalize(normal_mat  * local_normal);
    out.world_tangent      = normalize(model_mat3  * local_tangent);
    out.bitangent_sign     = v.bitangent_sign;
    out.tex_coords         = v.tex_coords;
    out.material_id        = inst.material_id;
    out.prev_clip_position = prev_clip;
    let moved = any(inst.prev_model[0] != inst.transform[0]) || any(inst.prev_model[1] != inst.transform[1])
        || any(inst.prev_model[2] != inst.transform[2]) || any(inst.prev_model[3] != inst.transform[3])
        || any(space_prev[0] != space[0]) || any(space_prev[1] != space[1])
        || any(space_prev[2] != space[2]) || any(space_prev[3] != space[3]);
    out.moving = select(0u, 1u, moved || inst.bone_palette_count > 0u);
    
    // Compute lightmap UV from atlas region.
    //
    // UV CHANNEL SELECTION STRATEGY
    // ──────────────────────────────
    // If the mesh has a dedicated lightmap UV channel (UV1, non-zero), use it.
    // UV1 is artist-authored or tool-generated to be non-overlapping and in [0,1],
    // exactly what offline bakers need. Nebula receives UV1 explicitly via
    // `lightmap_uvs: Some(...)` in mesh_upload_to_bake when UV1 is non-trivial.
    //
    // If UV1 is all-zero (mesh has only one UV channel), fall back to UV0
    // clamped to [0,1].  UV0 is what Nebula baked with in that case
    // (mesh_upload_to_bake passes UV0 as lightmap_uvs when UV1 is absent).
    // Clamping prevents tiled UV0 values (e.g. 2.3) from mapping outside
    // the atlas region and hitting neighbouring meshes' texels (the original
    // "random dim slivers" bug).
    //
    // The computed atlas UV is then half-texel-inset clamped to [uv_clamp_min,
    // uv_clamp_max] to prevent bilinear filtering from bleeding across atlas
    // region boundaries regardless of which UV channel was chosen.
    let lightmap_idx = inst.lightmap_index;
    if lightmap_idx != 0xFFFFFFFFu {
        let region = lightmap_atlas_regions[lightmap_idx];
        // Use UV1 if any component is clearly non-zero; otherwise fall back to UV0.
        let use_uv1 = any(abs(v.lightmap_uv) > vec2<f32>(0.001));
        let lm_input = select(
            clamp(v.tex_coords, vec2<f32>(0.0), vec2<f32>(1.0)),  // UV0 path: clamp to [0,1]
            v.lightmap_uv,                                           // UV1 path: already in [0,1]
            use_uv1,
        );
        let raw_uv = region.uv_offset + lm_input * region.uv_scale;
        out.lightmap_uv = clamp(raw_uv, region.uv_clamp_min, region.uv_clamp_max);
    } else {
        // Sentinel: negative UV signals "no lightmap" to the deferred pass.
        // Cannot use (0,0) because a valid atlas region can start at (0,0).
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
    @location(7) velocity:    vec4<f32>,  // motion.xy, view-depth residual.z, flags.w (compute_velocity)
}

fn compute_velocity(input: VertexOutput) -> vec4<f32> {
    let prev_ndc = input.prev_clip_position.xy / input.prev_clip_position.w;
    let prev_pixel_x = (prev_ndc.x * 0.5 + 0.5) * globals.screen_width;
    let prev_pixel_y = (0.5 - prev_ndc.y * 0.5) * globals.screen_height;
    let prev_pixel = vec2<f32>(prev_pixel_x, prev_pixel_y);
    let p=cameras[0].proj;
    var residual=0.0;
    var valid=0.0;
    if abs(p[2].w)==1.0 && p[3].w==0.0 && p[0].y==0.0 && p[1].x==0.0
        && p[0].z==0.0 && p[1].z==0.0 && p[0].w==0.0 && p[1].w==0.0
        && p[3].x==0.0 && p[3].y==0.0 {
        let reconstructed_z=p[3].z/(p[2].w*input.clip_position.z-p[2].z);
        let actual_z=(cameras[0].view*vec4<f32>(input.world_position,1.0)).z;
        residual=actual_z-reconstructed_z;
        valid=select(0.0,2.0,abs(residual)<60000.0);
        residual=select(0.0,residual,valid==2.0);
    }
    // Store only the small reconstruction error in FP16, not the large view
    // depth itself. This preserves contact-scale precision at long distances.
    // w: 2 when the residual is valid, plus 1 when the instance moves by
    // itself (helio-pass-tsr MOTION_VECTORS: static geometry leaves its
    // camera motion to TSR, which derives it from depth at full precision).
    return vec4<f32>(input.clip_position.xy-prev_pixel,residual,valid+f32(input.moving));
}

@fragment
fn fs_main(vertex: VertexOutput, @builtin(front_facing) front: bool) -> GBufferOutput {
    var input = vertex;
    let material = material_at(input.material_id);
    let material_tex = material_texture_data(material,input.material_id);
    // Reject empty card pixels before normal/ORM/extension texture reads,
    // including debug views. Graph materials retain their post-evaluation test.
    if (material.flags & MATERIAL_FLAG_ALPHA_TEST) != 0u && (material.flags & MATERIAL_FLAG_HAS_CUSTOM_SHADER) == 0u && material.material_class == MATERIAL_CLASS_DEFAULT {
        let alpha = material.base_color.a * sample_texture(material_tex.base_color, input.tex_coords, vec4<f32>(1.0)).a;
        if alpha < material_tex.params.z { discard; }
    }
    if !front && (material.flags & MATERIAL_FLAG_DOUBLE_SIDED) != 0u {
        input.world_normal = -input.world_normal;
        input.bitangent_sign = -input.bitangent_sign;
    }

    // DEBUG MODE 1: Show the shared UV reference texture across UV0.
    if globals.debug_mode == 1u {
        let grid = textureSample(uv_reference_texture, uv_reference_sampler, input.tex_coords);
        return GBufferOutput(
            vec4<f32>(grid.rgb, 1.0),
            vec4<f32>(0.0, 0.0, 1.0, 0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0),
            vec2<f32>(0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0),
            compute_velocity(input)
        );
    }

    // DEBUG MODE 2: Show texture sample directly
    if globals.debug_mode == 2u {
        let base_sample = sample_texture(material_tex.base_color, input.tex_coords, vec4<f32>(1.0));
        return GBufferOutput(
            vec4<f32>(base_sample.rgb, 1.0),
            vec4<f32>(0.0, 0.0, 1.0, 0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0),
            vec2<f32>(0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0),
            compute_velocity(input)
        );
    }

    // DEBUG MODE 3: Geometry normals only (skip normal mapping)
    if globals.debug_mode == 3u {
        let uv = input.tex_coords;
        let base_sample = sample_texture(material_tex.base_color, uv, vec4<f32>(1.0));
        let albedo = material.base_color * base_sample;
        let N_geom = normalize(input.world_normal);
        let orm_sample = sample_texture(material_tex.roughness_metallic, uv, vec4<f32>(1.0));
        let roughness = clamp(material.roughness_metallic.x * orm_sample.g, 0.045, 1.0);
        let metallic = clamp(material.roughness_metallic.y * orm_sample.b, 0.0, 1.0);
        return GBufferOutput(
            vec4<f32>(albedo.rgb, albedo.a),
            vec4<f32>(N_geom, 0.0),
            vec4<f32>(1.0, roughness, metallic, 0.0),
            vec4<f32>(0.0),
            vec2<f32>(0.0),
            vec4<f32>(0.0),
            vec4<f32>(0.0),
            compute_velocity(input)
        );
    }

    let surface = radiant_eval_surface(material, material_tex, MaterialInput(
        input.clip_position, input.world_position, input.world_normal,
        input.world_tangent, input.bitangent_sign, input.tex_coords,
        cameras[0].position_near.xyz, globals.frame,
    ));

    // Alpha test
    if surface.alpha <= 0.001 { discard; }
    if surface.alpha < material_tex.params.z { discard; }

    var out: GBufferOutput;
    out.albedo = vec4<f32>(surface.albedo.rgb, surface.alpha);
    out.normal = vec4<f32>(surface.normal, surface.specular_f0.r);
    out.orm = vec4<f32>(surface.ao, surface.roughness, surface.metallic, surface.specular_f0.g);
    out.emissive = vec4<f32>(surface.emissive, surface.specular_f0.b);
    out.lightmap_uv = input.lightmap_uv;
    out.sss = vec4<f32>(surface.subsurface_color, surface.subsurface_radius);
    out.extra = vec4<f32>(surface.roughness_aniso_x, surface.roughness_aniso_y,
                          surface.aniso_rotation, f32(surface.flags));
    out.velocity = compute_velocity(input);
    return out;
}
