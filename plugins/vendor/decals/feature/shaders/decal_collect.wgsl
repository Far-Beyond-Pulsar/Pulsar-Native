enable wgpu_binding_array;
//!use helio_prelude

struct GpuDecal {
    transform: mat4x4<f32>, color: vec4<f32>,
    albedo_texture_index: u32, normal_texture_index: u32,
    roughness_texture_index: u32, metalness_texture_index: u32,
    blend_mode: u32, decal_type: u32,
    fade_time: f32, fade_start_delay: f32, age: f32,
    normal_adapt: u32, _pad0: f32, _pad1: f32,
}

const BT: u32 = 0u; const BA: u32 = 1u; const BADD: u32 = 2u; const BMUL: u32 = 3u;
const DT_AN: u32 = 0u; const DT_NO: u32 = 1u; const DT_EM: u32 = 2u; const DT_AL: u32 = 3u;

// Matches GpuMaterial::NO_TEXTURE — decal uses tint only.
const NO_TEXTURE: u32 = 0xFFFFFFFFu;

struct DecalGlobals { decal_count: u32, _pad0: u32, _pad1: u32, _pad2: u32 }

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform> globals: DecalGlobals;
@group(0) @binding(2) var<storage, read> decals: array<GpuDecal>;
// Hi-Z mip 0 is the graph's portable R32Float copy of scene depth. Keeping
// decal collection on a color texture avoids the unsupported depth-texture
// textureLoad lowering on WGPU's GL/GLES backend.
@group(0) @binding(3) var scene_depth: texture_2d<f32>;
@group(0) @binding(4) var gbuf_albedo: texture_2d<f32>;
@group(0) @binding(5) var gbuf_normal: texture_2d<f32>;
@group(0) @binding(6) var gbuf_orm: texture_2d<f32>;
@group(0) @binding(7) var gbuf_emissive: texture_2d<f32>;
@group(0) @binding(8) var temp_albedo: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(9) var temp_normal: texture_storage_2d<rgba16float, write>;
@group(0) @binding(10) var temp_orm: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(11) var temp_emissive: texture_storage_2d<rgba16float, write>;
// Scene-wide bindless texture table, shared with the GBuffer pass. Indexed by
// GpuDecal::albedo_texture_index, which is a texture slot (TextureId::slot()).
@group(1) @binding(0) var scene_textures: binding_array<texture_2d<f32>, 256>;
@group(1) @binding(1) var scene_samplers: binding_array<sampler, 256>;

// Compute shaders cannot use textureSample (no implicit derivatives), so the
// decal table is always sampled at mip 0.
fn sample_decal_texture(texture_index: u32, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(scene_textures[texture_index], scene_samplers[texture_index], uv, 0.0);
}

fn fade_opacity(age: f32, delay: f32, time: f32) -> f32 {
    if time <= 0.0 { return 1.0; }
    if age < delay { return 1.0; }
    return clamp(1.0 - (age - delay) / time, 0.0, 1.0);
}

fn blend_over(back: vec4<f32>, front: vec4<f32>) -> vec4<f32> {
    let a = clamp(front.a, 0.0, 1.0);
    return vec4<f32>(mix(back.rgb, front.rgb, a), back.a);
}
fn blend_add(back: vec4<f32>, front: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(back.rgb + front.rgb * front.a, back.a);
}
fn blend_mul(back: vec4<f32>, front: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(back.rgb * mix(vec3<f32>(1.0), front.rgb, front.a), back.a);
}
fn blend_nrm(back: vec3<f32>, front: vec3<f32>, a: f32) -> vec3<f32> {
    return normalize(mix(back, front, a));
}

/// Rotate a decal tangent-space normal into world space.
///
/// `transform` maps world -> decal space, so the *rows* of its upper 3x3 are the
/// decal's axes in world space (row i dotted with a world position gives decal
/// coordinate i). Rotating by the transpose therefore takes decal space back to
/// world. `m[c][r]` is column c, row r, hence the indexing below.
///
/// The projection axis (row 2) is deliberately not used to *replace* the surface
/// normal: its sign depends on how the caller built the transform, and a decal
/// with no normal map should leave the shading normal alone regardless.
fn decal_tangent_to_world(m: mat4x4<f32>, n: vec3<f32>) -> vec3<f32> {
    let tx = normalize(vec3<f32>(m[0][0], m[1][0], m[2][0]));
    let ty = normalize(vec3<f32>(m[0][1], m[1][1], m[2][1]));
    let tz = normalize(vec3<f32>(m[0][2], m[1][2], m[2][2]));
    return normalize(n.x * tx + n.y * ty + n.z * tz);
}

