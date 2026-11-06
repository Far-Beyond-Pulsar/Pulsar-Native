//! Lowering a shader document for PSGC.
//!
//! The document is the Graphy graph the canvas edits; the compiler reads the same
//! type. What differs is only what the editor stores for convenience and PSGC
//! cannot take as it is: input defaults typed as text, blank defaults, and
//! `TextureSrc` asset references, which have to become WGSL texture bindings.
//! Everything here works on a copy, so compiling never changes the document.
//!
//! Compilation needs no editor state, so it can run on a worker thread.

use crate::definitions::NodeDefinitions;
use crate::types::{is_texture_previewable, type_name};
use graphy::{Connection, ConnectionType, GraphDescription, NodeInstance, PinType, Position};
use psgc::{compile_fragment_shader, DataType};

/// A copy of `graph` with its input defaults in the shape PSGC reads: numbers and
/// booleans typed as text become numbers and booleans, and a blank default of an
/// input pin is dropped so the pin falls back to the shader type's default
/// (serialized as `""` it would reach the WGSL as a string literal).
pub(crate) fn prepare(graph: &GraphDescription) -> GraphDescription {
    let mut prepared = graph.clone();
    for node in prepared.nodes.values_mut() {
        let inputs: Vec<String> = node.inputs.iter().map(|pin| pin.id.clone()).collect();
        let keys: Vec<String> = node.properties.keys().cloned().collect();
        for key in keys {
            let Some(serde_json::Value::String(text)) = node.properties.get(&key) else {
                continue;
            };
            match property_to_psgc_value(text, inputs.contains(&key)) {
                Some(value) => {
                    node.properties.insert(key, value);
                }
                None => {
                    node.properties.remove(&key);
                }
            }
        }
    }
    prepared
}

/// Compile a document to WGSL: lower reflected texture asset references into
/// concrete WGSL texture bindings (the asset path stays on the graph pin; the
/// generated WGSL binds that pin to a stable per-graph resource name), then run
/// PSGC.
pub(crate) fn compile_graph_to_wgsl(document: &GraphDescription) -> Result<String, String> {
    let mut graph = prepare(document);
    let bindings = lower_texture_sources(&mut graph)?;
    let source = psgc::compile_shader(&graph)
        .map_err(|error| format!("WGSL compilation failed: {error}"))?;
    Ok(format!("{bindings}{source}"))
}

/// The display name of a node: its definition's name, else its type.
pub(crate) fn node_title(node: &NodeInstance) -> String {
    NodeDefinitions::load()
        .get_node_definition(&node.node_type)
        .map_or_else(|| node.node_type.replace('_', " "), |d| d.name.clone())
}

/// A fragment shader that shows output `pin_id` of `node_id`, for the pin preview.
pub(crate) fn compile_preview_wgsl_for_pin(
    document: &GraphDescription,
    node_id: &str,
    pin_id: &str,
) -> Result<String, String> {
    let node = document
        .nodes
        .get(node_id)
        .ok_or_else(|| format!("Preview node '{}' not found", node_id))?;
    let pin = node
        .outputs
        .iter()
        .find(|pin| pin.id == pin_id)
        .ok_or_else(|| format!("Preview pin '{}' not found on node '{}'", pin_id, node_id))?;

    if !is_texture_previewable(pin.data_type()) {
        return Err(format!(
            "Pin '{}.{}' is not texture-previewable ({})",
            node_id,
            pin_id,
            type_name(pin.data_type())
        ));
    }

    let mut graph_desc = prepare(document);
    let texture_bindings = lower_texture_sources(&mut graph_desc)?;
    graph_desc.nodes.retain(|_, node| {
        node.node_type != "fragment_output" && node.node_type != "vertex_output"
    });
    graph_desc.connections.retain(|connection| {
        graph_desc.nodes.contains_key(&connection.source_node)
            && graph_desc.nodes.contains_key(&connection.target_node)
    });

    let fragment_output = NodeDefinitions::load()
        .get_node_definition("fragment_output")
        .ok_or_else(|| "Missing fragment_output definition".to_string())?;
    let color_input = fragment_output
        .inputs
        .iter()
        .find(|input| input.id == "color")
        .or_else(|| fragment_output.inputs.first())
        .ok_or_else(|| "fragment_output has no inputs".to_string())?;

    let preview_output_id = "__pin_preview_output__".to_string();
    let mut output_node = NodeInstance::new(
        preview_output_id.clone(),
        "fragment_output",
        Position { x: 0.0, y: 0.0 },
    );
    for input in &fragment_output.inputs {
        output_node.inputs.push(graphy::PinInstance {
            id: input.id.clone(),
            pin: graphy::Pin {
                id: input.id.clone(),
                name: input.name.clone(),
                pin_type: PinType::Input,
                data_type: input.data_type.clone(),
            },
        });
    }
    graph_desc.add_node(output_node);
    graph_desc.add_connection(Connection::new(
        node_id,
        pin_id,
        &preview_output_id,
        &color_input.id,
        ConnectionType::Data,
    ));

    let mut wgsl = compile_fragment_shader(&graph_desc)
        .map_err(|e| format!("Preview WGSL compilation failed: {}", e))?;
    wgsl.insert_str(0, &texture_bindings);
    if type_name(pin.data_type()) == "vec3<f32>" {
        wgsl = wrap_vec3_preview_return(&wgsl)?;
    }
    if preview_wgsl_requires_external_resources(&wgsl) {
        return Ok(unsupported_resource_preview_wgsl());
    }

    Ok(wgsl)
}

