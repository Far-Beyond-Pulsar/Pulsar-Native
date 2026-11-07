//! Alpha-mask evaluation for virtual geometry (Nanite-style programmable
//! raster of masked materials).
//!
//! The visibility-buffer raster tiers do not run the material, but a masked
//! (alpha-tested) surface needs its coverage decided at raster time, otherwise
//! leaf and fence cards render as solid quads. [`WGSL`] is the one evaluator
//! every tier splices in: it reads the same `GpuMaterial` rows and
//! `MaterialTextureData` table as the classic G-buffer and applies the same
//! test (`base_color.a * texture.a < cutoff` discards).
//!
//! The material texture table is bound in either of its two modes
//! ([`MaterialBindingMode`]); [`bindings_wgsl`] declares the matching WGSL and
//! [`layout_entries`] / [`bind_entries`] the Rust side, so a tier only picks a
//! group and a first binding:
//!
//! | binding | contents |
//! |---|---|
//! | `first` | `vga_materials`: `GpuMaterial` rows as `array<u32>` (24 words) |
//! | `first + 1` | `vga_material_textures`: `MaterialTextureData` rows (88 words) |
//! | `first + 2 ..` | the texture table (`MaterialBindingConfig::append_layout_entries`) |

use crate::{
    material::{MaterialBindingConfig, MaterialBindingMode, MaterialTextureBindings},
    FLAG_ALPHA_TEST,
};

/// The evaluator (`vga_load`, `vga_alpha_grad`, `vga_alpha_level`, ...).
pub const WGSL: &str = include_str!("vg_alpha.wgsl");
/// `vga_uv0`: uv0 of a cluster vertex. Needs `helio_vg_format::DECODE_WGSL` first.
pub const UV_WGSL: &str = include_str!("vg_alpha_uv.wgsl");

/// Words of one `GpuMaterial` row.
pub const MATERIAL_WORDS: u32 = 24;
/// Words of one `MaterialTextureData` row.
pub const MATERIAL_TEXTURE_WORDS: u32 = 88;

/// Whether a material with these `GpuMaterial::flags` is rasterized through
/// the masked path. Blended materials never are (they cannot be virtual
/// geometry at all).
pub fn is_masked(flags: u32) -> bool {
    flags & FLAG_ALPHA_TEST != 0
}

/// `enable` directives the module needs; they must lead the module source.
pub fn module_header(binding: MaterialBindingConfig) -> &'static str {
    if binding.uses_binding_arrays() {
        "enable wgpu_binding_array;\n"
    } else {
        ""
    }
}

