//! Shared PBR/BRDF evaluation — `fresnel_schlick`, the distribution and geometry terms.
//!
//! Lives alongside [`crate::material`] and [`crate::radiant`], which already own the
//! CPU side of the material model and consume this text directly (manual
//! concatenation, not `helio-core`'s generic shader-snippet mechanism — see
//! `helio-core/src/shader/mod.rs`'s doc for why that mechanism only owns the
//! generic prelude and pass-declared snippets, never a specific domain's text).
//!
//! Registering a module here is the step that is easy to miss and gives no warning when
//! missed: the `//!use` marker is an ordinary WGSL comment, so an unregistered module
//! leaves the marker inert, the source passes through untouched, and the shader fails at
//! `create_shader_module` with "no definition in scope" for a function that plainly
//! exists. That is exactly how `deferred_lighting.wgsl` and `forward_lit.wgsl` came to be
//! uncompilable — which on this path takes out lighting entirely.
pub const PBR_EVAL: &str = include_str!("../shaders/pbr_eval.wgsl");

/// Marker opting a shader into [`PBR_EVAL`]. Must appear in the source.
pub const PBR_MARKER: &str = "//!use pbr_eval";

/// [`helio_core::shader::ShaderSnippet`] for [`PBR_EVAL`]. Real consumers
/// (gbuffer/forward-lit/deferred-light templates) currently splice
/// [`PBR_EVAL`] in by hand rather than through `helio_core::shader`'s
/// marker mechanism, but this is the snippet form for anything that wants
/// to go through `resolve_with`/`module_with` instead.
pub const PBR_EVAL_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!(PBR_MARKER, "../shaders/pbr_eval.wgsl");

/// Specular anti-aliasing (`normal_map_roughness`, `geometric_roughness`,
/// `specular_aa_roughness`): roughness widened by the normals a pixel
/// covers, read from normal maps whose mips keep their spread
/// ([`crate::texture_mips`]).
pub const SPECULAR_AA: &str = include_str!("../shaders/specular_aa.wgsl");

/// Marker opting a shader into [`SPECULAR_AA`]. Must appear in the source.
pub const SPECULAR_AA_MARKER: &str = "//!use specular_aa";

/// [`helio_core::shader::ShaderSnippet`] for [`SPECULAR_AA`]: every material
/// shader built from a Radiant template gets it
/// ([`crate::RadiantTemplate::build_shader_source`]).
pub const SPECULAR_AA_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!(SPECULAR_AA_MARKER, "../shaders/specular_aa.wgsl");

/// The material row in WGSL (`//!use material_row`): `GpuMaterial`, its
/// word layout for raw readers (`MATERIAL_WORDS`, `MATERIAL_FLAGS_WORD`,
/// `MATERIAL_CLASS_WORD`), `MATERIAL_NO_TEXTURE`, the `MATERIAL_FLAG_*`,
/// `MATERIAL_CLASS_*` and `MATERIAL_WORKFLOW_*` constants and
/// `material_stand_in()`, the grey material a missing row draws as. Every
/// kind reading the material table reads it through this.
pub const MATERIAL_ROW_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!("//!use material_row", "../shaders/material_row.wgsl");

/// Material evaluation in WGSL (`//!use material_surface`, with
/// `material_row` and `specular_aa`): the texture metadata
/// (`MaterialTextureSlot`/`MaterialTextureData`), `MaterialInput` (the point
/// a drawer hands a material), `SurfaceData` (what it gets back),
/// `material_at` (a row, or the stand-in), `material_texture_data`,
/// `sample_texture` and `default_pbr_surface`. The reader declares the
/// table's bindings as `materials`, `material_textures`, `scene_textures`
/// and `scene_samplers` (see the file's header).
pub const MATERIAL_SURFACE_SNIPPET: helio_core::shader::ShaderSnippet = helio_core::wgsl_snippet!(
    "//!use material_surface",
    "../shaders/material_surface.wgsl"
);

/// Every material snippet, in the order they are prepended: what a
/// drawer's material program resolves (`RadiantTemplate::compose` adds
/// them itself).
pub const MATERIAL_SNIPPETS: [helio_core::shader::ShaderSnippet; 4] = [
    MATERIAL_ROW_SNIPPET,
    SPECULAR_AA_SNIPPET,
    PBR_EVAL_SNIPPET,
    MATERIAL_SURFACE_SNIPPET,
];

