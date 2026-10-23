//! Render graphs in the graph editor's terms.
//!
//! The editor edits `blueprint_graph` descriptions; the engine reads
//! `helio_graph_asset::GraphAsset`. This module is the bridge, and it is pure
//! (no UI), so it is unit tested: the node catalogue, the pin type strings the
//! editor compares, pin styles, and the conversions both ways.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use blueprint_editor_plugin::profile::{PinShape, PinStyle};
use blueprint_editor_plugin::{NodeCategory, NodeDefinition, NodeDefinitions, PinDefinition};
use blueprint_graph::{
    BlueprintAsset, Connection as DescConnection, ConnectionType, DataType, GraphDescription,
    NodeInstance as DescNode, Pin as DescPin, PinInstance, PinType as DescPinType, Position,
};
use helio_graph_asset::{
    Connection, GraphAsset, NodeDescriptor, NodeInstance, NodeRegistry, ParamType, Params, PinDesc,
    PinKind, PinRef, PinType, Severity,
};
use serde_json::Value;

/// The node types render graphs can use.
pub fn registry() -> &'static Arc<NodeRegistry> {
    static REGISTRY: OnceLock<Arc<NodeRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Arc::new(helio_default_graphs::nodes::node_registry()))
}

// ── Pin types ────────────────────────────────────────────────────────────────

/// The editor connects two pins when their type strings are equal, so the
/// string carries everything a render pin's compatibility depends on: kind,
/// persistence and the Helio resource name. Flow pins are the editor's own
/// execution pins.
pub fn pin_type_string(pin: &PinDesc) -> String {
    let prefix = match (pin.ty.kind, pin.ty.persistent) {
        (PinKind::Flow, _) => return "execution".to_string(),
        (PinKind::Texture, false) => "tex",
        (PinKind::Texture, true) => "ptex",
        (PinKind::Depth, _) => "depth",
        (PinKind::Buffer, false) => "buf",
        (PinKind::Buffer, true) => "pbuf",
        (PinKind::Scene, _) => "scene",
    };
    format!("{prefix}:{}", pin.resource)
}

/// The inverse of [`pin_type_string`] for the part that decides appearance.
fn pin_type_of(type_name: &str) -> PinType {
    let prefix = type_name.split_once(':').map_or(type_name, |(prefix, _)| prefix);
    match prefix {
        "tex" => PinType::TEXTURE,
        "ptex" => PinType::PERSISTENT_TEXTURE,
        "depth" => PinType::DEPTH,
        "buf" => PinType::BUFFER,
        "pbuf" => PinType::PERSISTENT_BUFFER,
        "scene" => PinType::SCENE,
        _ => PinType::FLOW,
    }
}

/// How the editor draws a pin of `type_name`: the shape follows the kind, a
/// second outline marks persistent data.
pub fn pin_style(type_name: &str) -> PinStyle {
    let ty = pin_type_of(type_name);
    let shape = match ty.shape() {
        helio_graph_asset::PinShape::Arrow => PinShape::Arrow,
        helio_graph_asset::PinShape::Circle => PinShape::Circle,
        helio_graph_asset::PinShape::Diamond => PinShape::Diamond,
        helio_graph_asset::PinShape::Square => PinShape::Square,
        helio_graph_asset::PinShape::Hexagon => PinShape::Hexagon,
    };
    let [r, g, b] = ty.color();
    PinStyle {
        shape,
        ringed: ty.ringed(),
        color: Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]),
    }
}

// ── Catalogue ────────────────────────────────────────────────────────────────

fn category_look(category: &str) -> (&'static str, &'static str) {
    match category {
        "Graph" => ("#6B7280", "⏹"),
        "Resources" => ("#B7791F", "▦"),
        "Helio Stages" => ("#2B6CB0", "◈"),
        _ => ("#4A5568", "●"),
    }
}

fn param_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn definition_pins(pins: &[PinDesc], pin_type: blueprint_editor_plugin::PinType) -> Vec<PinDefinition> {
    pins.iter()
        .map(|pin| PinDefinition {
            id: pin.name.clone(),
            name: if pin.ty.kind == PinKind::Flow { String::new() } else { pin.name.clone() },
            data_type: blueprint_editor_plugin::PinDataType::from_type_str(pin_type_string(pin)),
            pin_type: pin_type.clone(),
        })
        .collect()
}

