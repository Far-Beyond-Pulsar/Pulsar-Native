// Group 0 of every virtual-geometry raster kernel: the scene-side inputs.
// Prepended after `helio_vg_format::WGSL`; `vg_page_pool` / `vg_asset_quant`
// (bindings 1 and 2) are declared by `helio_vg_format::decode_bindings_wgsl`
// and consumed by the shared `helio_vg_format::DECODE_WGSL`. Binding
// visibility is VERTEX for the Tier 1 pipeline and COMPUTE for the others;
// the declarations are identical.
//
// Inputs (all produced by other crates):
//   vg_page_pool   helio-vg-stream page pool (PAGE_BYTES per slot)
//   vg_asset_quant helio-vg-stream `vg_asset_quant` (origin.xyz, step.w) by asset slot
//   vg_visible     helio-pass-vg-cull `visible_clusters`: {instance, cluster_slot}
//   vg_instances   `vg_instances` (GpuVgInstance rows)
//   vg_objects     scene join `static_objects` (252-byte / 63-word rows)

// Same layout as `helio_core::GpuCameraUniforms` (view 0 is used).
struct VgCamera {
    view:           mat4x4<f32>,
    proj:           mat4x4<f32>,
    view_proj:      mat4x4<f32>,
    view_proj_inv:  mat4x4<f32>,
    position_near:  vec4<f32>,
    forward_far:    vec4<f32>,
    jitter_frame:   vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

// Row of the scene join's `static_objects` buffer (`StaticObjectComponent`,
// 63 words = 252 bytes). Only the transform (words 4..20) is read here.
struct VgObject {
    head:      array<u32, 4>,
    transform: array<array<f32, 4>, 4>,
    rest:      array<u32, 43>,
}

struct VgRasterParams {
    width:        u32,
    height:       u32,
    tiles_x:      u32,
    tiles_y:      u32,
    // bit 0: cull back faces (software tiers; Tier 1 culls in the pipeline).
    flags:        u32,
    tile_bin_cap: u32,
    view_index:   u32,
    // Word offset of `{first, count}` of the software bin in `vg_sw_meta`.
    sw_meta_word: u32,
}

const VG_FLAG_CULL_BACK: u32 = 1u;
// Workgroups per row of the indirect software dispatch (cull `SW_GROUP_WIDTH`).
const VG_SW_ROW: u32 = 32768u;

@group(0) @binding(0) var<storage, read> vg_cameras:   array<VgCamera>;
// binding 1: vg_page_pool, binding 2: vg_asset_quant (decode_bindings_wgsl)
@group(0) @binding(3) var<storage, read> vg_visible:   array<VgVisibleCluster>;
@group(0) @binding(4) var<storage, read> vg_instances: array<VgInstance>;
@group(0) @binding(5) var<storage, read> vg_objects:   array<VgObject>;
@group(0) @binding(6) var<uniform>       vg_params:    VgRasterParams;

fn vg_object_transform(vis: VgVisibleCluster) -> mat4x4<f32> {
    let t = vg_objects[vg_instances[vis.instance].object_row].transform;
    return mat4x4<f32>(
        vec4<f32>(t[0][0], t[0][1], t[0][2], t[0][3]),
        vec4<f32>(t[1][0], t[1][1], t[1][2], t[1][3]),
        vec4<f32>(t[2][0], t[2][1], t[2][2], t[2][3]),
        vec4<f32>(t[3][0], t[3][1], t[3][2], t[3][3]),
    );
}

// Winding flips under a negative-determinant (mirrored) transform.
fn vg_is_mirrored(m: mat4x4<f32>) -> bool {
    return dot(cross(m[0].xyz, m[1].xyz), m[2].xyz) < 0.0;
}

// Asset-space position -> clip space.
fn vg_to_clip(m: mat4x4<f32>, p: vec3<f32>) -> vec4<f32> {
    return vg_cameras[vg_params.view_index].view_proj * (m * vec4<f32>(p, 1.0));
}

fn vg_asset_of(vis: VgVisibleCluster) -> u32 {
    return vg_instances[vis.instance].asset_slot;
}