/// Convert TextureSrc's editor-facing asset path into the opaque resource
/// expression expected by WGSL. These handles cannot be WGSL constants, so
/// each unconnected TextureSrc pin becomes a module binding instead.
pub(crate) fn lower_texture_sources(graph: &mut GraphDescription) -> Result<String, String> {
    let mut declarations = Vec::new();
    let mut next_binding = 1u32; // binding zero is the shared sampler
    let mut needs_sampler = false;

    let connections = graph.connections.clone();
    // Nodes in id order so binding numbers are stable.
    let mut ids: Vec<String> = graph.nodes.keys().cloned().collect();
    ids.sort();
    for id in ids {
        let Some(node) = graph.nodes.get_mut(&id) else {
            continue;
        };
        let title = node_title(node);
        let texture_pins: Vec<String> = node
            .inputs
            .iter()
            .filter(|pin| type_name(pin.data_type()) == "TextureSrc")
            .map(|pin| pin.id.clone())
            .collect();

        for pin_id in texture_pins {
            if let Some(pin) = node.inputs.iter_mut().find(|item| item.id == pin_id) {
                pin.pin.data_type = DataType::typed("texture_2d<f32>");
            }

            let connected = connections
                .iter()
                .any(|c| c.target_node == id && c.target_pin == pin_id);
            if connected {
                continue;
            }

            let stored_value = node
                .properties
                .get(&pin_id)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!(
                    "Texture sample node '{}' has no texture selected. Choose an image asset in the Details panel.",
                    title
                ))?;
            let path = resolve_texture_asset_path(stored_value);
            let resource_name = format!("pulsar_texture_{}", declarations.len());
            node.properties.insert(
                pin_id.clone(),
                serde_json::Value::String(resource_name.clone()),
            );
            declarations.push(format!(
                "// TextureSrc: {}\n@group(1) @binding({}) var {}: texture_2d<f32>;\n",
                serde_json::to_string(&path).unwrap_or_else(|_| "\"\"".into()),
                next_binding,
                resource_name,
            ));
            next_binding += 1;
            needs_sampler = true;
        }
    }

    if needs_sampler {
        declarations.insert(
            0,
            "@group(1) @binding(0) var texture_sampler: sampler;\n".into(),
        );
    }

    Ok(declarations.join(""))
}


/// Resolve a GUID-backed TextureSrc into the project-relative path consumed by
/// preview renderers. Legacy path strings pass through unchanged.
pub(crate) fn resolve_texture_asset_path(value: &str) -> String {
    let Some(reference) = pulsar_reflection::TextureRef::parse(value) else {
        return value.to_owned();
    };
    let Some(root) = engine_state::get_project_path().map(std::path::PathBuf::from) else {
        return value.to_owned();
    };
    engine_fs::open_project_file_index(&root)
        .ok()
        .flatten()
        .and_then(|index| {
            index
                .path_for_asset_id_kind("texture", reference.as_str())
                .ok()
                .flatten()
        })
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .unwrap_or_else(|| value.to_owned())
}

