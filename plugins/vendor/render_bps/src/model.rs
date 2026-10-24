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
    // The engine's registry: Helio's stages with the pins their passes declare
    // (the first call waits for the probe that reads them), and every node any
    // crate in this binary registered.
    REGISTRY.get_or_init(|| {
        Arc::new(engine_backend::subsystems::render::helio_renderer::pipelines::graph_node_registry())
    })
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

/// What the editor prints next to a pin: its resource, marked when it reads the
/// previous frame; nothing for a flow pin.
fn pin_label(pin: &PinDesc) -> String {
    if pin.ty.kind == PinKind::Flow {
        String::new()
    } else if pin.history {
        format!("{} (prev frame)", pin.resource)
    } else {
        pin.resource.clone()
    }
}

fn category_look(category: &str) -> (&'static str, &'static str) {
    match category {
        "Graph" => ("#6B7280", "⏹"),
        "Resources" => ("#B7791F", "▦"),
        "Helio Stages" => ("#2B6CB0", "◈"),
        "SceneDB" => ("#2F855A", "▤"),
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
            name: pin_label(pin),
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

/// Definition id of a node that places a collapsed graph, as the editor writes it.
const SUBGRAPH_PREFIX: &str = "subgraph:collapsed:";
/// Ids of the entry and exit nodes inside a collapsed graph description.
const ENTRY_ID: &str = "section_entry";
const EXIT_ID: &str = "section_exit";
const ENTRY_TYPE: &str = "macro_entry";
const EXIT_TYPE: &str = "macro_exit";

/// The collapsed graph a node placing one names: `subgraph:collapsed:<id>`, and the
/// older `subgraph:<id>` and `macro:<id>` forms.
fn called_section(definition_id: &str) -> Option<&str> {
    let rest = definition_id
        .strip_prefix("subgraph:")
        .or_else(|| definition_id.strip_prefix("macro:"))?;
    let rest = rest.strip_prefix("collapsed:").or_else(|| rest.strip_prefix("macro:")).unwrap_or(rest);
    (!rest.is_empty()).then_some(rest)
}

/// Ports from the pins of a node placing a collapsed graph.
fn ports_from_pins(pins: &[blueprint_editor_plugin::Pin]) -> Vec<helio_graph_asset::SectionPort> {
    pins.iter().map(|pin| port_from_type(&pin.id, &pin.data_type.type_name)).collect()
}

fn port_from_type(name: &str, type_name: &str) -> helio_graph_asset::SectionPort {
    helio_graph_asset::SectionPort {
        name: name.to_string(),
        ty: pin_type_of(type_name),
        resource: type_name
            .split_once(':')
            .map(|(_, resource)| resource.to_string())
            .unwrap_or_default(),
    }
}

fn pin_instances(pins: &[PinDesc], direction: DescPinType) -> Vec<PinInstance> {
    pins.iter()
        .map(|pin| PinInstance {
            id: pin.name.clone(),
            pin: DescPin {
                name: pin_label(pin),
                pin_type: direction.clone(),
                data_type: DataType::from_type_str(&pin_type_string(pin)),
                connected_to: Vec::new(),
            },
        })
        .collect()
}

/// One graph of the asset (the main one, or a section) as an editor description.
/// Inside a section (`io`), the entry and exit nodes carry its ports.
fn build_description(
    name: &str,
    nodes: &[NodeInstance],
    connections: &[Connection],
    comments: &[helio_graph_asset::GraphComment],
    registry: &NodeRegistry,
    sections: &[helio_graph_asset::GraphSection],
    io: Option<&helio_graph_asset::GraphSection>,
) -> Result<GraphDescription, String> {
    let mut graph = GraphDescription::new(name);
    let mut pin_kinds: HashMap<(String, String), PinKind> = HashMap::new();

    for node in nodes {
        let section_id = helio_graph_asset::sections::section_id(&node.kind);
        let section = section_id.and_then(|id| sections.iter().find(|s| s.id == id));
        let definition = match section_id {
            Some(id) => format!("{SUBGRAPH_PREFIX}{id}"),
            None => node.kind.clone(),
        };
        let mut instance = DescNode::new(
            &node.id,
            &definition,
            Position { x: node.position[0], y: node.position[1] },
        );
        let pins = if let Some(section) = section {
            Some(helio_graph_asset::sections::section_pins(section))
        } else if let Some(descriptor) = registry.get(&node.kind) {
            let params = descriptor.effective_params(&node.params);
            for (key, value) in &params {
                instance.properties.insert(key.clone(), Value::from(param_string(value)));
            }
            Some(descriptor.pins(&params))
        } else {
            None
        };
        if let Some(pins) = pins {
            for pin in pins.inputs.iter().chain(&pins.outputs) {
                pin_kinds.insert((node.id.clone(), pin.name.clone()), pin.ty.kind);
            }
            instance.inputs = pin_instances(&pins.inputs, DescPinType::Input);
            instance.outputs = pin_instances(&pins.outputs, DescPinType::Output);
        }
        graph.add_node(instance)?;
    }

    if let Some(section) = io {
        let (left, right, top) = if nodes.is_empty() {
            (0.0, 0.0, 0.0)
        } else {
            (
                nodes.iter().map(|n| n.position[0]).fold(f32::MAX, f32::min),
                nodes.iter().map(|n| n.position[0]).fold(f32::MIN, f32::max),
                nodes.iter().map(|n| n.position[1]).fold(f32::MAX, f32::min),
            )
        };
        let entry_pins: Vec<PinDesc> =
            section.inputs.iter().map(helio_graph_asset::SectionPort::to_pin).collect();
        let exit_pins: Vec<PinDesc> =
            section.outputs.iter().map(helio_graph_asset::SectionPort::to_pin).collect();
        let mut entry = DescNode::new(ENTRY_ID, ENTRY_TYPE, Position { x: left - 360.0, y: top });
        entry.outputs = pin_instances(&entry_pins, DescPinType::Output);
        let mut exit = DescNode::new(EXIT_ID, EXIT_TYPE, Position { x: right + 560.0, y: top });
        exit.inputs = pin_instances(&exit_pins, DescPinType::Input);
        for pin in &entry_pins {
            pin_kinds.insert((ENTRY_ID.to_string(), pin.name.clone()), pin.ty.kind);
        }
        for pin in &exit_pins {
            pin_kinds.insert((EXIT_ID.to_string(), pin.name.clone()), pin.ty.kind);
        }
        graph.add_node(entry)?;
        graph.add_node(exit)?;
    }

    let node_name = |id: &str| -> String {
        match id {
            helio_graph_asset::SECTION_ENTRY => ENTRY_ID.to_string(),
            helio_graph_asset::SECTION_EXIT => EXIT_ID.to_string(),
            other => other.to_string(),
        }
    };
    for (index, connection) in connections.iter().enumerate() {
        let (from_node, to_node) = (node_name(&connection.from.node), node_name(&connection.to.node));
        let kind = pin_kinds.get(&(from_node.clone(), connection.from.pin.clone())).copied();
        let wire = DescConnection {
            id: format!("wire{index}"),
            source_node: from_node,
            source_pin: connection.from.pin.clone(),
            target_node: to_node,
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
            log::warn!("render graph `{name}`: dropped wire: {error}");
        }
    }

    graph.comments = comments
        .iter()
        .map(|c| blueprint_graph::BlueprintComment {
            id: c.id.clone(),
            text: c.text.clone(),
            position: (c.position[0], c.position[1]),
            size: (c.size[0], c.size[1]),
            color: c.color,
            contained_node_ids: c.nodes.clone(),
        })
        .collect();
    Ok(graph)
}

/// A render graph as the editor document model: the main graph, and each
/// section as a collapsed graph of its own.
pub fn to_blueprint_asset(asset: &GraphAsset, registry: &NodeRegistry) -> Result<BlueprintAsset, String> {
    let name = if asset.name.is_empty() { "Render Graph" } else { &asset.name };
    let mut graph = build_description(
        name,
        &asset.nodes,
        &asset.connections,
        &asset.comments,
        registry,
        &asset.sections,
        None,
    )?;
    graph.metadata.description = asset.description.clone();

    let mut blueprint = BlueprintAsset::new();
    blueprint.main_graph = graph;
    for section in &asset.sections {
        let inner = build_description(
            &section.name,
            &section.nodes,
            &section.connections,
            &[],
            registry,
            &asset.sections,
            Some(section),
        )?;
        let interface_pins =
            |ports: &[helio_graph_asset::SectionPort]| -> Vec<blueprint_graph::SubGraphPin> {
                ports
                    .iter()
                    .map(|port| {
                        let pin = port.to_pin();
                        blueprint_graph::SubGraphPin {
                            id: port.name.clone(),
                            name: pin_label(&pin),
                            data_type: DataType::from_type_str(&pin_type_string(&pin)),
                            description: None,
                            default_value: None,
                            is_instance_editable: false,
                            category: None,
                        }
                    })
                    .collect()
            };
        let mut subgraph = blueprint_graph::SubGraph::new(&section.id, &section.name);
        subgraph.kind = blueprint_graph::SubGraphKind::Collapsed;
        subgraph.description = "A section of the render graph".to_string();
        subgraph.graph = inner;
        subgraph.interface = blueprint_graph::SubGraphInterface {
            inputs: interface_pins(&section.inputs),
            outputs: interface_pins(&section.outputs),
        };
        subgraph.macro_config.hide_in_palette = true;
        subgraph.macro_config.category = "Collapsed".to_string();
        blueprint.subgraphs.push(subgraph);
    }
    Ok(blueprint)
}

/// The nodes of an editor description, in id order, with the nodes placing
/// collapsed graphs turned into section nodes. Entry and exit nodes are left out.
fn nodes_of(graph: &GraphDescription, registry: &NodeRegistry) -> Vec<NodeInstance> {
    let mut ids: Vec<&String> = graph.nodes.keys().collect();
    ids.sort();
    ids.into_iter()
        .filter(|id| !matches!(graph.nodes[*id].node_type.as_str(), ENTRY_TYPE | EXIT_TYPE))
        .map(|id| {
            let node = &graph.nodes[id];
            let kind = called_section(&node.node_type)
                .map_or_else(|| node.node_type.clone(), helio_graph_asset::sections::section_kind);
            let params = registry.get(&kind).map_or_else(Params::new, |descriptor| {
                params_from_text(
                    descriptor,
                    node.properties.iter().map(|(key, value)| (key, param_string(value))),
                )
            });
            NodeInstance {
                id: node.id.clone(),
                kind,
                params,
                position: [node.position.x, node.position.y],
            }
        })
        .collect()
}

/// The wires of an editor description; the entry and exit nodes of a collapsed
/// graph become the section own `$entry` and `$exit`.
fn connections_of(graph: &GraphDescription) -> Vec<Connection> {
    let name_of = |id: &String| -> String {
        match graph.nodes.get(id).map(|n| n.node_type.as_str()) {
            Some(ENTRY_TYPE) => helio_graph_asset::SECTION_ENTRY.to_string(),
            Some(EXIT_TYPE) => helio_graph_asset::SECTION_EXIT.to_string(),
            _ => id.clone(),
        }
    };
    graph
        .connections
        .iter()
        .map(|c| Connection {
            from: PinRef::new(name_of(&c.source_node), &c.source_pin),
            to: PinRef::new(name_of(&c.target_node), &c.target_pin),
        })
        .collect()
}

fn comments_of(graph: &GraphDescription) -> Vec<helio_graph_asset::GraphComment> {
    graph
        .comments
        .iter()
        .map(|c| helio_graph_asset::GraphComment {
            id: c.id.clone(),
            text: c.text.clone(),
            position: [c.position.0, c.position.1],
            size: [c.size.0, c.size.1],
            color: c.color,
            nodes: c.contained_node_ids.clone(),
        })
        .collect()
}

/// The inverse of [`to_blueprint_asset`]: each collapsed graph is a section.
pub fn from_blueprint_asset(blueprint: &BlueprintAsset, registry: &NodeRegistry) -> GraphAsset {
    let graph = &blueprint.main_graph;
    let ports = |pins: &[blueprint_graph::SubGraphPin]| -> Vec<helio_graph_asset::SectionPort> {
        pins.iter().map(|pin| port_from_type(&pin.id, &pin.data_type.to_string())).collect()
    };
    let sections = blueprint
        .subgraphs
        .iter()
        .filter(|subgraph| subgraph.kind == blueprint_graph::SubGraphKind::Collapsed)
        .map(|subgraph| helio_graph_asset::GraphSection {
            id: subgraph.id.clone(),
            name: subgraph.name.clone(),
            inputs: ports(&subgraph.interface.inputs),
            outputs: ports(&subgraph.interface.outputs),
            nodes: nodes_of(&subgraph.graph, registry),
            connections: connections_of(&subgraph.graph),
        })
        .collect();
    GraphAsset {
        name: graph.metadata.name.clone(),
        description: graph.metadata.description.clone(),
        nodes: nodes_of(graph, registry),
        connections: connections_of(graph),
        comments: comments_of(graph),
        sections,
        ..GraphAsset::default()
    }
}

/// Wires through reroute nodes (the editor tidying device) joined end to end, and
/// the reroutes removed.
fn bypass_reroutes(nodes: &mut Vec<NodeInstance>, connections: &mut Vec<Connection>) {
    let reroutes: HashSet<String> =
        nodes.iter().filter(|n| n.kind == REROUTE).map(|n| n.id.clone()).collect();
    if reroutes.is_empty() {
        return;
    }
    let mut joined = Vec::new();
    for wire in connections.iter().filter(|w| !reroutes.contains(&w.from.node)) {
        if !reroutes.contains(&wire.to.node) {
            joined.push(wire.clone());
            continue;
        }
        let mut pending = vec![wire.to.node.clone()];
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(reroute) = pending.pop() {
            if !seen.insert(reroute.clone()) {
                continue; // a reroute cycle
            }
            for next in connections.iter().filter(|w| w.from.node == reroute) {
                if reroutes.contains(&next.to.node) {
                    pending.push(next.to.node.clone());
                } else {
                    joined.push(Connection { from: wire.from.clone(), to: next.to.clone() });
                }
            }
        }
    }
    nodes.retain(|n| n.kind != REROUTE);
    *connections = joined;
}

/// The asset the editor document describes, ready to check and build: sections
/// kept, reroutes removed.
pub fn buildable_asset(blueprint: &BlueprintAsset, registry: &NodeRegistry) -> GraphAsset {
    let mut asset = from_blueprint_asset(blueprint, registry);
    bypass_reroutes(&mut asset.nodes, &mut asset.connections);
    for section in &mut asset.sections {
        bypass_reroutes(&mut section.nodes, &mut section.connections);
    }
    asset
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
                    kind: called_section(&n.definition_id)
                        .map_or_else(|| n.definition_id.clone(), helio_graph_asset::sections::section_kind),
                    params,
                    position: n.position,
                }
            })
            .collect(),
        connections,
        ..GraphAsset::default()
    }
}