fn node_definition(descriptor: &NodeDescriptor) -> NodeDefinition {
    let params = descriptor.effective_params(&Params::new());
    let pins = descriptor.pins(&params);
    let (color, icon) = category_look(&descriptor.category);
    NodeDefinition {
        id: descriptor.kind.clone(),
        name: descriptor.title.clone(),
        icon: icon.to_string(),
        description: descriptor.description.clone(),
        documentation: descriptor.description.clone(),
        inputs: definition_pins(&pins.inputs, blueprint_editor_plugin::PinType::Input),
        outputs: definition_pins(&pins.outputs, blueprint_editor_plugin::PinType::Output),
        properties: params.iter().map(|(k, v)| (k.clone(), param_string(v))).collect(),
        color: Some(color.to_string()),
        is_event: false,
    }
}

/// The editor's palette for `registry`: one category per node category.
pub fn catalog(registry: &NodeRegistry) -> NodeDefinitions {
    let mut by_category: BTreeMap<String, Vec<NodeDefinition>> = BTreeMap::new();
    for descriptor in registry.list() {
        by_category
            .entry(descriptor.category.clone())
            .or_default()
            .push(node_definition(&descriptor));
    }
    NodeDefinitions {
        categories: by_category
            .into_iter()
            .map(|(name, nodes)| NodeCategory {
                color: category_look(&name).0.to_string(),
                name,
                nodes,
            })
            .collect(),
    }
}

// ── Params ───────────────────────────────────────────────────────────────────

/// A parameter as the editor stores it (text) read back as its declared type.
fn param_from_text(ty: &ParamType, text: &str) -> Value {
    match ty {
        ParamType::Bool => Value::from(text.trim().eq_ignore_ascii_case("true")),
        ParamType::Int { .. } => text.trim().parse::<i64>().map_or(Value::from(text), Value::from),
        ParamType::Float { .. } => text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::from(text), Value::Number),
        ParamType::Text | ParamType::Choice(_) => Value::from(text),
    }
}

/// Params from the editor's text properties; only keys the node declares.
fn params_from_text<'a>(
    descriptor: &NodeDescriptor,
    properties: impl Iterator<Item = (&'a String, String)>,
) -> Params {
    let mut params = Params::new();
    for (key, text) in properties {
        if let Some(declared) = descriptor.params.iter().find(|p| &p.key == key) {
            let value = param_from_text(&declared.ty, &text);
            // Defaults are not stored; the asset stays as small as it was authored.
            if value != declared.default {
                params.insert(key.clone(), value);
            }
        }
    }
    params
}

// ── Asset <-> editor description ─────────────────────────────────────────────

/// A render graph as the editor's document model.
pub fn to_blueprint_asset(asset: &GraphAsset, registry: &NodeRegistry) -> Result<BlueprintAsset, String> {
    let mut graph = GraphDescription::new(if asset.name.is_empty() { "Render Graph" } else { &asset.name });
    graph.metadata.description = asset.description.clone();

    let mut pin_kinds: HashMap<(String, String), PinKind> = HashMap::new();
    for node in &asset.nodes {
        let mut instance = DescNode::new(
            &node.id,
            &node.kind,
            Position { x: node.position[0], y: node.position[1] },
        );
        if let Some(descriptor) = registry.get(&node.kind) {
            let params = descriptor.effective_params(&node.params);
            for (key, value) in &params {
                instance.properties.insert(key.clone(), Value::from(param_string(value)));
            }
            let pins = descriptor.pins(&params);
            for (list, direction, is_input) in [
                (&pins.inputs, DescPinType::Input, true),
                (&pins.outputs, DescPinType::Output, false),
            ] {
                for pin in list {
                    pin_kinds.insert((node.id.clone(), pin.name.clone()), pin.ty.kind);
                    let entry = PinInstance {
                        id: pin.name.clone(),
                        pin: DescPin {
                            name: if pin.ty.kind == PinKind::Flow { String::new() } else { pin.name.clone() },
                            pin_type: direction.clone(),
                            data_type: DataType::from_type_str(&pin_type_string(pin)),
                            connected_to: Vec::new(),
                        },
                    };
                    if is_input {
                        instance.inputs.push(entry);
                    } else {
                        instance.outputs.push(entry);
                    }
                }
            }
        }
        graph.add_node(instance)?;
    }

    for (index, connection) in asset.connections.iter().enumerate() {
        let kind = pin_kinds
            .get(&(connection.from.node.clone(), connection.from.pin.clone()))
            .copied();
        let wire = DescConnection {
            id: format!("wire{index}"),
            source_node: connection.from.node.clone(),
            source_pin: connection.from.pin.clone(),
            target_node: connection.to.node.clone(),
            target_pin: connection.to.pin.clone(),
            connection_type: if kind == Some(PinKind::Flow) {
                ConnectionType::Execution
            } else {
                ConnectionType::Data
            },
        };
        // A wire to a pin the node no longer has is dropped here; validation of
        // the saved asset reports the same thing.
        if let Err(error) = graph.add_connection(wire) {
            log::warn!("render graph `{}`: dropped wire: {error}", asset.name);
        }
    }

    let mut blueprint = BlueprintAsset::new();
    blueprint.main_graph = graph;
    Ok(blueprint)
}

