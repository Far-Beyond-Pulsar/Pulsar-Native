//! Graph conversion - convert between BlueprintGraph and GraphDescription formats

use gpui::*;
use super::core::BlueprintEditorPanel;
use super::super::{BlueprintGraph, BlueprintNode, BlueprintComment, Connection, Pin, PinType, NodeType, NodeDefinitions};
use ui::graph::{self as graph_types, GraphDescription, NodeInstance, PropertyValue, Position};

impl BlueprintEditorPanel {
    /// Convert current blueprint graph to graph description
    pub(super) fn convert_to_graph_description(&self) -> Result<GraphDescription, String> {
        self.convert_graph_to_description(&self.graph)
    }

    /// Convert any blueprint graph to graph description
    pub(super) fn convert_graph_to_description(
        &self,
        graph: &BlueprintGraph
    ) -> Result<GraphDescription, String> {
        let mut graph_desc = GraphDescription::new("Blueprint Graph");

        // Convert nodes
        for bp_node in &graph.nodes {
            let mut node_instance = NodeInstance::new(
                &bp_node.id,
                &self.get_node_type_from_blueprint(bp_node)?,
                Position {
                    x: bp_node.position.x,
                    y: bp_node.position.y,
                },
            );

            // Convert pins
            for pin in &bp_node.inputs {
                node_instance.add_input_pin(&pin.id, pin.data_type.clone());
            }
            for pin in &bp_node.outputs {
                node_instance.add_output_pin(&pin.id, pin.data_type.clone());
            }

            // Convert properties
            for (key, value) in &bp_node.properties {
                let prop_value = if value.parse::<f64>().is_ok() {
                    PropertyValue::Number(value.parse().unwrap())
                } else if value.parse::<bool>().is_ok() {
                    PropertyValue::Boolean(value.parse().unwrap())
                } else {
                    PropertyValue::String(value.clone())
                };
                node_instance.set_property(key, prop_value);
            }

            graph_desc.add_node(node_instance);
        }

        // Convert connections
        for connection in &graph.connections {
            let conn_type = graph.nodes.iter()
                .find(|n| n.id == connection.source_node)
                .and_then(|node| node.outputs.iter().find(|p| p.id == connection.source_pin))
                .map(|pin| match &pin.data_type {
                    graph_types::DataType::Execution => graph_types::ConnectionType::Execution,
                    _ => graph_types::ConnectionType::Data,
                })
                .unwrap_or(graph_types::ConnectionType::Data);

            let graph_connection = graph_types::Connection::new(
                &connection.id,
                &connection.source_node,
                &connection.source_pin,
                &connection.target_node,
                &connection.target_pin,
                conn_type,
            );
            graph_desc.add_connection(graph_connection);
        }

        // Convert comments
        graph_desc.comments = graph.comments.iter().map(|c| {
            graph_types::BlueprintComment {
                id: c.id.clone(),
                text: c.text.clone(),
                position: (c.position.x, c.position.y),
                size: (c.size.width, c.size.height),
                color: [c.color.h, c.color.s, c.color.l, c.color.a],
                contained_node_ids: c.contained_node_ids.clone(),
            }
        }).collect();

        Ok(graph_desc)
    }

    /// Get node type from blueprint node
    fn get_node_type_from_blueprint(&self, bp_node: &BlueprintNode) -> Result<String, String> {
        Ok(bp_node.definition_id.clone())
    }