/// The asset the editor's graph describes.
pub fn live_asset(graph: &blueprint_editor_plugin::BlueprintGraph) -> GraphAsset {
    let nodes: Vec<LiveNode> = graph
        .nodes
        .iter()
        .map(|node| LiveNode {
            id: node.id.clone(),
            definition_id: node.definition_id.clone(),
            position: [node.position.x, node.position.y],
            properties: node.properties.clone(),
        })
        .collect();
    let wires: Vec<LiveWire> = graph
        .connections
        .iter()
        .map(|c| LiveWire {
            source_node: c.source_node.clone(),
            source_pin: c.source_pin.clone(),
            target_node: c.target_node.clone(),
            target_pin: c.target_pin.clone(),
        })
        .collect();
    asset_from_live("", &nodes, &wires, registry())
}

/// Arranges the editor's graph: positions every node and replaces the lane
/// comments, keeping comments the author made. Reroute nodes are the editor's own
/// and keep where they are.
pub fn auto_layout_graph(graph: &mut blueprint_editor_plugin::BlueprintGraph) -> bool {
    let mut asset = live_asset(graph);
    // Collapsed graphs are sections; their pins are what the nodes placing them show.
    asset.sections = graph
        .nodes
        .iter()
        .filter_map(|node| {
            called_section(&node.definition_id).map(|id| helio_graph_asset::GraphSection {
                id: id.to_string(),
                name: node.title.clone(),
                inputs: ports_from_pins(&node.inputs),
                outputs: ports_from_pins(&node.outputs),
                nodes: Vec::new(),
                connections: Vec::new(),
            })
        })
        .collect();
    helio_graph_asset::auto_layout(&mut asset, registry());

    let positions: HashMap<String, [f32; 2]> =
        asset.nodes.iter().map(|n| (n.id.clone(), n.position)).collect();
    for node in graph.nodes.iter_mut() {
        if let Some(position) = positions.get(&node.id) {
            node.position = gpui::Point::new(position[0], position[1]);
        }
    }

    let lanes: Vec<blueprint_editor_plugin::BlueprintComment> = asset
        .comments
        .iter()
        .map(|c| blueprint_editor_plugin::BlueprintComment {
            id: c.id.clone(),
            text: c.text.clone(),
            position: gpui::Point::new(c.position[0], c.position[1]),
            size: gpui::Size::new(c.size[0], c.size[1]),
            color: gpui::Hsla::from(gpui::Rgba {
                r: c.color[0],
                g: c.color[1],
                b: c.color[2],
                a: c.color[3],
            }),
            contained_node_ids: c.nodes.clone(),
            is_selected: false,
            color_picker_state: None,
        })
        .collect();
    graph.comments.with_mut(|comments| {
        comments.retain(|c| !c.id.starts_with(helio_graph_asset::LANE_PREFIX));
        comments.extend(lanes);
    });
    true
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
            name: pin_label(pin),
            pin_type: direction.clone(),
            data_type: blueprint_editor_plugin::PinDataType::from_type_str(pin_type_string(pin)),
        })
        .collect()
}

