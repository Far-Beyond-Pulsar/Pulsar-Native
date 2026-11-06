//! Node definition system for loading and managing node metadata.
//!
//! This module provides the infrastructure for loading shader node definitions
//! from PSGC (Pulsar Shader Graph Compiler) and converting them into UI-ready
//! node definitions. It uses a global singleton pattern for efficient access
//! to node metadata throughout the application.

use graphy::DataType;
use psgc::metadata::get_shader_nodes;
use psgc::NodeTypes;
use std::collections::HashMap;

// ============================================================================
// Node Definition Types
// ============================================================================

/// Root structure containing all node definitions organized by category.
#[derive(Debug)]
pub struct NodeDefinitions {
    pub categories: Vec<NodeCategory>,
}

/// A category of related nodes with a name and color theme.
#[derive(Debug, Clone)]
pub struct NodeCategory {
    pub name: String,
    pub color: String,
    pub nodes: Vec<NodeDefinition>,
}

/// Complete definition of a node including pins, properties, and metadata.
#[derive(Debug, Clone)]
pub struct NodeDefinition {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub description: String,
    pub documentation: String,
    pub inputs: Vec<PinDefinition>,
    pub outputs: Vec<PinDefinition>,
    pub properties: HashMap<String, String>,
    pub color: Option<String>,
    /// True when the underlying shader node is `NodeTypes::event`.
    /// Used to classify as NodeType::Event. No shader graph nodes are
    /// currently events — every node, including the output sink, is pure.
    pub is_event: bool,
}

/// Definition of a single pin on a node.
#[derive(Debug, Clone)]
pub struct PinDefinition {
    pub id: String,
    pub name: String,
    pub data_type: DataType,
}

// ============================================================================
// Global Node Definitions
// ============================================================================

/// Global node definitions (loaded once at startup)
use std::sync::OnceLock;
static NODE_DEFINITIONS: OnceLock<NodeDefinitions> = OnceLock::new();