/// WGSL declaring the alpha bindings in `group` starting at `first_binding`
/// (see the module table) plus `vga_sample_grad`, `vga_sample_level` and
/// `vga_tex_size` over the texture table.
pub fn bindings_wgsl(binding: MaterialBindingConfig, group: u32, first: u32) -> String {
    use std::fmt::Write;
    let n = binding.max_textures;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "@group({group}) @binding({first}) var<storage, read> vga_materials: array<u32>;\n\
         @group({group}) @binding({}) var<storage, read> vga_material_textures: array<u32>;",
        first + 1
    );
    match binding.mode {
        MaterialBindingMode::BindingArray => {
            let _ = writeln!(
                s,
                "@group({group}) @binding({}) var vga_textures: binding_array<texture_2d<f32>, {n}>;\n\
                 @group({group}) @binding({}) var vga_samplers: binding_array<sampler, {n}>;\n\
                 fn vga_sample_grad(i: u32, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {{\n    \
                     let k = min(i, {}u);\n    \
                     return textureSampleGrad(vga_textures[k], vga_samplers[k], uv, dx, dy);\n}}\n\
                 fn vga_sample_level(i: u32, uv: vec2<f32>, lod: f32) -> vec4<f32> {{\n    \
                     let k = min(i, {}u);\n    \
                     return textureSampleLevel(vga_textures[k], vga_samplers[k], uv, lod);\n}}\n\
                 fn vga_tex_size(i: u32) -> vec2<f32> {{\n    \
                     return vec2<f32>(textureDimensions(vga_textures[min(i, {}u)], 0));\n}}",
                first + 2,
                first + 3,
                n - 1,
                n - 1,
                n - 1
            );
        }
        MaterialBindingMode::Expanded => {
            for i in 0..n {
                let _ = writeln!(
                    s,
                    "@group({group}) @binding({}) var vga_texture_{i}: texture_2d<f32>;\n\
                     @group({group}) @binding({}) var vga_sampler_{i}: sampler;",
                    first + 2 + i as u32,
                    first + 2 + (n + i) as u32
                );
            }
            let _ = writeln!(s, "fn vga_sample_grad(i: u32, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {{\n    switch i {{");
            for i in 0..n {
                let _ = writeln!(
                    s,
                    "        case {i}u: {{ return textureSampleGrad(vga_texture_{i}, vga_sampler_{i}, uv, dx, dy); }}"
                );
            }
            let _ = writeln!(
                s,
                "        default: {{ return vec4<f32>(1.0); }}\n    }}\n}}"
            );
            let _ = writeln!(s, "fn vga_sample_level(i: u32, uv: vec2<f32>, lod: f32) -> vec4<f32> {{\n    switch i {{");
            for i in 0..n {
                let _ = writeln!(
                    s,
                    "        case {i}u: {{ return textureSampleLevel(vga_texture_{i}, vga_sampler_{i}, uv, lod); }}"
                );
            }
            let _ = writeln!(
                s,
                "        default: {{ return vec4<f32>(1.0); }}\n    }}\n}}"
            );
            let _ = writeln!(
                s,
                "fn vga_tex_size(i: u32) -> vec2<f32> {{\n    switch i {{"
            );
            for i in 0..n {
                let _ = writeln!(
                    s,
                    "        case {i}u: {{ return vec2<f32>(textureDimensions(vga_texture_{i}, 0)); }}"
                );
            }
            let _ = writeln!(
                s,
                "        default: {{ return vec2<f32>(1.0); }}\n    }}\n}}"
            );
        }
    }
    s
}

fn storage_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Bind group layout entries matching [`bindings_wgsl`].
pub fn layout_entries(
    binding: MaterialBindingConfig,
    first: u32,
    visibility: wgpu::ShaderStages,
) -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries = vec![
        storage_entry(first, visibility),
        storage_entry(first + 1, visibility),
    ];
    binding.append_layout_entries(&mut entries, first + 2, visibility);
    entries
}