/// The title of a Get or Set Buffer node: what it does and to which buffer
/// (`Get pre_aa`). `None` for any other node or a node without a buffer yet.
pub fn buffer_title(
    definition_id: &str,
    properties: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let verb = match definition_id {
        helio_graph_asset::core_nodes::BUFFER_GET => "Get",
        helio_graph_asset::core_nodes::BUFFER_SET => "Set",
        _ => return None,
    };
    let (_, name) = helio_graph_asset::buffers::parse_entry(properties.get("buffer")?)?;
    Some(format!("{verb} {name}"))
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
        if let Some(title) = buffer_title(&node.definition_id, &node.properties) {
            node.title = title;
        }
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
    use helio_default_passes::graphs::{base_registry, deferred_graph_asset};
    use helio_graph_asset::Probe;

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
        let registry = base_registry();
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
        let registry = base_registry();
        let asset = deferred_graph_asset(&base_registry());
        let blueprint = to_blueprint_asset(&asset, &registry).unwrap();
        assert_eq!(blueprint.main_graph.nodes.len(), asset.nodes.len());
        assert_eq!(blueprint.main_graph.connections.len(), asset.connections.len());

        let back = from_blueprint_asset(&blueprint, &registry);
        let mut expected = asset.clone();
        expected.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(back.nodes, expected.nodes);
        assert_eq!(back.connections, asset.connections);

        // Each section is a collapsed graph and comes back as the same section.
        assert!(!asset.sections.is_empty());
        assert_eq!(blueprint.subgraphs.len(), asset.sections.len());
        assert_eq!(back.sections.len(), asset.sections.len());
        for (section, restored) in asset.sections.iter().zip(&back.sections) {
            assert_eq!(section.name, restored.name);
            assert_eq!(section.inputs, restored.inputs);
            assert_eq!(section.outputs, restored.outputs);
            assert_eq!(section.connections, restored.connections);
            let mut nodes = section.nodes.clone();
            nodes.sort_by(|a, b| a.id.cmp(&b.id));
            assert_eq!(nodes, restored.nodes);
        }
        assert!(problems(&back, &registry).is_empty());
    }

    #[test]
    fn edited_parameters_round_trip_as_their_declared_types() {
        let registry = base_registry();
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
        let registry = base_registry();
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
        let registry = base_registry();
        let found = problems(&GraphAsset::default(), &registry);
        assert!(found.iter().any(|p| p.contains("Output")), "{found:?}");
    }
}
