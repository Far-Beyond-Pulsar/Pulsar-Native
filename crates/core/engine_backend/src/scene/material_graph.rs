//! Lower compiled PSGC material graphs into Helio surface evaluation.

/// Only the output's dependency chain owns runtime texture dependencies.
pub(crate) fn texture_assets(graph: &psgc::GraphDescription) -> Result<Vec<String>, String> {
    let mut reachable = std::collections::HashSet::new();
    let mut pending: Vec<_> = graph
        .nodes
        .values()
        .filter(|n| n.node_type == "fragment_output")
        .map(|n| n.id.clone())
        .collect();
    while let Some(id) = pending.pop() {
        if reachable.insert(id.clone()) {
            pending.extend(
                graph
                    .connections
                    .iter()
                    .filter(|c| c.target_node == id)
                    .map(|c| c.source_node.clone()),
            );
        }
    }
    let mut assets = std::collections::BTreeSet::new();
    for node in graph
        .nodes
        .values()
        .filter(|n| reachable.contains(&n.id) && is_texture_sample(&n.node_type))
    {
        let asset = node
            .properties
            .get("texture")
            .and_then(serde_json::Value::as_str)
            .filter(|a| !a.trim().is_empty())
            .ok_or_else(|| format!("Texture sample '{}' has no texture asset selected", node.id))?;
        assets.insert(asset.to_string());
    }
    Ok(assets.into_iter().collect())
}

fn is_texture_sample(kind: &str) -> bool {
    matches!(
        kind,
        "sample_texture" | "sample_texture_level" | "sample_texture_grad"
    )
}

/// Bind asset properties before code generation. Replacing paths in generated
/// source corrupts overlapping filenames and paths that resemble WGSL names.
pub(crate) fn compile_material_graph(
    graph: &psgc::GraphDescription,
    texture_bindings: &std::collections::HashMap<String, u32>,
) -> Result<String, String> {
    for asset in texture_assets(graph)? {
        if !texture_bindings.contains_key(&asset) {
            return Err(format!("Texture asset '{asset}' has no scene binding"));
        }
    }
    let mut graph = graph.clone();
    for node in graph.nodes.values_mut() {
        if !is_texture_sample(&node.node_type) {
            continue;
        }
        if let Some(asset) = node
            .properties
            .get("texture")
            .and_then(serde_json::Value::as_str)
        {
            if let Some(slot) = texture_bindings.get(asset) {
                node.properties.insert(
                    "texture".into(),
                    serde_json::Value::String(format!("scene_textures[{slot}u]")),
                );
            }
        }
    }
    let generated = psgc::compile_shader(&graph)
        .map_err(|error| format!("shader graph compile failed: {error}"))?;
    adapt_graph_wgsl(&generated)
}

fn adapt_graph_wgsl(generated: &str) -> Result<String, String> {
    let mut source = generated.to_string();
    bind_graph_texture_samplers(&mut source);
    if let Some(start) = source.find("struct Uniforms {") {
        let end = source[start..]
            .find("};")
            .map(|offset| start + offset + 2)
            .ok_or_else(|| "malformed PSGC Uniforms declaration".to_string())?;
        source.replace_range(start..end, "");
    }
    source = source.replace("@group(0) @binding(0) var<uniform> uniforms: Uniforms;", "");
    source = source.replace("uniforms.time", "0.0");
    source = source.replace("FragmentOutput", "PulsarGraphOutput");
    for location in 0..8 {
        source = source.replace(&format!("@location({location}) "), "");
    }

    let entry = source
        .find("@fragment\nfn fragment_main(")
        .or_else(|| source.find("@fragment\r\nfn fragment_main("))
        .ok_or_else(|| "PSGC output is not a fragment shader".to_string())?;
    let open = source[entry..]
        .find('{')
        .map(|offset| entry + offset)
        .ok_or_else(|| "malformed PSGC fragment entry point".to_string())?;
    let replacement = "fn pulsar_material_graph(input: VertexOutput) -> PulsarGraphOutput {\n    let frag_coord = input.clip_position;\n    let uv = input.tex_coords;\n    let normal = input.world_normal;\n    let world_pos = input.world_position;";
    source.replace_range(entry..=open, replacement);

    let body = r#"let graph_surface = pulsar_material_graph(input);
albedo = graph_surface.base_color;
// Preserve the connected RGBA color's alpha. The separate opacity input
// modulates coverage and defaults to 1 when it is not connected.
alpha = clamp(graph_surface.base_color.a, 0.0, 1.0) * clamp(graph_surface.opacity, 0.0, 1.0);
albedo.a = alpha;
roughness = clamp(graph_surface.roughness, 0.045, 1.0);
metallic = clamp(graph_surface.metallic, 0.0, 1.0);
ao = clamp(graph_surface.ambient_occlusion, 0.0, 1.0);
emissive = graph_surface.emissive_color.rgb * graph_surface.emissive_color.a;
let graph_normal_length = length(graph_surface.normal);
if graph_normal_length > 0.0001 { N = graph_surface.normal / graph_normal_length; }
specular_f0 = clamp(mix(vec3<f32>(0.04), albedo.rgb, metallic), vec3<f32>(0.0), vec3<f32>(0.999));"#;
    Ok(format!(
        "/*RADIANT_GRAPH_DECLARATIONS*/\n{source}\n/*RADIANT_GRAPH_BODY*/\n{body}"
    ))
}

fn bind_graph_texture_samplers(source: &mut String) {
    for function in [
        "textureSample(",
        "textureSampleLevel(",
        "textureSampleGrad(",
    ] {
        let mut search_from = 0;
        while let Some(relative) = source[search_from..].find(function) {
            let call_start = search_from + relative;
            let args_start = call_start + function.len();
            let Some(first_comma_rel) = source[args_start..].find(',') else {
                break;
            };
            let first_comma = args_start + first_comma_rel;
            let first_arg = source[args_start..first_comma]
                .trim()
                .trim_matches(['(', ')'])
                .trim();
            let Some(slot_text) = first_arg
                .strip_prefix("scene_textures[")
                .and_then(|value| value.strip_suffix(']'))
            else {
                search_from = first_comma + 1;
                continue;
            };
            let Some(second_comma_rel) = source[first_comma + 1..].find(',') else {
                break;
            };
            let second_comma = first_comma + 1 + second_comma_rel;
            source.replace_range(
                first_comma + 1..second_comma,
                &format!(" scene_samplers[{slot_text}]"),
            );
            search_from = second_comma + 1;
        }
    }
}