/// Replace native material binding arrays with baseline-WebGPU bindings.
///
/// Browser WebGPU does not expose wgpu's `binding_array` WGSL extension. The
/// fixed bindings and switch preserve every material texture slot without
/// requiring a native-only feature. The native-only extension directive is
/// removed together with the binding-array declarations.
pub fn apply_webgpu_material_bindings(src: &str, max_textures: usize) -> String {
    let mut declarations = String::new();
    for index in 0..max_textures {
        declarations.push_str(&format!(
            "@group(1) @binding({}) var scene_texture_{index}: texture_2d<f32>;\n",
            2 + index,
        ));
    }
    for index in 0..max_textures {
        declarations.push_str(&format!(
            "@group(1) @binding({}) var scene_sampler_{index}: sampler;\n",
            2 + max_textures + index,
        ));
    }

    let mut source = String::with_capacity(src.len() + declarations.len());
    for line in src.lines() {
        if line.trim() == "enable wgpu_binding_array;" {
            // The rewritten shader no longer uses this native-only extension.
        } else if line.contains("scene_textures:") && line.contains("binding_array<texture_2d") {
            source.push_str(&declarations);
        } else if line.contains("scene_samplers:") && line.contains("binding_array<sampler") {
            // Both binding tables were emitted in place of scene_textures.
        } else {
            source.push_str(line);
            source.push('\n');
        }
    }

    let mut sample_switch = String::from("switch slot.texture_index {\n");
    for index in 0..max_textures {
        sample_switch.push_str(&format!(
            "        case {index}u: {{ return textureSampleLevel(scene_texture_{index}, scene_sampler_{index}, uv, 0.0); }}\n",
        ));
    }
    sample_switch.push_str("        default: { return fallback; }\n    }");

    let mut source = source.replace(
        "return textureSample(scene_textures[slot.texture_index], scene_samplers[slot.texture_index], uv);",
        &sample_switch,
    );
    // Blueprint graphs bind TextureSrc assets to fixed SceneDB texture slots
    // at compile time. Lower those constant indices to the same individual
    // bindings used by the WebGPU fallback path.
    for index in 0..max_textures {
        source = source
            .replace(
                &format!("scene_textures[{index}u]"),
                &format!("scene_texture_{index}"),
            )
            .replace(
                &format!("scene_samplers[{index}u]"),
                &format!("scene_sampler_{index}"),
            );
    }
    lower_graph_texture_samples(&source, max_textures)
}

/// Graph WGSL indexes the scene texture table through `GpuMaterial` fields.
/// Baseline WebGPU replaces that table with fixed bindings, so lower each
/// graph sampling expression to a switch over those bindings.
fn lower_graph_texture_samples(source: &str, max_textures: usize) -> String {
    const GRAPH_FIELDS: [&str; 5] = [
        "tex_base_color",
        "tex_normal",
        "tex_roughness",
        "tex_emissive",
        "tex_occlusion",
    ];

    let mut rewritten = String::with_capacity(source.len());
    let mut helpers = std::collections::BTreeSet::new();
    let mut cursor = 0;
    while cursor < source.len() {
        let next = [
            "textureSample(",
            "textureSampleLevel(",
            "textureSampleGrad(",
        ]
        .into_iter()
        .filter_map(|name| {
            source[cursor..]
                .find(name)
                .map(|offset| (cursor + offset, name))
        })
        .min_by_key(|(offset, _)| *offset);
        let Some((start, name)) = next else {
            rewritten.push_str(&source[cursor..]);
            break;
        };
        let Some((end, args)) = call_arguments(source, start + name.len() - 1) else {
            rewritten.push_str(&source[cursor..]);
            break;
        };
        let parts = split_wgsl_arguments(args);
        let texture_field = parts
            .first()
            .map(|texture| strip_outer_parentheses(texture))
            .and_then(|texture| texture.strip_prefix("scene_textures[material."))
            .and_then(|field| field.strip_suffix(']'));
        let sampler_field = parts
            .get(1)
            .map(|sampler| strip_outer_parentheses(sampler))
            .and_then(|sampler| sampler.strip_prefix("scene_samplers[material."))
            .and_then(|field| field.strip_suffix(']'));
        let variant = match name {
            "textureSample(" if parts.len() == 3 => Some(("pulsar_graph_sample", 1)),
            "textureSampleLevel(" if parts.len() == 4 => Some(("pulsar_graph_sample_level", 2)),
            "textureSampleGrad(" if parts.len() == 5 => Some(("pulsar_graph_sample_grad", 3)),
            _ => None,
        };
        if let (Some(texture_field), Some(sampler_field), Some((helper, helper_index))) =
            (texture_field, sampler_field, variant)
        {
            if texture_field == sampler_field
                && GRAPH_FIELDS.contains(&texture_field)
                && max_textures > 0
            {
                rewritten.push_str(&source[cursor..start]);
                let mut call = format!("{helper}(material.{texture_field}, {}", parts[2]);
                for arg in parts.iter().skip(3) {
                    call.push_str(", ");
                    call.push_str(arg);
                }
                call.push(')');
                rewritten.push_str(&call);
                helpers.insert(helper_index);
            } else {
                rewritten.push_str(&source[cursor..end]);
            }
        } else {
            rewritten.push_str(&source[cursor..end]);
        }
        cursor = end;
    }

    if helpers.is_empty() {
        return rewritten;
    }
    let mut generated = String::new();
    for helper in helpers {
        generated.push_str(&graph_sample_helper(helper, max_textures));
    }
    if let Some(position) = rewritten.find("fn radiant_eval_surface") {
        rewritten.insert_str(position, &generated);
    } else {
        rewritten.push_str(&generated);
    }
    rewritten
}

