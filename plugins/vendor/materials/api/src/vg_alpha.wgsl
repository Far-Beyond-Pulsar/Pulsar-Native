// Alpha-mask evaluation of alpha-tested materials for the virtual-geometry
// raster tiers (hardware fragment, software compute, dense peel). It mirrors
// the classic G-buffer's early alpha test (`gbuffer.wgsl` `fs_main`):
//
//     alpha = material.base_color.a * sample(base_color slot).a
//     discard when alpha < cutoff        (and when alpha <= 0.001)
//
// including the explicit metadata table / single-row fallback rule of
// `material_texture_data` (row 0 `params.w == -1` selects the GpuMaterial
// texture ids with identity UVs and cutoff 0.5).
//
// The includer declares (see `helio_mats::vg_alpha::bindings_wgsl`):
//   vga_materials          array<u32>   GpuMaterial rows, 24 words each
//   vga_material_textures  array<u32>   MaterialTextureData rows, 88 words each
//   the texture table, as `vga_sample_grad` / `vga_sample_level` / `vga_tex_size`
//
// Texture sampling needs a uniform-control-flow free formulation (the table
// index is divergent): every sample is an explicit-gradient or explicit-level
// sample.

const VGA_MAT_WORDS: u32 = 24u;
const VGA_TEX_WORDS: u32 = 88u;
const VGA_NO_TEXTURE: u32 = 0xffffffffu;
const VGA_FLAG_DOUBLE_SIDED: u32 = 1u;
const VGA_FLAG_ALPHA_TEST: u32 = 4u;

struct VgaAlpha {
    // Material flags (0 for a missing row: treated as opaque).
    flags: u32,
    // Base-color texture table index, or VGA_NO_TEXTURE.
    tex: u32,
    // select_uv: offset.xy, scale.zw ; rotation sin, cos.
    offset_scale: vec4<f32>,
    rotation: vec4<f32>,
    // material.base_color.a
    factor: f32,
    cutoff: f32,
}

fn vga_load(material_id: u32) -> VgaAlpha {
    var a: VgaAlpha;
    a.flags = 0u;
    a.tex = VGA_NO_TEXTURE;
    a.offset_scale = vec4<f32>(0.0, 0.0, 1.0, 1.0);
    a.rotation = vec4<f32>(0.0, 1.0, 0.0, 0.0);
    a.factor = 1.0;
    a.cutoff = 0.0;
    let m = material_id * VGA_MAT_WORDS;
    if m + VGA_MAT_WORDS > arrayLength(&vga_materials) {
        return a;
    }
    a.flags = vga_materials[m + 18u];
    a.factor = bitcast<f32>(vga_materials[m + 3u]);
    let rows = arrayLength(&vga_material_textures) / VGA_TEX_WORDS;
    if rows > 0u && material_id < rows && bitcast<f32>(vga_material_textures[VGA_TEX_WORDS - 1u]) != -1.0 {
        let t = material_id * VGA_TEX_WORDS;
        a.tex = vga_material_textures[t];
        a.offset_scale = vec4<f32>(
            bitcast<f32>(vga_material_textures[t + 4u]), bitcast<f32>(vga_material_textures[t + 5u]),
            bitcast<f32>(vga_material_textures[t + 6u]), bitcast<f32>(vga_material_textures[t + 7u]));
        a.rotation = vec4<f32>(
            bitcast<f32>(vga_material_textures[t + 8u]), bitcast<f32>(vga_material_textures[t + 9u]),
            0.0, 0.0);
        a.cutoff = bitcast<f32>(vga_material_textures[t + 86u]);
    } else {
        a.tex = vga_materials[m + 12u];
        a.cutoff = select(0.0, 0.5, (a.flags & VGA_FLAG_ALPHA_TEST) != 0u);
    }
    return a;
}

fn vga_is_masked(a: VgaAlpha) -> bool {
    return (a.flags & VGA_FLAG_ALPHA_TEST) != 0u;
}

fn vga_double_sided(a: VgaAlpha) -> bool {
    return (a.flags & VGA_FLAG_DOUBLE_SIDED) != 0u;
}

// `select_uv` of the classic evaluator.
fn vga_uv(a: VgaAlpha, uv: vec2<f32>) -> vec2<f32> {
    let scaled = uv * a.offset_scale.zw;
    let s = a.rotation.x;
    let c = a.rotation.y;
    return vec2<f32>(scaled.x * c - scaled.y * s, scaled.x * s + scaled.y * c) + a.offset_scale.xy;
}

// The linear part of `vga_uv` (for derivatives).
fn vga_uv_linear(a: VgaAlpha, d: vec2<f32>) -> vec2<f32> {
    let scaled = d * a.offset_scale.zw;
    let s = a.rotation.x;
    let c = a.rotation.y;
    return vec2<f32>(scaled.x * c - scaled.y * s, scaled.x * s + scaled.y * c);
}

// Alpha with screen-space derivatives of the (untransformed) uv.
fn vga_alpha_grad(a: VgaAlpha, uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> f32 {
    var t = 1.0;
    if a.tex != VGA_NO_TEXTURE {
        t = vga_sample_grad(a.tex, vga_uv(a, uv), vga_uv_linear(a, ddx), vga_uv_linear(a, ddy)).a;
    }
    return a.factor * t;
}

// Alpha at an explicit mip level.
fn vga_alpha_level(a: VgaAlpha, uv: vec2<f32>, lod: f32) -> f32 {
    var t = 1.0;
    if a.tex != VGA_NO_TEXTURE {
        t = vga_sample_level(a.tex, vga_uv(a, uv), lod).a;
    }
    return a.factor * t;
}

// Mip level whose texel footprint matches `uv_area` (uv^2, untransformed) per
// `px_area` screen pixels^2. 0 when there is no texture.
fn vga_lod_for_area(a: VgaAlpha, uv_area: f32, px_area: f32) -> f32 {
    if a.tex == VGA_NO_TEXTURE { return 0.0; }
    let size = vga_tex_size(a.tex);
    let k = abs(a.offset_scale.z * a.offset_scale.w);
    let texels = uv_area * k * size.x * size.y;
    return clamp(0.5 * log2(max(texels, 1.0e-12) / max(px_area, 1.0e-6)), 0.0, 16.0);
}

fn vga_rejects(a: VgaAlpha, alpha: f32) -> bool {
    return alpha < a.cutoff || alpha <= 0.001;
}