/// Bind group entries matching [`layout_entries`].
pub fn bind_entries<'a>(
    binding: MaterialBindingConfig,
    first: u32,
    materials: &'a wgpu::Buffer,
    textures: MaterialTextureBindings<'a>,
) -> Vec<wgpu::BindGroupEntry<'a>> {
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: first,
            resource: materials.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: first + 1,
            resource: textures.material_textures.as_entire_binding(),
        },
    ];
    binding.append_bind_group_entries(
        &mut entries,
        first + 2,
        textures.texture_views,
        textures.samplers,
    );
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(binding: MaterialBindingConfig) {
        let src = format!(
            "{}{}\n{}\n@compute @workgroup_size(1) fn main() {{\n    let a = vga_load(0u);\n    \
             let x = vga_alpha_grad(a, vec2<f32>(0.5), vec2<f32>(0.01), vec2<f32>(0.01)) + vga_alpha_level(a, vec2<f32>(0.5), 1.0) + vga_lod_for_area(a, 0.01, 4.0);\n    \
             if vga_rejects(a, x) && vga_is_masked(a) && vga_double_sided(a) {{ return; }}\n}}\n",
            module_header(binding),
            bindings_wgsl(binding, 0, 0),
            WGSL
        );
        let module = naga::front::wgsl::parse_str(&src)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn both_binding_modes_validate() {
        validate(MaterialBindingConfig {
            mode: MaterialBindingMode::BindingArray,
            max_textures: 256,
        });
        validate(MaterialBindingConfig {
            mode: MaterialBindingMode::Expanded,
            max_textures: 16,
        });
        validate(MaterialBindingConfig {
            mode: MaterialBindingMode::Expanded,
            max_textures: 3,
        });
    }

    /// The same measurement on the real tree leaf/twig alpha maps of the
    /// virtual-geometry stress scene (skipped when the assets are not on disk).
    /// Run with `--nocapture` for the table quoted in the plan.
    #[test]
    fn real_tree_alpha_coverage_by_mip() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/assets/vg_stress/trees");
        let mut measured = 0;
        for (tree, file) in [
            ("fir_tree_01", "fir_tree_01_twig_alpha_2k.png"),
            ("island_tree_01", "island_tree_01_leaves_alpha_2k.png"),
            ("tree_small_02", "tree_small_02_leaves_alpha_2k.png"),
        ] {
            let path = root.join(tree).join("textures").join(file);
            let Ok(img) = image::open(&path) else {
                continue;
            };
            let gray = img.to_luma8();
            let (w, h) = (gray.width() as usize, gray.height() as usize);
            let mut level: Vec<f32> = gray.pixels().map(|p| p.0[0] as f32 / 255.0).collect();
            let (mut lw, mut lh) = (w, h);
            let mut row = String::new();
            let base = level.iter().filter(|v| **v >= 0.5).count() as f32 / level.len() as f32;
            for mip in 0..=6 {
                let c = level.iter().filter(|v| **v >= 0.5).count() as f32 / level.len() as f32;
                row.push_str(&format!(" m{mip}={:.3}", c / base.max(1e-6)));
                let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
                let mut next = vec![0f32; nw * nh];
                for y in 0..nh {
                    for x in 0..nw {
                        let (x0, y0) = ((2 * x).min(lw - 1), (2 * y).min(lh - 1));
                        let (x1, y1) = ((2 * x + 1).min(lw - 1), (2 * y + 1).min(lh - 1));
                        next[y * nw + x] = (level[y0 * lw + x0]
                            + level[y0 * lw + x1]
                            + level[y1 * lw + x0]
                            + level[y1 * lw + x1])
                            / 4.0;
                    }
                }
                level = next;
                lw = nw;
                lh = nh;
            }
            eprintln!("{tree}: {w}x{h} base coverage {base:.3}; relative coverage kept at mip level:{row}");
            measured += 1;
        }
        if measured == 0 {
            eprintln!("real tree alpha maps not on disk; skipped");
        }
    }

    /// Coverage of an alpha-tested card as a function of the mip it is sampled
    /// at: documents why thin foliage thins out with distance (the classic path
    /// has the same behaviour; see the virtual-geometry plan).
    #[test]
    fn mip_coverage_of_thin_masks_decays() {
        // A leaf-like mask: thin diagonal strokes, 25% of the texels opaque.
        let n = 256usize;
        let mut alpha = vec![0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                if (x + y) % 8 < 2 {
                    alpha[y * n + x] = 1.0;
                }
            }
        }
        let coverage = |a: &[f32]| a.iter().filter(|v| **v >= 0.5).count() as f32 / a.len() as f32;
        let base = coverage(&alpha);
        let mut cur = alpha;
        let mut size = n;
        let mut last = base;
        for level in 1..=4 {
            let m = size / 2;
            let mut next = vec![0f32; m * m];
            for y in 0..m {
                for x in 0..m {
                    next[y * m + x] = (cur[2 * y * size + 2 * x]
                        + cur[2 * y * size + 2 * x + 1]
                        + cur[(2 * y + 1) * size + 2 * x]
                        + cur[(2 * y + 1) * size + 2 * x + 1])
                        / 4.0;
                }
            }
            cur = next;
            size = m;
            let c = coverage(&cur);
            assert!(
                c <= last + 1e-6,
                "coverage must not grow with mip {level}: {c} > {last}"
            );
            last = c;
        }
        assert!(
            last < base * 0.5,
            "mip 4 keeps {last} of {base}: thin masks lose coverage"
        );
    }
}