fn call_arguments(source: &str, open: usize) -> Option<(usize, &str)> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        match *byte {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((offset + 1, &source[open + 1..offset]));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_wgsl_arguments(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, byte) in args.bytes().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts
}

fn strip_outer_parentheses(mut expression: &str) -> &str {
    expression = expression.trim();
    while expression.starts_with('(') && expression.ends_with(')') {
        let mut depth = 0usize;
        let closes_at_end = expression.bytes().enumerate().all(|(index, byte)| {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && index + 1 < expression.len() {
                        return false;
                    }
                }
                _ => {}
            }
            true
        });
        if !closes_at_end {
            break;
        }
        expression = expression[1..expression.len() - 1].trim();
    }
    expression
}

fn graph_sample_helper(kind: usize, max_textures: usize) -> String {
    let (name, params, sample) = match kind {
        1 => (
            "pulsar_graph_sample",
            "slot: u32, uv: vec2<f32>",
            "textureSampleGrad(TEXTURE, SAMPLER, uv, dx, dy)",
        ),
        2 => (
            "pulsar_graph_sample_level",
            "slot: u32, uv: vec2<f32>, level: f32",
            "textureSampleLevel(TEXTURE, SAMPLER, uv, level)",
        ),
        _ => (
            "pulsar_graph_sample_grad",
            "slot: u32, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>",
            "textureSampleGrad(TEXTURE, SAMPLER, uv, dx, dy)",
        ),
    };
    let mut helper = format!("fn {name}({params}) -> vec4<f32> {{\n");
    if kind == 1 {
        helper.push_str("    let dx = dpdx(uv);\n    let dy = dpdy(uv);\n");
    }
    helper.push_str("    switch slot {\n");
    for index in 0..max_textures {
        helper.push_str(&format!(
            "        case {index}u: {{ return {}; }}\n",
            sample
                .replace("TEXTURE", &format!("scene_texture_{index}"))
                .replace("SAMPLER", &format!("scene_sampler_{index}"))
        ));
    }
    helper.push_str("        default: { return vec4<f32>(1.0); }\n    }\n}\n");
    helper
}

/// Replace the decal pass's scene binding arrays with baseline-WebGPU bindings.
///
/// Same idea as [`apply_webgpu_material_bindings`], but for the decal collect
/// compute shader: it indexes the table with a plain `u32` rather than a
/// material slot, samples with `textureSampleLevel` (compute has no implicit
/// derivatives), and owns group 1 outright so its tables start at binding 0.
pub fn apply_webgpu_decal_bindings(src: &str, max_textures: usize) -> String {
    let mut declarations = String::new();
    for index in 0..max_textures {
        declarations.push_str(&format!(
            "@group(1) @binding({index}) var scene_texture_{index}: texture_2d<f32>;\n",
        ));
    }
    for index in 0..max_textures {
        declarations.push_str(&format!(
            "@group(1) @binding({}) var scene_sampler_{index}: sampler;\n",
            max_textures + index,
        ));
    }

    let mut source = String::with_capacity(src.len() + declarations.len());
    for line in src.lines() {
        if line.contains("scene_textures:") && line.contains("binding_array<texture_2d") {
            source.push_str(&declarations);
        } else if line.contains("scene_samplers:") && line.contains("binding_array<sampler") {
            // Both binding tables were emitted in place of scene_textures.
        } else if line.trim() == "enable wgpu_binding_array;" {
            // wgpu-only extension; baseline WebGPU rejects the directive.
        } else {
            source.push_str(line);
            source.push('\n');
        }
    }

    let mut sample_switch = String::from("switch texture_index {\n");
    for index in 0..max_textures {
        sample_switch.push_str(&format!(
            "        case {index}u: {{ return textureSampleLevel(scene_texture_{index}, scene_sampler_{index}, uv, 0.0); }}\n",
        ));
    }
    sample_switch.push_str("        default: { return vec4<f32>(1.0); }\n    }");

    source.replace(
        "return textureSampleLevel(scene_textures[texture_index], scene_samplers[texture_index], uv, 0.0);",
        &sample_switch,
    )
}