/// The inverse of [`to_blueprint_asset`].
pub fn from_blueprint_asset(blueprint: &BlueprintAsset, registry: &NodeRegistry) -> GraphAsset {
    let graph = &blueprint.main_graph;
    let mut ids: Vec<&String> = graph.nodes.keys().collect();
    ids.sort();
    let nodes = ids
        .into_iter()
        .map(|id| {
            let node = &graph.nodes[id];
            let params = registry.get(&node.node_type).map_or_else(Params::new, |descriptor| {
                params_from_text(
                    descriptor,
                    node.properties.iter().map(|(key, value)| (key, param_string(value))),
                )
            });
            NodeInstance {
                id: node.id.clone(),
                kind: node.node_type.clone(),
                params,
                position: [node.position.x, node.position.y],
            }
        })
        .collect();
    GraphAsset {
        name: graph.metadata.name.clone(),
        description: graph.metadata.description.clone(),
        nodes,
        connections: graph
            .connections
            .iter()
            .map(|c| Connection {
                from: PinRef::new(&c.source_node, &c.source_pin),
                to: PinRef::new(&c.target_node, &c.target_pin),
            })
            .collect(),
        ..GraphAsset::default()
    }
}

// ── The editor's live graph ──────────────────────────────────────────────────

/// A graph the editor is showing, reduced to what compilation needs.
pub struct LiveNode {
    pub id: String,
    pub definition_id: String,
    pub position: [f32; 2],
    pub properties: HashMap<String, String>,
}

pub struct LiveWire {
    pub source_node: String,
    pub source_pin: String,
    pub target_node: String,
    pub target_pin: String,
}

const REROUTE: &str = "reroute";

/// The asset a live editor graph describes. Reroute nodes are the editor's own
/// tidying device; wires through them are joined end to end.
pub fn asset_from_live(
    name: &str,
    nodes: &[LiveNode],
    wires: &[LiveWire],
    registry: &NodeRegistry,
) -> GraphAsset {
    let reroutes: HashSet<&str> =
        nodes.iter().filter(|n| n.definition_id == REROUTE).map(|n| n.id.as_str()).collect();

    // Follow a wire's end through any reroutes to the real pins it reaches.
    let reaches = |start_node: &str| -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut pending = vec![start_node.to_string()];
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(reroute) = pending.pop() {
            if !seen.insert(reroute.clone()) {
                continue; // a reroute cycle
            }
            for wire in wires.iter().filter(|w| w.source_node == reroute) {
                if reroutes.contains(wire.target_node.as_str()) {
                    pending.push(wire.target_node.clone());
                } else {
                    found.push((wire.target_node.clone(), wire.target_pin.clone()));
                }
            }
        }
        found
    };

    let mut connections = Vec::new();
    for wire in wires.iter().filter(|w| !reroutes.contains(w.source_node.as_str())) {
        if reroutes.contains(wire.target_node.as_str()) {
            for (node, pin) in reaches(&wire.target_node) {
                connections.push(Connection {
                    from: PinRef::new(&wire.source_node, &wire.source_pin),
                    to: PinRef::new(node, pin),
                });
            }
        } else {
            connections.push(Connection {
                from: PinRef::new(&wire.source_node, &wire.source_pin),
                to: PinRef::new(&wire.target_node, &wire.target_pin),
            });
        }
    }

    GraphAsset {
        name: name.to_string(),
        nodes: nodes
            .iter()
            .filter(|n| n.definition_id != REROUTE)
            .map(|n| {
                let params = registry.get(&n.definition_id).map_or_else(Params::new, |descriptor| {
                    params_from_text(
                        descriptor,
                        n.properties.iter().map(|(key, value)| (key, value.clone())),
                    )
                });
                NodeInstance {
                    id: n.id.clone(),
                    kind: n.definition_id.clone(),
                    params,
                    position: n.position,
                }
            })
            .collect(),
        connections,
        ..GraphAsset::default()
    }
}

