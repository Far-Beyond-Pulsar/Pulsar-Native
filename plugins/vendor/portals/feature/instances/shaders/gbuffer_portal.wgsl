enable wgpu_binding_array;
//!use helio_prelude
//!use portal_view
//!use specular_aa
//!use mesh_draw_rows
//!use pbr_eval
//!use material_row
//!use material_surface

//! Portal-duplicate G-buffer write — draws the instances `helio-pass-portal-cull`
//! selected, one duplicate per surviving (instance, chain) pair, mapped
//! through that chain's *composed* transform, clipped to every portal
//! along the chain.
//!
//! Fused into the same physical render pass `helio-pass-gbuffer` opened
//! (`LoadOp::Load` on all 8 attachments — see `helio-pass-foliage-gbuffer`
//! for the precedent this follows), so it shares the real depth buffer and
//! composes correctly with everything already drawn: no separate camera, no
//! compositing step. One `multi_draw_indexed_indirect` call, same shape as
//! the ordinary non-portal G-buffer pass — one draw per mesh/material draw
//! group, *not* one per chain; which chain each instance belongs to is
//! looked up per-instance from `portal_compacted_chains` (written by
//! `helio-pass-portal-cull`), not selected via a per-draw uniform.
//!
//! Vertex/material logic mirrors `helio-pass-gbuffer/shaders/gbuffer.wgsl`
//! closely (own copy — see that file for the fuller commentary on each
//! piece); the differences are: (1) composing the instance's own coordinate
//! space through an entire *chain* of portal spaces, deepest first, instead
//! of world space directly; (2) a depth-side and ray-through-aperture test at
//! every composed stage;
//! and (3) the `portal_mask`
//! screen-space gate (see below). Debug-visualization modes and lightmap
//! sampling are not reachable here. Materials are the materials' (the row,
//! the texture table and `default_pbr_surface` are their snippets): the
//! material's template (`radiant_eval_surface`) is composed after this
//! program per material key (`RadiantTemplate::compose`), as into the
//! G-buffer's, so a portal duplicate draws with the material the real
//! object does.
//!
//! # Why the depth-side tests and screen-space mask
//!
//! Each composed stage still rejects content on the camera-facing side of
//! its portal plane. X/Y aperture clipping is deliberately not repeated for
//! inner stages: that rectangle is a bound on the portal surface, not on the
//! dimensions of the entire target scene. Reapplying it to every mapped
//! fragment makes large wall geometry disappear while compact props survive.
//! The outer physical opening is instead enforced by the screen-space mask.
//!
//! `helio-pass-portal-mask` fixes this the standard way non-recursive
//! portal renderers do: it stamps the *outermost* portal's true on-screen
//! footprint (from the current camera, respecting real occluders) into
//! `portal_mask` before this pass runs. A fragment here is kept only when
//! the mask at its own screen pixel matches `chain.portals[0]` — i.e. only
//! where that chain's real, physical entry point is actually visible on
//! screen right now. Inner portals in the chain don't get their own mask
//! stamp (they're virtual — mapped, not physically where the camera can
//! look directly). Their exact projected aperture is evaluated per fragment
//! by intersecting the camera ray with each mapped portal plane.


struct ScreenSize {
    width:  f32,
    height: f32,
    _pad0:  f32,
    _pad1:  f32,
}