fn property_to_psgc_value(value: &str, is_input_property: bool) -> Option<serde_json::Value> {
    let value = value.trim();
    if is_input_property && value.is_empty() {
        return None;
    }

    Some(if let Ok(number) = value.parse::<f64>() {
        serde_json::json!(number)
    } else if let Ok(boolean) = value.parse::<bool>() {
        serde_json::json!(boolean)
    } else {
        serde_json::json!(value)
    })
}

#[cfg(test)]
mod property_conversion_tests {
    use super::property_to_psgc_value;

    #[test]
    fn blank_input_property_uses_shader_default() {
        assert_eq!(property_to_psgc_value("", true), None);
        assert_eq!(property_to_psgc_value("   ", true), None);
    }

    #[test]
    fn blank_non_input_property_is_preserved() {
        assert_eq!(
            property_to_psgc_value("", false),
            Some(serde_json::Value::String(String::new()))
        );
    }

    #[test]
    fn scalar_input_properties_keep_their_types() {
        assert_eq!(
            property_to_psgc_value("0.5", true),
            Some(serde_json::json!(0.5))
        );
        assert_eq!(
            property_to_psgc_value("true", true),
            Some(serde_json::json!(true))
        );
    }
}

fn preview_wgsl_requires_external_resources(wgsl: &str) -> bool {
    let uses_external_resource = wgsl.contains("@group(1)")
        || wgsl.contains("textureSample(")
        || wgsl.contains("textureSampleLevel(")
        || wgsl.contains("textureSampleGrad(")
        || wgsl.contains("texture_2d<")
        || wgsl.contains(": sampler");
    // PinPreviewRenderer can bind TextureSrc assets. Other external resources
    // still lack a preview binding and use the safe fallback.
    uses_external_resource && !wgsl.contains("// TextureSrc:")
}