/// Problems that stop a graph from building, as one line each.
pub fn problems(asset: &GraphAsset, registry: &NodeRegistry) -> Vec<String> {
    helio_graph_asset::validate(asset, registry)
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(ToString::to_string)
        .collect()
}


// ── Editing ───────────────────────────────────────────────────────────────────

/// The parameters the editor shows for a node type.
pub fn parameter_fields(
    definition_id: &str,
    registry: &NodeRegistry,
) -> Vec<blueprint_editor_plugin::profile::ParamField> {
    use blueprint_editor_plugin::profile::{ParamField, ParamKind};
    registry.get(definition_id).map_or_else(Vec::new, |descriptor| {
        descriptor
            .params
            .iter()
            .map(|param| ParamField {
                key: param.key.clone(),
                label: param.label.clone(),
                kind: match &param.ty {
                    ParamType::Bool => ParamKind::Bool,
                    ParamType::Choice(options) => ParamKind::Choice(options.clone()),
                    ParamType::Text | ParamType::Int { .. } | ParamType::Float { .. } => {
                        ParamKind::Text
                    }
                },
            })
            .collect()
    })
}

fn editor_pins(
    pins: &[PinDesc],
    direction: blueprint_editor_plugin::PinType,
) -> Vec<blueprint_editor_plugin::Pin> {
    pins.iter()
        .map(|pin| blueprint_editor_plugin::Pin {
            id: pin.name.clone(),
            name: if pin.ty.kind == PinKind::Flow { String::new() } else { pin.name.clone() },
            pin_type: direction.clone(),
            data_type: blueprint_editor_plugin::PinDataType::from_type_str(pin_type_string(pin)),
        })
        .collect()
}