impl NodeDefinitions {
    /// Get the global node definitions, loading them if necessary.
    pub fn load() -> &'static NodeDefinitions {
        NODE_DEFINITIONS.get_or_init(|| {
            let metadata = get_shader_nodes();
            Self::from_shader_metadata(metadata)
        })
    }

    fn from_shader_metadata(
        metadata: Vec<graphy::core::NodeMetadata>,
    ) -> NodeDefinitions {
        let mut categories_map: HashMap<String, Vec<NodeDefinition>> = HashMap::new();

        // Add reroute node to Utility category
            categories_map
                .entry("Utility".to_string())
                .or_insert_with(Vec::new)
                .push(NodeDefinition {
                    id: "reroute".to_string(),
                    name: "Reroute".to_string(),
                    icon: "•".to_string(),
                    description: "Organize connections with a pass-through node (typeless until connected)".to_string(),
                    documentation: "Organize connections with a pass-through node (typeless until connected)".to_string(),
                    inputs: vec![],
                    outputs: vec![],
                    properties: HashMap::new(),
                    color: None,
                    is_event: false,
                });

        // Constant input nodes — editable values inline in the Properties panel
        let constants: Vec<(&str, &str, &str, &str)> = vec![
            ("float_constant", "Float", "∑", "f32"),
            ("int_constant", "Integer", "∑", "int"),
            ("uint_constant", "Unsigned Integer", "∑", "uint"),
            ("bool_constant", "Boolean", "∑", "bool"),
            ("vec2_constant", "Vector 2", "↗", "vec2<f32>"),
            ("vec3_constant", "Vector 3", "↗", "vec3<f32>"),
            ("vec4_constant", "Vector 4", "↗", "vec4<f32>"),
            ("mat4_constant", "Matrix 4x4", "↗", "mat4x4<f32>"),
        ];
        let mut const_props = HashMap::new();
        const_props.insert("value".to_string(), "0".to_string());
        for (id, name, icon, type_str) in &constants {
            let out_pin = PinDefinition {
                id: "result".to_string(),
                name: "".to_string(),
                data_type: DataType::typed(*type_str),
            };
            categories_map
                .entry("Input".to_string())
                .or_insert_with(Vec::new)
                .push(NodeDefinition {
                    id: id.to_string(),
                    name: name.to_string(),
                    icon: icon.to_string(),
                    description: format!("Constant {} value — edit in the Properties panel", name),
                    documentation: format!("Constant {} value — edit in the Properties panel", name),
                    inputs: vec![],
                    outputs: vec![out_pin],
                    properties: const_props.clone(),
                    color: None,
                    is_event: false,
                });
        }

        // Group shader nodes by category
        for node_meta in metadata {
            let mut inputs = Vec::new();
            let mut outputs = Vec::new();

            // Add regular inputs
            for param in node_meta.params.iter() {
                inputs.push(PinDefinition {
                    id: param.name.to_string(),
                    name: param.name.to_string(),
                    data_type: DataType::typed(param.param_type.as_str()),
                });
            }

            // Add execution outputs if present
            for exec_pin in node_meta.exec_outputs.iter() {
                outputs.push(PinDefinition {
                    id: exec_pin.to_string(),
                    name: exec_pin.to_string(),
                    data_type: DataType::Exec,
                });
            }

            // Add named output pins from output_params (Break / Make multi-output).
            if !node_meta.output_params.is_empty() {
                for out in &node_meta.output_params {
                    outputs.push(PinDefinition {
                        id: out.name.clone(),
                        name: out.name.clone(),
                        data_type: DataType::typed(out.param_type.as_str()),
                    });
                }
            // Fall back to a single "result" pin from return_type.
            } else if let Some(return_type) = &node_meta.return_type {
                outputs.push(PinDefinition {
                    id: "result".to_string(),
                    name: "result".to_string(),
                    data_type: DataType::typed(return_type.type_string.as_str()),
                });
            }

            let category = node_meta.category.clone();
            let description = format!("{} ({})", node_meta.name, node_meta.category);
            // Shader graphs are fully pure dataflow: the output node is a
            // pure sink, not an execution event, so only an actual
            // `NodeTypes::event` node (none remain in wgsl_std) is treated
            // as an event for visual classification.
            let is_event = matches!(node_meta.node_type, NodeTypes::event);

            let static_def = NodeDefinition {
                id: node_meta.name.clone(),
                name: node_meta.name.clone(),
                icon: match category.as_str() {
                    "Math" => "∑",
                    "Vector" => "↗",
                    "Color" => "🎨",
                    "Texture" => "◉",
                    "Noise" => "▒",
                    "Input" => "⬇",
                    "Output" => "⬆",
                    _ => "⚙️",
                }.to_string(),
                description: description.clone(),
                documentation: description,
                inputs,
                outputs,
                properties: HashMap::new(),
                color: Some(Self::get_category_color(&category)),
                is_event,
            };

            categories_map
                .entry(category)
                .or_insert_with(Vec::new)
                .push(static_def);
        }

        Self::categories_to_definitions(categories_map)
    }

    fn categories_to_definitions(
        categories_map: HashMap<String, Vec<NodeDefinition>>,
    ) -> NodeDefinitions {
        let categories = categories_map
            .into_iter()
            .map(|(name, nodes)| NodeCategory {
                name: name.clone(),
                color: Self::get_category_color(&name),
                nodes,
            })
            .collect();

        NodeDefinitions { categories }
    }

    fn get_category_color(category: &str) -> String {
        match category {
            "Math" => "#4A90E2".to_string(),
            "Vector" => "#7B61FF".to_string(),
            "Color" => "#E74C3C".to_string(),
            "Texture" => "#2ECC71".to_string(),
            "Noise" => "#8E44AD".to_string(),
            "Input" => "#F39C12".to_string(),
            "Output" => "#E67E22".to_string(),
            "Utility" => "#B8E986".to_string(),
            _ => "#9B9B9B".to_string(),
        }
    }

    pub fn get_node_definition(&self, node_id: &str) -> Option<&NodeDefinition> {
        self.categories
            .iter()
            .flat_map(|category| &category.nodes)
            .find(|node| node.id == node_id)
    }

    pub fn get_node_definition_by_name(&self, node_name: &str) -> Option<&NodeDefinition> {
        self.categories
            .iter()
            .flat_map(|category| &category.nodes)
            .find(|node| node.name == node_name)
    }

    pub fn get_category_for_node(&self, node_id: &str) -> Option<&NodeCategory> {
        self.categories
            .iter()
            .find(|category| category.nodes.iter().any(|node| node.id == node_id))
    }
}