fn unsupported_resource_preview_wgsl() -> String {
    r#"struct Uniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    time: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@fragment
fn fragment_main(
    @builtin(position) frag_coord: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
"#
    .to_string()
}

fn wrap_vec3_preview_return(wgsl: &str) -> Result<String, String> {
    // The codegen now emits `return FragmentOutput(base_color, metallic, …)`
    // — wrap the first struct argument in `vec4<f32>(..., 1.0)` so a vec3
    // node output works with the vec4 base_color field.
    let output_struct = "FragmentOutput(";
    if let Some(pos) = wgsl.rfind(output_struct) {
        let before = &wgsl[..pos];
        let after = &wgsl[pos + output_struct.len()..];
        // Find the first comma-separated argument
        if let Some(comma) = after.find(',') {
            let first_arg = after[..comma].trim();
            return Ok(format!(
                "{}return vec4<f32>({}, 1.0){}",
                before,
                first_arg,
                &after[comma..]
            ));
        }
        // Single-value struct (shouldn't happen with current PBR output,
        // but handle gracefully)
        let trimmed = after.trim_end_matches(')').trim_end_matches(';').trim();
        return Ok(format!("{}return vec4<f32>({}, 1.0);", before, trimmed));
    }

    // Fallback: try the old `return <expr>;` pattern
    let mut replaced = false;
    let mut lines = Vec::new();
    for line in wgsl.lines() {
        let trimmed = line.trim_start();
        if !replaced && trimmed.starts_with("return ") && trimmed.ends_with(';') {
            let indent_len = line.len() - trimmed.len();
            let indent = &line[..indent_len];
            let expr = trimmed
                .trim_start_matches("return ")
                .trim_end_matches(';')
                .trim();
            // Skip if expression is already vec4
            if !expr.starts_with("vec4<f32>") {
                lines.push(format!("{}return vec4<f32>({}, 1.0);", indent, expr));
                replaced = true;
                continue;
            }
        }
        lines.push(line.to_string());
    }

    if replaced {
        Ok(lines.join("\n"))
    } else {
        Err("Unable to adapt vec3 preview shader output".to_string())
    }
}

#[cfg(test)]
mod compile_tests {
    use super::*;
    use crate::features::compilation::compiler::validate_wgsl;
    use crate::profile::node_from_definition;

    fn node(graph: &mut GraphDescription, kind: &str, id: &str) {
        let definition = NodeDefinitions::load()
            .get_node_definition(kind)
            .unwrap_or_else(|| panic!("missing node definition {kind}"));
        graph.add_node(node_from_definition(definition, id, Position::new(0.0, 0.0)));
    }

    fn wire(graph: &mut GraphDescription, from: &str, from_pin: &str, to: &str, to_pin: &str) {
        graph.add_connection(Connection::new(from, from_pin, to, to_pin, ConnectionType::Data));
    }

    /// The graph a new material starts with.
    fn rainbow_graph() -> GraphDescription {
        let mut graph = GraphDescription::new("rainbow");
        node(&mut graph, "frag_uv", "uv");
        node(&mut graph, "time", "time");
        node(&mut graph, "rainbow", "rainbow");
        node(&mut graph, "fragment_output", "out");
        wire(&mut graph, "uv", "result", "rainbow", "uv");
        wire(&mut graph, "time", "result", "rainbow", "shift");
        wire(&mut graph, "rainbow", "result", "out", "color");
        graph
    }

    #[test]
    fn a_new_materials_graph_compiles_to_valid_wgsl() {
        let wgsl = compile_graph_to_wgsl(&rainbow_graph()).expect("PSGC compiles the sample graph");
        validate_wgsl(&wgsl).expect("the WGSL validates");
        assert!(wgsl.contains("fragment_main"));
    }

    /// The document is not changed by compiling it.
    #[test]
    fn compiling_leaves_the_document_alone() {
        let graph = rainbow_graph();
        let before = serde_json::to_string(&graph.nodes["out"]).unwrap();
        compile_graph_to_wgsl(&graph).unwrap();
        assert_eq!(before, serde_json::to_string(&graph.nodes["out"]).unwrap());
    }

    /// A default typed as text (what the text field and older files store) reaches
    /// PSGC as the same number.
    #[test]
    fn text_defaults_compile_like_numbers() {
        let with = |value: serde_json::Value| {
            let mut graph = rainbow_graph();
            graph
                .nodes
                .get_mut("out")
                .unwrap()
                .properties
                .insert("metallic".into(), value);
            compile_graph_to_wgsl(&graph).unwrap()
        };
        assert_eq!(with(serde_json::json!(0.5)), with(serde_json::json!("0.5")));
        // A blank default is the shader type's default, not an empty literal.
        let blank = with(serde_json::json!(""));
        validate_wgsl(&blank).expect("a blank default does not reach the WGSL");
    }

    #[test]
    fn an_unconnected_texture_pin_becomes_a_binding_and_needs_an_asset() {
        let mut graph = GraphDescription::new("textured");
        node(&mut graph, "sample_texture", "tex");
        node(&mut graph, "fragment_output", "out");
        wire(&mut graph, "tex", "result", "out", "color");

        let error = compile_graph_to_wgsl(&graph).expect_err("no texture selected");
        assert!(error.contains("no texture selected"), "{error}");

        graph
            .nodes
            .get_mut("tex")
            .unwrap()
            .properties
            .insert("texture".into(), serde_json::json!("textures/wood.png"));
        let wgsl = compile_graph_to_wgsl(&graph).expect("compiles with a texture");
        assert!(wgsl.contains("@group(1) @binding(0) var texture_sampler: sampler;"));
        assert!(wgsl.contains("@group(1) @binding(1) var pulsar_texture_0: texture_2d<f32>;"));
        assert!(wgsl.contains("// TextureSrc: \"textures/wood.png\""));
        // The document still names the asset; only the compiler's copy was lowered.
        assert_eq!(
            graph.nodes["tex"].properties["texture"],
            serde_json::json!("textures/wood.png")
        );
    }

    #[test]
    fn a_colour_output_gets_a_preview_shader() {
        let graph = rainbow_graph();
        let wgsl = compile_preview_wgsl_for_pin(&graph, "rainbow", "result").expect("preview compiles");
        validate_wgsl(&wgsl).expect("the preview WGSL validates");
        assert!(compile_preview_wgsl_for_pin(&graph, "time", "result").is_err());
        assert!(compile_preview_wgsl_for_pin(&graph, "missing", "result").is_err());
    }
}