@compute @workgroup_size(16, 16, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pxl = vec2<i32>(id.xy);
    let sz = textureDimensions(gbuf_albedo);
    if id.x >= u32(sz.x) || id.y >= u32(sz.y) { return; }

    let depth = textureLoad(scene_depth, pxl, 0).x;
    if helio_is_sky(depth) { return; }

    let uv_scr = vec2<f32>((f32(pxl.x)+0.5)/f32(sz.x), (f32(pxl.y)+0.5)/f32(sz.y));
    let ndc = vec4<f32>(uv_scr.x*2.0-1.0, 1.0-uv_scr.y*2.0, depth, 1.0);
    let world_pos = (cameras[0].view_proj_inv * ndc).xyz / (cameras[0].view_proj_inv * ndc).w;

    let ea = textureLoad(gbuf_albedo, pxl, 0);
    let en = textureLoad(gbuf_normal, pxl, 0);
    let eo = textureLoad(gbuf_orm, pxl, 0);
    let ee = textureLoad(gbuf_emissive, pxl, 0);

    var ra = ea; var rn = en; var ro = eo; var re = ee;

    for (var i = 0u; i < globals.decal_count; i = i + 1u) {
        let d = decals[i];
        let da = fade_opacity(d.age, d.fade_start_delay, d.fade_time);
        if da <= 0.0 { continue; }

        let dl = d.transform * vec4<f32>(world_pos, 1.0);
        if abs(dl.x) > 1.0 || abs(dl.y) > 1.0 || dl.z < -1.0 || dl.z > 1.0 { continue; }

        let duv = vec2<f32>(dl.x*0.5+0.5, 0.5-dl.y*0.5);
        let duvc = clamp(duv, vec2<f32>(0.0), vec2<f32>(1.0));

        let tint = d.color;

        var decal_albedo = vec4<f32>(1.0);
        if d.albedo_texture_index != NO_TEXTURE {
            decal_albedo = sample_decal_texture(d.albedo_texture_index, duvc);
        }

        // The texture's alpha is the decal's shape mask, so it gates every
        // channel below (including emissive) rather than just albedo.
        let opa = tint.a * da * decal_albedo.a;
        if opa <= 0.0 { continue; }

        let wn = normalize(en.xyz);
        let dc = vec4<f32>(decal_albedo.rgb * tint.rgb, opa);

        // Only a decal that carries a normal map may perturb the shading normal.
        // Without one there is nothing to say about the surface's orientation, so
        // the G-buffer normal is left exactly as the geometry pass wrote it.
        let has_normal_map = d.normal_texture_index != NO_TEXTURE;
        var fnrm = wn;
        if has_normal_map {
            let tn = sample_decal_texture(d.normal_texture_index, duvc).xyz * 2.0 - 1.0;
            // normal_adapt re-anchors the perturbation to the surface the decal
            // landed on, rather than the decal's own projection frame.
            fnrm = select(
                decal_tangent_to_world(d.transform, tn),
                blend_nrm(wn, decal_tangent_to_world(d.transform, tn), 0.5),
                d.normal_adapt == 1u,
            );
        }

        if d.decal_type == DT_AN || d.decal_type == DT_AL {
            if d.blend_mode == BT || d.blend_mode == BA { ra = blend_over(ra, dc); }
            else if d.blend_mode == BADD { ra = blend_add(ra, dc); }
            else if d.blend_mode == BMUL { ra = blend_mul(ra, dc); }
            if has_normal_map { rn = vec4<f32>(blend_nrm(wn, fnrm, opa), en.a); }
        }
        if d.decal_type == DT_NO && has_normal_map {
            rn = vec4<f32>(blend_nrm(wn, fnrm, opa), en.a);
        }
        if d.decal_type == DT_EM || d.decal_type == DT_AL {
            // Weighted by the decal's own opacity: the surface's emissive
            // alpha says nothing about the decal (it gated non-emissive
            // surfaces out entirely).
            let de = vec4<f32>(tint.rgb, opa);
            re = select(blend_over(re, de), blend_add(re, de), d.blend_mode == BADD);
        }
        if d.decal_type == DT_AL && (d.blend_mode == BT || d.blend_mode == BA) {
            ro = vec4<f32>(mix(ro.r, 1.0, opa), mix(ro.g, 0.5, opa), mix(ro.b, 0.0, opa), ro.a);
        }
    }

    textureStore(temp_albedo, pxl, ra);
    textureStore(temp_normal, pxl, rn);
    textureStore(temp_orm, pxl, ro);
    textureStore(temp_emissive, pxl, re);
}
