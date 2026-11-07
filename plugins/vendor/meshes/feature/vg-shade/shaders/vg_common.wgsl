// Shared helpers of the virtual-geometry shade modules. The includer declares
// `static_objects: array<u32>` (the scene join's 252-byte rows as words) and
// `coordinate_spaces: array<mat4x4<f32>>` before this text.

const VGS_OBJ_WORDS: u32 = 63u;     // 252-byte StaticObjectRow
const VGS_OBJ_MATERIAL_SLOT: u32 = 2u;
const VGS_OBJ_TRANSFORM: u32 = 4u;
const VGS_OBJ_PREV_TRANSFORM: u32 = 20u;
const VGS_OBJ_NORMAL_MAT: u32 = 36u;
const VGS_OBJ_FLAGS: u32 = 58u;
const VGS_INVALID: u32 = 0xffffffffu;
// Ids of the ray-traced backend start here (`helio_pass_vg_dense::DENSE_RT_VISIBLE_BASE`);
// they index `rt_visible`, not the cull's visible list.
const VGS_RT_VISIBLE_BASE: u32 = 16777216u;
// What produced a pixel's surface, recorded by `vis_resolve` from the candidate
// that actually won the pixel (`helio_vg_format::debug_view::pixel_source`).
const VGS_SRC_NONE: u32 = 0u;  // not virtual geometry (classic raster, voxel, sky)
const VGS_SRC_HW: u32 = 1u;    // Tier 1 hardware raster
const VGS_SRC_SW: u32 = 2u;    // Tier 2 compute micro-raster
const VGS_SRC_DENSE: u32 = 3u; // Tier 3 depth peel
const VGS_SRC_RT: u32 = 4u;    // Tier 3 ray query


fn vgs_obj_mat4(base: u32) -> mat4x4<f32> {
    return mat4x4<f32>(
        vec4<f32>(bitcast<f32>(static_objects[base]), bitcast<f32>(static_objects[base + 1u]),
                  bitcast<f32>(static_objects[base + 2u]), bitcast<f32>(static_objects[base + 3u])),
        vec4<f32>(bitcast<f32>(static_objects[base + 4u]), bitcast<f32>(static_objects[base + 5u]),
                  bitcast<f32>(static_objects[base + 6u]), bitcast<f32>(static_objects[base + 7u])),
        vec4<f32>(bitcast<f32>(static_objects[base + 8u]), bitcast<f32>(static_objects[base + 9u]),
                  bitcast<f32>(static_objects[base + 10u]), bitcast<f32>(static_objects[base + 11u])),
        vec4<f32>(bitcast<f32>(static_objects[base + 12u]), bitcast<f32>(static_objects[base + 13u]),
                  bitcast<f32>(static_objects[base + 14u]), bitcast<f32>(static_objects[base + 15u])),
    );
}

fn vgs_obj_vec4(base: u32) -> vec4<f32> {
    return vec4<f32>(bitcast<f32>(static_objects[base]), bitcast<f32>(static_objects[base + 1u]),
                     bitcast<f32>(static_objects[base + 2u]), bitcast<f32>(static_objects[base + 3u]));
}

fn vgs_space_id(row_word: u32) -> u32 {
    return (static_objects[row_word + VGS_OBJ_FLAGS] >> 8u) & 0xffu;
}

/// Asset-space point to world space (object transform, then coordinate space).
fn vgs_world_pos(row_word: u32, p: vec3<f32>) -> vec3<f32> {
    let m = vgs_obj_mat4(row_word + VGS_OBJ_TRANSFORM);
    let space = coordinate_spaces[vgs_space_id(row_word)];
    return (space * (m * vec4<f32>(p, 1.0))).xyz;
}

/// Exact perspective-correct barycentrics of the pixel whose NDC xy is `ndc`
/// in the (possibly near-plane-straddling) triangle with clip-space vertices
/// c0..c2. Solves `sum(b_i * (c_i.xy - ndc * c_i.w)) = 0` with `sum(b) = 1`:
/// the homogeneous weights ARE the perspective-correct ones, so no per-vertex
/// divide (and no clipping) is needed and the pixel ray is implicit.
fn vgs_bary(c0: vec4<f32>, c1: vec4<f32>, c2: vec4<f32>, ndc: vec2<f32>) -> vec3<f32> {
    let e0 = c0.xy - ndc * c0.w;
    let e1 = c1.xy - ndc * c1.w;
    let e2 = c2.xy - ndc * c2.w;
    let b = vec3<f32>(
        e1.x * e2.y - e2.x * e1.y,
        e2.x * e0.y - e0.x * e2.y,
        e0.x * e1.y - e1.x * e0.y,
    );
    let s = b.x + b.y + b.z;
    if abs(s) < 1e-30 {
        return vec3<f32>(1.0, 0.0, 0.0);
    }
    return b / s;
}

fn vgs_pixel_ndc(pixel: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(pixel.x / size.x * 2.0 - 1.0, 1.0 - pixel.y / size.y * 2.0);
}
