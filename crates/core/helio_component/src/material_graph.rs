//! Lower compiled PSGC material graphs into Helio surface evaluation.
//!
//! Used by `StaticMeshDraw` to resolve a shader-graph material slot; public
//! so the engine's material-graph tests drive the same lowering.

/// Only the output's dependency chain owns runtime texture dependencies.
pub fn texture_assets(graph: &psgc::GraphDescription) -> Result<Vec<String>, String> {
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

/// How each texture the graph samples is used, by the asset path it names:
/// what the output pin it ends up feeding expects.
///
/// A texture feeding `base_color` or `emissive_color` is a colour; one feeding
/// `normal` is a normal map; one feeding `roughness`, `metallic`,
/// `ambient_occlusion`, `opacity` or `opacity_mask` is data, numbers that must
/// not be gamma-decoded. A texture that reaches several pins takes the
/// strongest use ([`crate::cooked_texture::TextureUse::strongest`]: colour
/// wins, which is how every texture was treated before uses existed). Only
/// samples the output depends on are listed, as in [`texture_assets`].
pub fn texture_uses(
    graph: &psgc::GraphDescription,
) -> std::collections::HashMap<String, crate::cooked_texture::TextureUse> {
    use crate::cooked_texture::TextureUse;

    fn pin_use(pin: &str) -> TextureUse {
        match pin {
            "normal" => TextureUse::Normal,
            "metallic" | "roughness" | "ambient_occlusion" | "opacity" | "opacity_mask" => {
                TextureUse::Data
            }
            // base_color, emissive_color, and anything unknown.
            _ => TextureUse::Color,
        }
    }

    let mut uses: std::collections::HashMap<String, TextureUse> = Default::default();
    for output in graph.nodes.values().filter(|n| n.node_type == "fragment_output") {
        for feed in graph.connections.iter().filter(|c| c.target_node == output.id) {
            let usage = pin_use(&feed.target_pin);
            let mut seen = std::collections::HashSet::new();
            let mut pending = vec![feed.source_node.clone()];
            while let Some(id) = pending.pop() {
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(node) = graph.nodes.get(&id) {
                    if is_texture_sample(&node.node_type) {
                        if let Some(asset) = node
                            .properties
                            .get("texture")
                            .and_then(serde_json::Value::as_str)
                            .filter(|a| !a.trim().is_empty())
                        {
                            uses.entry(asset.to_string())
                                .and_modify(|existing| *existing = existing.strongest(usage))
                                .or_insert(usage);
                        }
                    }
                }
                pending.extend(
                    graph
                        .connections
                        .iter()
                        .filter(|c| c.target_node == id)
                        .map(|c| c.source_node.clone()),
                );
            }
        }
    }
    uses
}

/// Bind graph textures through `GpuMaterial` fields before code generation.
/// The map values are graph-local texture ordinals, not scene texture slots,
/// so the compiled source and its pipeline key stay independent of residency.
pub fn compile_material_graph(
    graph: &psgc::GraphDescription,
    texture_bindings: &std::collections::HashMap<String, u32>,
) -> Result<String, String> {
    let assets = texture_assets(graph)?;
    if assets.len() > GRAPH_TEXTURE_FIELDS.len() {
        return Err(format!(
            "material graphs support at most {} sampled textures",
            GRAPH_TEXTURE_FIELDS.len()
        ));
    }
    for asset in &assets {
        if !texture_bindings.contains_key(asset) {
            return Err(format!(
                "Texture asset '{asset}' has no material texture binding"
            ));
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
            if let Some(ordinal) = texture_bindings.get(asset) {
                let Some(field) = GRAPH_TEXTURE_FIELDS.get(*ordinal as usize) else {
                    return Err(format!(
                        "texture asset '{asset}' has unsupported graph-local texture ordinal {ordinal}"
                    ));
                };
                node.properties.insert(
                    "texture".into(),
                    serde_json::Value::String(format!("scene_textures[material.{field}]")),
                );
            }
        }
    }
    let generated = psgc::compile_shader(&graph)
        .map_err(|error| format!("shader graph compile failed: {error}"))?;
    adapt_graph_wgsl(&generated)
}

/// The material row's existing texture fields provide scene-local indirection
/// for a graph's sampled textures.
pub const GRAPH_TEXTURE_FIELDS: [&str; 5] = [
    "tex_base_color",
    "tex_normal",
    "tex_roughness",
    "tex_emissive",
    "tex_occlusion",
];

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
    // Time comes from the host: Radiant defines `radiant_graph_time()` per template.
    source = source.replace("uniforms.time", "radiant_graph_time()");
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
    let replacement = "fn pulsar_material_graph(material: GpuMaterial, input: VertexOutput) -> PulsarGraphOutput {\n    let frag_coord = input.clip_position;\n    let uv = input.tex_coords;\n    let normal = input.world_normal;\n    let world_pos = input.world_position;";
    source = source.replace(
        "pulsar_material_graph(input)",
        "pulsar_material_graph(material, input)",
    );
    source.replace_range(entry..=open, replacement);

    let body = r#"let graph_surface = pulsar_material_graph(material, input);
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

#[cfg(test)]
mod usage_tests {
    use super::*;
    use crate::cooked_texture::TextureUse;
    use psgc::{Connection, ConnectionType, GraphDescription, NodeInstance, Position};

    fn node(graph: &mut GraphDescription, id: &str, kind: &str, texture: Option<&str>) {
        let mut node = NodeInstance::new(id, kind, Position { x: 0.0, y: 0.0 });
        if let Some(texture) = texture {
            node.properties.insert("texture".into(), serde_json::json!(texture));
        }
        graph.add_node(node);
    }

    fn connect(graph: &mut GraphDescription, from: &str, to: &str, pin: &str) {
        graph.add_connection(Connection::new(from, "result", to, pin, ConnectionType::Data));
    }

    #[test]
    fn each_texture_takes_the_use_of_the_pin_it_feeds_through_any_chain() {
        let mut graph = GraphDescription::new("uses");
        node(&mut graph, "out", "fragment_output", None);
        // base colour straight from a sample.
        node(&mut graph, "albedo", "sample_texture", Some("albedo.png"));
        connect(&mut graph, "albedo", "out", "base_color");
        // roughness through a channel pick.
        node(&mut graph, "orm", "sample_texture", Some("orm.png"));
        node(&mut graph, "green", "component_g", None);
        connect(&mut graph, "orm", "green", "vec");
        connect(&mut graph, "green", "out", "roughness");
        // normal.
        node(&mut graph, "nrm", "sample_texture", Some("normal.png"));
        connect(&mut graph, "nrm", "out", "normal");
        // a sample the output does not depend on is not listed.
        node(&mut graph, "unused", "sample_texture", Some("unused.png"));

        let uses = texture_uses(&graph);
        assert_eq!(uses["albedo.png"], TextureUse::Color);
        assert_eq!(uses["orm.png"], TextureUse::Data);
        assert_eq!(uses["normal.png"], TextureUse::Normal);
        assert!(!uses.contains_key("unused.png"));
    }

    #[test]
    fn a_texture_feeding_colour_and_data_stays_colour() {
        let mut graph = GraphDescription::new("shared");
        node(&mut graph, "out", "fragment_output", None);
        node(&mut graph, "tex", "sample_texture", Some("both.png"));
        node(&mut graph, "r", "component_r", None);
        connect(&mut graph, "tex", "out", "base_color");
        connect(&mut graph, "tex", "r", "vec");
        connect(&mut graph, "r", "out", "ambient_occlusion");
        assert_eq!(texture_uses(&graph)["both.png"], TextureUse::Color);
    }

    #[test]
    fn a_graph_with_no_samples_has_no_uses() {
        let mut graph = GraphDescription::new("empty");
        node(&mut graph, "out", "fragment_output", None);
        assert!(texture_uses(&graph).is_empty());
    }
}