    /// Convert graph description to blueprint graph
    pub(super) fn convert_graph_description_to_blueprint(
        &mut self,
        graph_desc: &GraphDescription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<BlueprintGraph, String> {
        let mut nodes = Vec::new();
        let mut connections = Vec::new();

        let node_definitions = NodeDefinitions::load();

        // Convert nodes
        for (node_id, node_instance) in &graph_desc.nodes {
            let definition_id = node_instance.node_type.clone();
            let node_def = node_definitions.get_node_definition(&definition_id);

            let (title, icon, description, node_type, color) = if definition_id == "reroute" {
                (
                    "Reroute".to_string(),
                    "•".to_string(),
                    "Reroute node for organizing connections".to_string(),
                    NodeType::Reroute,
                    None,
                )
            } else if let Some(def) = node_def {
                let category = node_definitions.get_category_for_node(&def.id);
                let node_type = match category.map(|c| c.name.as_str()) {
                    Some("Events") => NodeType::Event,
                    Some("Logic") => NodeType::Logic,
                    Some("Math") => NodeType::Math,
                    Some("Object") => NodeType::Object,
                    _ => NodeType::Logic,
                };
                (
                    def.name.clone(),
                    def.icon.clone(),
                    def.description.clone(),
                    node_type,
                    def.color.clone(),
                )
            } else {
                (
                    definition_id.replace('_', " "),
                    "⚙️".to_string(),
                    String::new(),
                    NodeType::Logic,
                    None,
                )
            };

            let bp_node = BlueprintNode {
                id: node_id.clone(),
                definition_id,
                title,
                icon,
                node_type,
                position: Point::new(node_instance.position.x, node_instance.position.y),
                size: Size::new(150.0, 100.0),
                inputs: node_instance.inputs.iter().map(|pin_inst| {
                    let pin = &pin_inst.pin;
                    Pin {
                        id: pin_inst.id.clone(),
                        name: pin.name.clone(),
                        pin_type: match pin.pin_type {
                            graph_types::PinType::Input => PinType::Input,
                            graph_types::PinType::Output => PinType::Output,
                        },
                        data_type: pin.data_type.clone(),
                    }
                }).collect(),
                outputs: node_instance.outputs.iter().map(|pin_inst| {
                    let pin = &pin_inst.pin;
                    Pin {
                        id: pin_inst.id.clone(),
                        name: pin.name.clone(),
                        pin_type: match pin.pin_type {
                            graph_types::PinType::Input => PinType::Input,
                            graph_types::PinType::Output => PinType::Output,
                        },
                        data_type: pin.data_type.clone(),
                    }
                }).collect(),
                properties: node_instance.properties.iter().map(|(k, v)| {
                    let value_str = match v {
                        PropertyValue::String(s) => s.clone(),
                        PropertyValue::Number(n) => n.to_string(),
                        PropertyValue::Boolean(b) => b.to_string(),
                        _ => "".to_string(),
                    };
                    (k.clone(), value_str)
                }).collect(),
                is_selected: false,
                description,
                color,
            };
            nodes.push(bp_node);
        }

        // Convert connections
        for connection in &graph_desc.connections {
            let bp_connection = Connection {
                id: connection.id.clone(),
                source_node: connection.source_node.clone(),
                source_pin: connection.source_pin.clone(),
                target_node: connection.target_node.clone(),
                target_pin: connection.target_pin.clone(),
                connection_type: connection.connection_type.clone(),
            };
            connections.push(bp_connection);
        }

        // Convert comments with initialized color picker states
        let comments: Vec<BlueprintComment> = graph_desc.comments.iter().map(|c| {
            let color = Hsla { h: c.color[0], s: c.color[1], l: c.color[2], a: c.color[3] };
            let color_picker_state = Some(cx.new(|cx| {
                ui::color_picker::ColorPickerState::new(window, cx)
            }));
            
            BlueprintComment {
                id: c.id.clone(),
                text: c.text.clone(),
                position: Point::new(c.position.0, c.position.1),
                size: Size::new(c.size.0, c.size.1),
                color,
                contained_node_ids: c.contained_node_ids.clone(),
                is_selected: false,
                color_picker_state,
            }
        }).collect();

        Ok(BlueprintGraph {
            nodes,
            connections,
            comments,
            selected_nodes: Vec::new(),
            selected_comments: Vec::new(),
            zoom_level: 1.0,
            pan_offset: Point::new(0.0, 0.0),
            virtualization_stats: super::super::VirtualizationStats::default(),
        })
    }
}