#[cfg(test)]
mod tests {
    use super::{apply_webgpu_decal_bindings, apply_webgpu_material_bindings};

    #[test]
    fn expands_material_binding_arrays() {
        let source = r#"
enable wgpu_binding_array;
@group(1) @binding(2) var scene_textures: binding_array<texture_2d<f32>, 2>;
@group(1) @binding(3) var scene_samplers: binding_array<sampler, 2>;
fn sample_texture(slot: MaterialTextureSlot, uv: vec2<f32>, fallback: vec4<f32>) -> vec4<f32> {
    return textureSample(scene_textures[slot.texture_index], scene_samplers[slot.texture_index], uv);
}
"#;
        let fixed = apply_webgpu_material_bindings(source, 2);

        assert!(!fixed.contains("binding_array"));
        assert!(fixed.contains("@binding(2) var scene_texture_0"));
        assert!(fixed.contains("@binding(5) var scene_sampler_1"));
        assert!(fixed.contains("case 1u:"));
        assert!(fixed.contains("textureSampleLevel"));
    }

    #[test]
    fn lowers_graph_material_indices_for_expanded_bindings() {
        let source = r#"
@group(1) @binding(2) var scene_textures: binding_array<texture_2d<f32>, 2>;
@group(1) @binding(3) var scene_samplers: binding_array<sampler, 2>;
fn graph(material: GpuMaterial, uv: vec2<f32>) -> vec4<f32> {
    let a = textureSample((scene_textures[material.tex_base_color]), scene_samplers[material.tex_base_color], uv);
    let b = textureSampleLevel(scene_textures[material.tex_normal], scene_samplers[material.tex_normal], uv, 2.0);
    let c = textureSampleGrad(scene_textures[material.tex_roughness], scene_samplers[material.tex_roughness], uv, vec2<f32>(1.0), vec2<f32>(1.0));
    return a + b + c;
}
"#;
        let fixed = apply_webgpu_material_bindings(source, 2);

        assert!(!fixed.contains("binding_array"));
        assert!(!fixed.contains("scene_textures[material."));
        assert!(!fixed.contains("scene_samplers[material."));
        assert!(fixed.contains("pulsar_graph_sample(material.tex_base_color, uv)"));
        assert!(fixed.contains("let dx = dpdx(uv);"));
        assert!(fixed.contains("textureSampleGrad(scene_texture_0, scene_sampler_0, uv, dx, dy)"));
        assert!(fixed.contains("pulsar_graph_sample_level(material.tex_normal, uv, 2.0)"));
        assert!(fixed.contains(
            "pulsar_graph_sample_grad(material.tex_roughness, uv, vec2<f32>(1.0), vec2<f32>(1.0))"
        ));
    }

    #[test]
    fn expands_decal_binding_arrays() {
        let source = r#"
enable wgpu_binding_array;
@group(1) @binding(0) var scene_textures: binding_array<texture_2d<f32>, 256>;
@group(1) @binding(1) var scene_samplers: binding_array<sampler, 256>;
fn sample_decal_texture(texture_index: u32, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(scene_textures[texture_index], scene_samplers[texture_index], uv, 0.0);
}
"#;
        let fixed = apply_webgpu_decal_bindings(source, 2);

        // The extension directive and both tables must be gone — baseline
        // WebGPU rejects all three.
        assert!(!fixed.contains("binding_array"));
        assert!(!fixed.contains("enable wgpu_binding_array"));
        assert!(fixed.contains("@group(1) @binding(0) var scene_texture_0"));
        assert!(fixed.contains("@group(1) @binding(1) var scene_texture_1"));
        // Samplers follow the textures: base = max_textures.
        assert!(fixed.contains("@group(1) @binding(2) var scene_sampler_0"));
        assert!(fixed.contains("@group(1) @binding(3) var scene_sampler_1"));
        assert!(fixed.contains(
            "case 1u: { return textureSampleLevel(scene_texture_1, scene_sampler_1, uv, 0.0); }"
        ));
        assert!(fixed.contains("default: { return vec4<f32>(1.0); }"));
    }
}
