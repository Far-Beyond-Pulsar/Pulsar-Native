//! The node previews: a pin's value drawn on its own (a swatch per output).
//! A pin's graph compiles with the materials' one compiler
//! (`materials_component::graph_preview`) like the material itself, and
//! draws with their preview program. Everything here works on a copy, so a
//! preview never changes the document.

use crate::definitions::NodeDefinitions;
use crate::profile::node_from_definition;
use crate::types::{is_texture_previewable, type_name};
use graphy::{Connection, ConnectionType, GraphDescription, NodeInstance, Position};

/// The display name of a node: its definition's name, else its type.
pub(crate) fn node_title(node: &NodeInstance) -> String {
    NodeDefinitions::load()
        .get_node_definition(&node.node_type)
        .map_or_else(|| node.node_type.replace('_', " "), |d| d.name.clone())
}

/// The graph that shows output `pin_id` of `node_id` on its own: the
/// document with its outputs replaced by one whose colour is that pin (a
/// `vec3` through `vec3_to_vec4`). It compiles with the materials' one
/// compiler and draws, unlit, with their preview program.
pub(crate) fn pin_preview_graph(
    document: &GraphDescription,
    node_id: &str,
    pin_id: &str,
) -> Result<GraphDescription, String> {
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
    let widen = type_name(pin.data_type()) == "vec3<f32>";

    let mut graph = document.clone();
    graph
        .nodes
        .retain(|_, node| node.node_type != "fragment_output" && node.node_type != "vertex_output");
    graph.connections.retain(|connection| {
        graph.nodes.contains_key(&connection.source_node)
            && graph.nodes.contains_key(&connection.target_node)
    });

    let definitions = NodeDefinitions::load();
    let definition = |kind: &str| {
        definitions
            .get_node_definition(kind)
            .ok_or_else(|| format!("Missing {kind} definition"))
    };
    let output_id = "__pin_preview_output__";
    graph.add_node(node_from_definition(
        definition("fragment_output")?,
        output_id,
        Position { x: 0.0, y: 0.0 },
    ));
    let (source, source_pin) = if widen {
        let widen_id = "__pin_preview_widen__";
        graph.add_node(node_from_definition(
            definition("vec3_to_vec4")?,
            widen_id,
            Position { x: 0.0, y: 0.0 },
        ));
        graph.add_connection(Connection::new(
            node_id,
            pin_id,
            widen_id,
            "input",
            ConnectionType::Data,
        ));
        (widen_id, "result")
    } else {
        (node_id, pin_id)
    };
    graph.add_connection(Connection::new(
        source,
        source_pin,
        output_id,
        "color",
        ConnectionType::Data,
    ));
    Ok(graph)
}

#[cfg(test)]
mod compile_tests {
    use super::*;
    use crate::features::compilation::compiler::validate_wgsl;

    fn node(graph: &mut GraphDescription, kind: &str, id: &str) {
        let definition = NodeDefinitions::load()
            .get_node_definition(kind)
            .unwrap_or_else(|| panic!("missing node definition {kind}"));
        graph.add_node(node_from_definition(
            definition,
            id,
            Position::new(0.0, 0.0),
        ));
    }

    fn wire(graph: &mut GraphDescription, from: &str, from_pin: &str, to: &str, to_pin: &str) {
        graph.add_connection(Connection::new(
            from,
            from_pin,
            to,
            to_pin,
            ConnectionType::Data,
        ));
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

    /// A new material's graph compiles through the materials' compiler into
    /// a program that validates.
    #[test]
    fn a_new_materials_graph_compiles_to_valid_wgsl() {
        let preview = materials_component::graph_preview::compile_graph(
            &rainbow_graph(),
            std::path::Path::new("."),
        )
        .expect("the materials' compiler compiles the sample graph");
        let source = materials_component::preview_program::program_source(&preview).unwrap();
        validate_wgsl(&source).expect("the program validates");
    }

    /// The document is not changed by compiling it.
    #[test]
    fn compiling_leaves_the_document_alone() {
        let graph = rainbow_graph();
        let before = serde_json::to_string(&graph.nodes["out"]).unwrap();
        materials_component::graph_preview::compile_graph(&graph, std::path::Path::new("."))
            .unwrap();
        assert_eq!(before, serde_json::to_string(&graph.nodes["out"]).unwrap());
    }

    /// A default typed as text (what the text field and older files store)
    /// compiles as the same number, in the engine as in the editor.
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
            materials_component::graph_preview::compile_graph(&graph, std::path::Path::new("."))
                .unwrap()
                .snippet
        };
        assert_eq!(with(serde_json::json!(0.5)), with(serde_json::json!("0.5")));
        // A blank default is the shader type's default, not an empty literal.
        let blank = materials_component::graph_preview::GraphPreview {
            snippet: with(serde_json::json!("")),
            textures: Vec::new(),
        };
        validate_wgsl(&materials_component::preview_program::program_source(&blank).unwrap())
            .expect("a blank default does not reach the WGSL");
    }

    /// A colour output's preview graph compiles through the materials'
    /// compiler into a program that validates; a vec3 output is widened.
    /// Scalars and missing pins have no preview.
    #[test]
    fn a_colour_output_gets_a_preview_shader() {
        let mut graph = rainbow_graph();
        node(&mut graph, "frag_normal", "normal");
        for (node_id, pin_id) in [("rainbow", "result"), ("normal", "result")] {
            let preview = pin_preview_graph(&graph, node_id, pin_id).expect("a preview graph");
            let compiled = materials_component::graph_preview::compile_graph(
                &preview,
                std::path::Path::new("."),
            )
            .expect("the preview compiles");
            validate_wgsl(
                &materials_component::preview_program::program_source(&compiled).unwrap(),
            )
            .expect("the preview program validates");
        }
        assert!(pin_preview_graph(&graph, "time", "result").is_err());
        assert!(pin_preview_graph(&graph, "missing", "result").is_err());
    }
}