struct Globals {
    time: f32, // RADIANT_GLOBALS_TIME: host-driven frame clock
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

/// SceneDB's variable-length portal-chain field.
struct GpuPortalChainHandle {
    offset: u32,
    count:  u32,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform>       screen:  ScreenSize;
@group(0) @binding(13) var<uniform>      globals: Globals;
@group(0) @binding(2) var<storage, read> instance_data: array<GpuInstanceData>;
@group(0) @binding(3) var<storage, read> coordinate_spaces:      array<mat4x4<f32>>;
@group(0) @binding(4) var<storage, read> coordinate_spaces_prev: array<mat4x4<f32>>;
// Written by helio-pass-portal-cull: shared compacted original instance
// slots — group `g`'s draw call's `first_instance`/`instance_count` (from
// `portal_indirect`) already point the draw slot at exactly
// this group's region, so no per-draw offset math is needed here.
@group(0) @binding(5) var<storage, read> portal_compacted_indices: array<u32>;
@group(0) @binding(6) var<storage, read> portal_views: array<GpuPortalView>;
@group(0) @binding(7) var<storage, read> portal_chain_handles: array<GpuPortalChainHandle>;
// Parallel to portal_compacted_indices — which chain each compacted entry
// was selected under.
@group(0) @binding(8) var<storage, read> portal_compacted_chains: array<u32>;
// Written by helio-pass-portal-mask: per-pixel `portal_view_index + 1` where
// that portal's opening is actually visible on screen this frame, 0
// elsewhere. See the module doc above for why this is needed alongside the
// per-stage world-space clip below.
@group(0) @binding(9) var portal_mask: texture_2d<u32>;
@group(0) @binding(10) var<storage, read> portal_chain_portals: array<u32>;
struct SkinVertex { joints: vec4<u32>, weights: vec4<f32> }
@group(0) @binding(11) var<storage, read> skin_vertices: array<SkinVertex>;
@group(0) @binding(12) var<storage, read> bone_palette: array<mat4x4<f32>>;

@group(1) @binding(0) var<storage, read>    materials:          array<GpuMaterial>;
@group(1) @binding(1) var<storage, read>    material_textures:  array<MaterialTextureData>;
@group(1) @binding(2) var                   scene_textures:     binding_array<texture_2d<f32>, 256>;
@group(1) @binding(3) var                   scene_samplers:     binding_array<sampler, 256>;

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
    @location(0) world_position:     vec3<f32>,
    @location(1) world_normal:       vec3<f32>,
    @location(2) tex_coords:         vec2<f32>,
    @location(3) world_tangent:      vec3<f32>,
    @location(4) bitangent_sign:     f32,
    @location(5) @interpolate(flat) material_id: u32,
    @location(6) prev_clip_position: vec4<f32>,
    @location(7) @interpolate(flat) chain_idx: u32,
    @location(8) pre_portal_position: vec3<f32>,
    @location(9) lightmap_uv:        vec2<f32>,
}

fn decode_snorm8x4(packed: u32) -> vec3<f32> {
    return unpack4x8snorm(packed).xyz;
}

// `instance_index` is per-instance vertex data, which every backend offsets by the
// indirect draw's `first_instance` (`helio_mesh_api::draw_slots`).
@vertex
fn vs_main(v: Vertex, @location(15) instance_index: u32, @builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let slot_idx = portal_compacted_indices[instance_index];
    let chain_idx = portal_compacted_chains[instance_index];
    let inst = instance_data[slot_idx];
    let chain = portal_chain_handles[chain_idx];

    // Compose the instance's own coordinate space (identity for an ordinary
    // world-space object, or its sublevel's transform) through the whole
    // chain, deepest portal first — see the module doc for why. The fragment
    // shader reconstructs intermediate positions by undoing each outer map,
    // so the number of stages remains fully runtime-sized.
    let own_space_id  = instance_coordinate_space(inst.flags);
    let own_space      = coordinate_spaces[own_space_id];
    let own_space_prev = coordinate_spaces_prev[own_space_id];

    var local_position = v.position;
    var previous_local_position = v.position;
    var local_normal = decode_snorm8x4(v.normal);
    if inst.bone_palette_count > 0u {
        let skin = skin_vertices[vertex_index + inst.skin_vertex_offset];
        var current_blended = mat4x4<f32>(vec4f(0.0), vec4f(0.0), vec4f(0.0), vec4f(0.0));
        var previous_blended = mat4x4<f32>(vec4f(0.0), vec4f(0.0), vec4f(0.0), vec4f(0.0));
        var total = 0.0;
        for (var i = 0u; i < 4u; i = i + 1u) {
            if skin.weights[i] > 0.0 && skin.joints[i] < inst.bone_palette_count {
                current_blended += bone_palette[inst.bone_palette_offset + skin.joints[i]] * skin.weights[i];
                previous_blended += bone_palette[inst.bone_palette_offset + inst.bone_palette_count + skin.joints[i]] * skin.weights[i];
                total += skin.weights[i];
            }
        }
        if total > 0.00001 {
            current_blended = current_blended * (1.0 / total);
            previous_blended = previous_blended * (1.0 / total);
            local_position = (current_blended * vec4f(v.position, 1.0)).xyz;
            previous_local_position = (previous_blended * vec4f(v.position, 1.0)).xyz;
            local_normal = normalize(mat3x3<f32>(current_blended[0].xyz, current_blended[1].xyz, current_blended[2].xyz) * local_normal);
        }
    }
    var pos      = own_space * (inst.transform * vec4<f32>(local_position, 1.0));
    var pos_prev = own_space_prev * (inst.prev_model * vec4<f32>(previous_local_position, 1.0));
    let pre_portal_position = pos.xyz;
    var space_rot = mat3x3<f32>(own_space[0].xyz, own_space[1].xyz, own_space[2].xyz);

    var stage = chain.count;
    loop {
        if stage == 0u { break; }
        stage -= 1u;
        let portal_index = portal_chain_portals[chain.offset + stage];
        let p = portal_views[portal_index];
        let p_space = coordinate_spaces[p.coordinate_space];
        let p_space_prev = coordinate_spaces_prev[p.coordinate_space];
        pos = p_space * pos;
        pos_prev = p_space_prev * pos_prev;
        space_rot = mat3x3<f32>(p_space[0].xyz, p_space[1].xyz, p_space[2].xyz) * space_rot;
    }

    let world_pos = pos;

    let normal_mat = space_rot * mat3x3<f32>(
        inst.normal_mat_0.xyz,
        inst.normal_mat_1.xyz,
        inst.normal_mat_2.xyz,
    );
    let model_mat3 = space_rot * mat3x3<f32>(
        inst.transform[0].xyz,
        inst.transform[1].xyz,
        inst.transform[2].xyz,
    );

    let prev_clip = cameras[0].prev_view_proj * pos_prev;

    var out: VertexOutput;
    out.clip_position      = cameras[0].view_proj * world_pos;
    out.world_position     = world_pos.xyz;
    out.world_normal       = normalize(normal_mat * local_normal);
    out.world_tangent      = normalize(model_mat3 * decode_snorm8x4(v.tangent));
    out.bitangent_sign     = v.bitangent_sign;
    out.tex_coords         = v.tex_coords;
    out.material_id        = inst.material_id;
    out.prev_clip_position = prev_clip;
    out.chain_idx          = chain_idx;
    out.pre_portal_position = pre_portal_position;
    out.lightmap_uv        = GBUFFER_TAG_UNLIGHTMAPPED;
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
    @location(7) velocity:    vec4<f32>,
}

fn compute_velocity(input: VertexOutput) -> vec2<f32> {
    let prev_ndc = input.prev_clip_position.xy / input.prev_clip_position.w;
    let prev_pixel_x = (prev_ndc.x * 0.5 + 0.5) * screen.width;
    let prev_pixel_y = (0.5 - prev_ndc.y * 0.5) * screen.height;
    let prev_pixel = vec2<f32>(prev_pixel_x, prev_pixel_y);
    return input.clip_position.xy - prev_pixel;
}

// A portal aperture is a finite plane, not an infinite clipping tube. A
// target point is visible through it when the ray from the camera in that
// portal's current context intersects the portal plane inside its X/Y bounds.
// This perspective-correct projected aperture test is evaluated per fragment
// so object culling can remain conservative for large meshes.
fn clip_depth_only(local: vec4<f32>) -> bool {
    // Portal-view mapping places valid target contents on the positive-Z side
    // of the source portal plane. Negative-Z points are camera-facing and
    // must not leak into the portal image.
    return local.z < 0.0;
}

// Portal coordinate spaces are rigid placement maps. Inverting one here is
// cheaper and more portable than requiring a second GPU matrix table, while
// preserving the runtime-sized chain ABI.
fn inverse_rigid_point(transform: mat4x4<f32>, point: vec3<f32>) -> vec3<f32> {
    let delta = point - transform[3].xyz;
    return vec3<f32>(
        dot(delta, transform[0].xyz),
        dot(delta, transform[1].xyz),
        dot(delta, transform[2].xyz),
    );
}

// Return the camera position in the source context of `stage`. Outer portal
// maps have already been crossed when looking at a deeper portal, so undo
// those maps before testing that deeper aperture.
fn camera_before_stage(chain: GpuPortalChainHandle, stage: u32) -> vec3<f32> {
    var camera_position = cameras[0].position_near.xyz;
    var i = 0u;
    loop {
        if i >= stage { break; }
        let portal_index = portal_chain_portals[chain.offset + i];
        let p = portal_views[portal_index];
        camera_position = inverse_rigid_point(
            coordinate_spaces[p.coordinate_space],
            camera_position,
        );
        i += 1u;
    }
    return camera_position;
}

fn ray_hits_aperture(
    portal: GpuPortalView,
    camera_position: vec3<f32>,
    mapped_point: vec3<f32>,
) -> bool {
    let camera_local = (portal.inverse_transform * vec4<f32>(camera_position, 1.0)).xyz;
    let point_local = (portal.inverse_transform * vec4<f32>(mapped_point, 1.0)).xyz;

    // The camera must be on the source/front side and mapped content on the
    // target/back side of the portal plane. In Helio's inward-facing portal
    // convention those are negative and positive local Z respectively.
    if camera_local.z >= 0.0 || point_local.z < 0.0 {
        return false;
    }

    let direction = point_local - camera_local;
    if abs(direction.z) < 1e-5 {
        return false;
    }
    let plane_t = -camera_local.z / direction.z;
    if plane_t < 0.0 || plane_t > 1.0 {
        return false;
    }
    let hit = camera_local + direction * plane_t;
    return abs(hit.x) <= portal.half_extent.x
        && abs(hit.y) <= portal.half_extent.y;
}

@fragment
fn fs_main(input: VertexOutput) -> GBufferOutput {
    let chain = portal_chain_handles[input.chain_idx];
    let outer_portal = portal_chain_portals[chain.offset];

    // Screen-space gate: only draw where `helio-pass-portal-mask` determined
    // this chain's *outermost* portal is actually visible from the current
    // camera — see the module doc for why the per-stage world-space clips
    // below aren't enough on their own.
    let mask_px = vec2<i32>(input.clip_position.xy);
    let mask_value = textureLoad(portal_mask, mask_px, 0).r;
    if mask_value != outer_portal + 1u {
        discard;
    }

    // Re-run the same deepest-to-outer composition on the interpolated point
    // before the portal maps. At every stage, test the camera ray against the
    // corresponding finite portal plane. This gives the exact perspective
    // aperture for arbitrary room geometry without a fixed number of
    // recursion slots or a per-object rectangle approximation.
    var stage_pos = input.pre_portal_position;
    var stage = chain.count;
    loop {
        if stage == 0u { break; }
        stage -= 1u;
        let portal_index = portal_chain_portals[chain.offset + stage];
        let p = portal_views[portal_index];
        stage_pos = (coordinate_spaces[p.coordinate_space] * vec4<f32>(stage_pos, 1.0)).xyz;
        let local = p.inverse_transform * vec4<f32>(stage_pos, 1.0);
        if clip_depth_only(local)
            || !ray_hits_aperture(p, camera_before_stage(chain, stage), stage_pos)
        {
            discard;
        }
    }

    let material = material_at(input.material_id);
    let material_tex = material_texture_data(material,input.material_id);

    let surface = radiant_eval_surface(material, material_tex, MaterialInput(
        input.clip_position, input.world_position, input.world_normal,
        input.world_tangent, input.bitangent_sign, input.tex_coords,
        cameras[0].position_near.xyz, u32(cameras[0].jitter_frame.z),
    ));

    // Alpha test
    if surface.alpha <= 0.001 || surface.alpha < material_tex.params.z { discard; }

    var out: GBufferOutput;
    out.albedo = vec4<f32>(surface.albedo.rgb, surface.alpha);
    out.normal = vec4<f32>(surface.normal, surface.specular_f0.r);
    out.orm = vec4<f32>(surface.ao, surface.roughness, surface.metallic, surface.specular_f0.g);
    out.emissive = vec4<f32>(surface.emissive, surface.specular_f0.b);
    // x = -1 reads as "no lightmap"; y carries the chain so lighting can shade this
    // pixel in the content's own world (-3 - chain index, below the -1/-2 tags).
    out.lightmap_uv = gbuffer_tag_view_remap(input.chain_idx);
    out.sss = vec4<f32>(surface.subsurface_color, surface.subsurface_radius);
    out.extra = vec4<f32>(surface.roughness_aniso_x, surface.roughness_aniso_y,
                          surface.aniso_rotation, f32(surface.flags));
    // w: treated as moving by itself (helio-pass-tsr MOTION_VECTORS).
    out.velocity = vec4<f32>(compute_velocity(input),0.0,1.0);
    return out;
}