/// Re-derives a node's pins after one of its parameters changed. A resource
/// node's pin is named after the resource, so its wires are moved to the pin's
/// new id rather than left pointing at one that no longer exists.
pub fn refresh_pins(
    graph: &mut blueprint_editor_plugin::BlueprintGraph,
    node_id: &str,
    registry: &NodeRegistry,
) {
    let Some(node) = graph.nodes.iter().find(|n| n.id == node_id) else {
        return;
    };
    let Some(descriptor) = registry.get(&node.definition_id) else {
        return;
    };
    let params = params_from_text(
        descriptor,
        node.properties.iter().map(|(key, value)| (key, value.clone())),
    );
    let pins = descriptor.pins(&params);
    let inputs = editor_pins(&pins.inputs, blueprint_editor_plugin::PinType::Input);
    let outputs = editor_pins(&pins.outputs, blueprint_editor_plugin::PinType::Output);

    // Pins keep their position, so the i-th old pin becomes the i-th new one.
    let renamed = |old: &[blueprint_editor_plugin::Pin], new: &[blueprint_editor_plugin::Pin]| {
        old.iter()
            .zip(new)
            .filter(|(old, new)| old.id != new.id)
            .map(|(old, new)| (old.id.clone(), new.id.clone()))
            .collect::<Vec<_>>()
    };
    let renamed_inputs = renamed(&node.inputs, &inputs);
    let renamed_outputs = renamed(&node.outputs, &outputs);

    if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == node_id) {
        node.inputs = inputs;
        node.outputs = outputs;
    }
    for connection in graph.connections.iter_mut() {
        if connection.source_node == node_id {
            if let Some((_, new)) = renamed_outputs.iter().find(|(old, _)| *old == connection.source_pin) {
                connection.source_pin = new.clone();
            }
        }
        if connection.target_node == node_id {
            if let Some((_, new)) = renamed_inputs.iter().find(|(old, _)| *old == connection.target_pin) {
                connection.target_pin = new.clone();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use helio_default_graphs::nodes::{default_graph_asset, node_registry};

    #[test]
    fn pin_type_strings_carry_kind_persistence_and_resource() {
        let tex = PinDesc::resource(PinType::TEXTURE, "pre_aa");
        let history = PinDesc::resource(PinType::PERSISTENT_TEXTURE, "pre_aa");
        assert_eq!(pin_type_string(&tex), "tex:pre_aa");
        assert_eq!(pin_type_string(&history), "ptex:pre_aa");
        assert_ne!(pin_type_string(&tex), pin_type_string(&history));
        assert_eq!(pin_type_string(&PinDesc::flow("out")), "execution");
    }

    #[test]
    fn each_pin_kind_has_its_own_shape_and_persistence_rings() {
        let shapes: Vec<_> =
            ["execution", "tex:a", "depth:a", "buf:a", "scene:a"].map(|t| pin_style(t).shape).into();
        assert_eq!(
            shapes,
            [PinShape::Arrow, PinShape::Circle, PinShape::Diamond, PinShape::Square, PinShape::Hexagon]
        );
        assert!(pin_style("ptex:a").ringed && pin_style("pbuf:a").ringed);
        assert!(!pin_style("tex:a").ringed && !pin_style("buf:a").ringed);
        assert_eq!(pin_style("pbuf:a").shape, PinShape::Square);
    }

    #[test]
    fn the_catalogue_lists_every_registered_node_with_matching_pins() {
        let registry = node_registry();
        let catalog = catalog(&registry);
        let listed: usize = catalog.categories.iter().map(|c| c.nodes.len()).sum();
        assert_eq!(listed, registry.list().len());
        let buffer = catalog
            .categories
            .iter()
            .flat_map(|c| &c.nodes)
            .find(|n| n.id == "core.persistent_buffer")
            .expect("persistent buffer node");
        assert_eq!(buffer.outputs.len(), 1);
        assert_eq!(buffer.outputs[0].data_type.type_name, "pbuf:my_buffer");
    }

    #[test]
    fn the_default_graph_survives_the_editor_model_both_ways() {
        let registry = node_registry();
        let asset = default_graph_asset();
        let blueprint = to_blueprint_asset(&asset, &registry).unwrap();
        assert_eq!(blueprint.main_graph.nodes.len(), asset.nodes.len());
        assert_eq!(blueprint.main_graph.connections.len(), asset.connections.len());

        let back = from_blueprint_asset(&blueprint, &registry);
        let mut expected = asset.clone();
        expected.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(back.nodes, expected.nodes);
        assert_eq!(back.connections, asset.connections);
        assert!(problems(&back, &registry).is_empty());
    }

    #[test]
    fn edited_parameters_round_trip_as_their_declared_types() {
        let registry = node_registry();
        let mut asset = GraphAsset::default();
        asset.nodes.push(NodeInstance {
            id: "buf".into(),
            kind: "core.persistent_buffer".into(),
            params: [
                ("name".to_string(), Value::from("counters")),
                ("size_bytes".to_string(), Value::from(4096)),
            ]
            .into(),
            position: [10.0, 20.0],
        });
        let blueprint = to_blueprint_asset(&asset, &registry).unwrap();
        let node = &blueprint.main_graph.nodes["buf"];
        assert_eq!(node.outputs[0].pin.data_type.to_string(), "pbuf:counters");
        let back = from_blueprint_asset(&blueprint, &registry);
        assert_eq!(back.nodes[0].params["size_bytes"], Value::from(4096));
        assert_eq!(back.nodes[0].params["name"], Value::from("counters"));
    }

    #[test]
    fn live_graphs_join_wires_through_reroutes() {
        let registry = node_registry();
        let node = |id: &str, kind: &str| LiveNode {
            id: id.into(),
            definition_id: kind.into(),
            position: [0.0; 2],
            properties: HashMap::new(),
        };
        let wire = |a: &str, ap: &str, b: &str, bp: &str| LiveWire {
            source_node: a.into(),
            source_pin: ap.into(),
            target_node: b.into(),
            target_pin: bp.into(),
        };
        let nodes = [
            node("early", "helio.stage.early"),
            node("r", "reroute"),
            node("out", "core.output"),
        ];
        let wires = [wire("early", "out", "r", "input"), wire("r", "output", "out", "in")];
        let asset = asset_from_live("g", &nodes, &wires, &registry);
        assert_eq!(asset.nodes.len(), 2);
        assert_eq!(
            asset.connections,
            vec![Connection { from: PinRef::new("early", "out"), to: PinRef::new("out", "in") }]
        );
    }

    #[test]
    fn problems_report_a_graph_without_an_output() {
        let registry = node_registry();
        let found = problems(&GraphAsset::default(), &registry);
        assert!(found.iter().any(|p| p.contains("Output")), "{found:?}");
    }
}
