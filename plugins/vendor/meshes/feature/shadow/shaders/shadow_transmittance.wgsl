struct ShadowFace { mat:mat4x4f, atlas:vec4f, policy:vec4u }
// Coloured shadow transmittance — translucent casters (stained glass).
//
// Each atlas face gets an Rgba16Float layer storing the complement, so that
// zero (a cleared or never-written layer, or a fallback) means "no glass":
//   rgb = 1 - T, T the product of the transmittance of every translucent
//         surface in front of the nearest opaque occluder. Blending
//         src * (1 - dst) + dst composes 1 - T1 * T2 exactly.
//   a   = 1 - depth of the nearest translucent surface (max blend).
// Receivers deeper than that pane take their light filtered by T; receivers in
// front of the glass, or where no glass was drawn, are unfiltered.
//
// Glass hidden behind opaque static geometry is rejected against the static
// depth atlas, so a window deep inside a wall does not tint the courtyard in
// front of it.

struct GpuInstanceData {
    transform:    mat4x4<f32>,
    normal_mat_0: vec4<f32>,
    normal_mat_1: vec4<f32>,
    normal_mat_2: vec4<f32>,
    bounds:       vec4<f32>,
    prev_model:   mat4x4<f32>,
    mesh_id:      u32,
    material_id:  u32,
    flags:        u32,
    _pad:         u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}

struct FaceIndex {
    value: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

// Mirrors `helio_mats::GpuMaterial`.
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

struct TransmitParams {
    // Depth-atlas texels per transmittance texel.
    depth_scale: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<storage, read> shadow_matrices:   array<ShadowFace>;
@group(0) @binding(1) var<storage, read> instances:         array<GpuInstanceData>;
@group(0) @binding(2) var<uniform>       face:              FaceIndex;
@group(0) @binding(3) var<storage, read> coordinate_spaces: array<mat4x4<f32>>;

@group(1) @binding(0) var<storage, read> materials:     array<GpuMaterial>;
@group(1) @binding(1) var static_depth:                 texture_depth_2d_array;
@group(1) @binding(2) var<uniform>       params:        TransmitParams;

@group(0) @binding(4) var<storage,read> dirty:array<u32>;
struct SkinVertex { joints: vec4<u32>, weights: vec4<f32> }
@group(0) @binding(5) var<storage, read> skin_vertices: array<SkinVertex>;
@group(0) @binding(6) var<storage, read> bone_palette: array<mat4x4<f32>>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) material: u32,
}

@vertex
fn vs_main(
    @location(0)             position: vec3<f32>,
    @builtin(vertex_index)    vertex_index: u32,
    // Per-instance vertex data, offset by the indirect draw's `first_instance`
    // on every backend (`helio_pass_gbuffer::draw_slots`).
    @location(15)            slot:     u32,
) -> VsOut {
    let inst  = instances[slot];
    var local_position = position;
    if inst.bone_palette_count > 0u {
        let skin = skin_vertices[vertex_index + inst.skin_vertex_offset];
        var blended = mat4x4<f32>(vec4f(0.0), vec4f(0.0), vec4f(0.0), vec4f(0.0));
        var total = 0.0;
        for (var i = 0u; i < 4u; i = i + 1u) {
            if skin.weights[i] > 0.0 && skin.joints[i] < inst.bone_palette_count {
                blended += bone_palette[inst.bone_palette_offset + skin.joints[i]] * skin.weights[i];
                total += skin.weights[i];
            }
        }
        if total > 0.00001 { local_position = (blended * (1.0 / total) * vec4f(position, 1.0)).xyz; }
    }
    let space = coordinate_spaces[(inst.flags >> 8u) & 0xFFu];
    let world = space * (inst.transform * vec4<f32>(local_position, 1.0));
    var out: VsOut;
    out.pos = shadow_matrices[face.value].mat * world;
    if face._pad0 != 0u && (dirty[face.value] & face._pad0)==0u {out.pos=vec4f(2,2,2,1);}
    out.material = inst.material_id;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dims  = vec2<i32>(textureDimensions(static_depth));
    let texel = clamp(vec2<i32>(in.pos.xy * params.depth_scale), vec2<i32>(0), dims - 1);
    let opaque = textureLoad(static_depth, texel, 0, 0);
    if in.pos.z > opaque {
        discard;
    }
    // Beer-Lambert through tinted glass: base colour is the transmittance of a
    // reference sheet, alpha its optical thickness (a = 0.5 transmits c, a = 0
    // is clear). Matches the depth of colour of RT thin-sheet transmission far
    // better than the display blend mix(1, c, a), which reads as pastel.
    let m = materials[in.material];
    let c = clamp(m.base_color.rgb, vec3<f32>(0.02), vec3<f32>(1.0));
    let a = clamp(m.base_color.a, 0.0, 1.0);
    return vec4<f32>(1.0 - pow(c, vec3<f32>(2.0 * a)), 1.0 - in.pos.z);
}
