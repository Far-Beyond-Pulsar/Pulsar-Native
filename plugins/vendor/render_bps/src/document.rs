//! The render graph document: what the editor edits, and how a `.rendergraph`
//! asset becomes it and back.
//!
//! There is one editable document, made of Graphy `GraphDescription`s: the main
//! graph and one per section. The canvas owns them while the editor is open; a
//! [`DocRef`] borrows them (with the few things a graph description has no slot
//! for) so saving, checking and previewing read the document directly. The asset
//! is only ever built from a document: at load it is converted once into the
//! document ([`RenderDocument::from_asset`]); at save, validation and preview a
//! `GraphAsset` is produced from it because that is what Helio's `validate` and
//! compiler take. It is a projection that is never edited.

use std::collections::{HashMap, HashSet};

use graphy::core::GraphComment;
use graphy::{Connection, ConnectionType, GraphDescription, NodeInstance, PinInstance, PinType, Position};
use helio_graph_asset::{
    sections, GraphAsset, GraphSection, NodeDescriptor, NodeInstance as AssetNode, NodePins,
    NodeRegistry, Params, PinDesc, PinRef, SectionPort,
};
use serde_json::Value;

use crate::model::{self, pin_instances, pin_type_of};

/// Ids of the entry and exit nodes inside a section's graph.
pub const ENTRY_ID: &str = "section_entry";
pub const EXIT_ID: &str = "section_exit";
/// Their node types.
pub const ENTRY_TYPE: &str = "section.entry";
pub const EXIT_TYPE: &str = "section.exit";
/// The editor's tidying device; never part of a built graph.
pub const REROUTE: &str = "reroute";
/// The pin type of a pin on a node this build has no definition for.
const OPAQUE_PIN: &str = "?";
/// Reroutes are saved with the type string of the wire they carry.
const REROUTE_TYPE: &str = "type";

/// One graph of the document with what it keeps outside its description.
pub struct GraphRef<'a> {
    /// `""` for the main graph, the section id otherwise.
    pub id: &'a str,
    pub name: &'a str,
    pub graph: &'a GraphDescription,
    /// Node ids in authoring order (which breaks ties when the graph is built).
    /// Nodes not listed follow, by id.
    pub order: &'a [String],
    /// What each comment contains, by comment index, when it was worked out from
    /// the laid-out geometry (the graph description keeps the last placed set).
    pub contained: Option<&'a [Vec<String>]>,
}

/// A borrowed render graph document.
pub struct DocRef<'a> {
    pub format_version: u32,
    pub name: &'a str,
    pub description: &'a str,
    pub main: GraphRef<'a>,
    pub sections: Vec<GraphRef<'a>>,
}

/// A section of an owned document.
pub struct SectionDoc {
    pub id: String,
    pub name: String,
    pub graph: GraphDescription,
    pub order: Vec<String>,
}

/// An owned document: a freshly decoded asset, before an editor takes the graphs.
pub struct RenderDocument {
    pub format_version: u32,
    pub name: String,
    pub description: String,
    pub main: GraphDescription,
    pub main_order: Vec<String>,
    pub sections: Vec<SectionDoc>,
}

impl RenderDocument {
    pub fn as_ref(&self) -> DocRef<'_> {
        DocRef {
            format_version: self.format_version,
            name: &self.name,
            description: &self.description,
            main: GraphRef {
                id: "",
                name: &self.name,
                graph: &self.main,
                order: &self.main_order,
                contained: None,
            },
            sections: self
                .sections
                .iter()
                .map(|s| GraphRef {
                    id: &s.id,
                    name: &s.name,
                    graph: &s.graph,
                    order: &s.order,
                    contained: None,
                })
                .collect(),
        }
    }

    /// The document an asset describes. The one conversion in, at load.
    pub fn from_asset(asset: &GraphAsset, registry: &NodeRegistry) -> Self {
        let name = if asset.name.is_empty() {
            "Render Graph"
        } else {
            &asset.name
        };
        let mut main = build_graph(
            name,
            &asset.nodes,
            &asset.connections,
            &asset.comments,
            registry,
            &asset.sections,
            None,
        );
        main.metadata.description = asset.description.clone();
        let sections = asset
            .sections
            .iter()
            .map(|section| SectionDoc {
                id: section.id.clone(),
                name: section.name.clone(),
                graph: build_graph(
                    &section.name,
                    &section.nodes,
                    &section.connections,
                    &[],
                    registry,
                    &asset.sections,
                    Some(section),
                ),
                order: section.nodes.iter().map(|n| n.id.clone()).collect(),
            })
            .collect();
        Self {
            format_version: asset.format_version,
            name: asset.name.clone(),
            description: asset.description.clone(),
            main,
            main_order: asset.nodes.iter().map(|n| n.id.clone()).collect(),
            sections,
        }
    }
}

impl DocRef<'_> {
    /// The asset to write: every node, reroutes and sections included.
    pub fn to_asset(&self, registry: &NodeRegistry) -> GraphAsset {
        GraphAsset {
            format_version: self.format_version,
            name: self.name.to_string(),
            description: self.description.to_string(),
            nodes: nodes_of(&self.main, registry),
            connections: connections_of(self.main.graph),
            comments: comments_of(self.main.graph, self.main.contained),
            sections: self
                .sections
                .iter()
                .map(|section| GraphSection {
                    id: section.id.to_string(),
                    name: section.name.to_string(),
                    inputs: ports_of(section.graph, ENTRY_ID, true),
                    outputs: ports_of(section.graph, EXIT_ID, false),
                    nodes: nodes_of(section, registry),
                    connections: connections_of(section.graph),
                })
                .collect(),
        }
    }

    /// The asset to check and build: as [`to_asset`](Self::to_asset), with the
    /// editor's reroutes removed and the wires through them joined end to end.
    pub fn buildable_asset(&self, registry: &NodeRegistry) -> GraphAsset {
        let mut asset = self.to_asset(registry);
        bypass_reroutes(&mut asset.nodes, &mut asset.connections);
        for section in &mut asset.sections {
            bypass_reroutes(&mut section.nodes, &mut section.connections);
        }
        asset
    }
}

// ── Asset -> document ────────────────────────────────────────────────────────

fn build_graph(
    name: &str,
    nodes: &[AssetNode],
    connections: &[helio_graph_asset::Connection],
    comments: &[helio_graph_asset::GraphComment],
    registry: &NodeRegistry,
    sections: &[GraphSection],
    io: Option<&GraphSection>,
) -> GraphDescription {
    let mut graph = GraphDescription::new(name);
    let mut opaque: HashSet<String> = HashSet::new();

    for node in nodes {
        let position = Position::new(f64::from(node.position[0]), f64::from(node.position[1]));
        let mut instance = NodeInstance::new(&node.id, &node.kind, position);
        if let Some(section) = sections::section_id(&node.kind)
            .and_then(|id| sections.iter().find(|s| s.id == id))
        {
            let pins = sections::section_pins(section);
            instance.inputs = pin_instances(&pins.inputs, PinType::Input);
            instance.outputs = pin_instances(&pins.outputs, PinType::Output);
        } else if node.kind == REROUTE {
            let type_name = node
                .params
                .get(REROUTE_TYPE)
                .and_then(Value::as_str)
                .unwrap_or(OPAQUE_PIN);
            instance.add_input_pin("in", model::data_type(type_name));
            instance.add_output_pin("out", model::data_type(type_name));
        } else if let Some(descriptor) = registry.get(&node.kind) {
            instance.properties = model::properties_of(&node.params);
            let pins = descriptor.pins(&node.params);
            instance.inputs = pin_instances(&pins.inputs, PinType::Input);
            instance.outputs = pin_instances(&pins.outputs, PinType::Output);
        } else {
            // A node this build has no definition for keeps everything it has, and
            // gets a pin for each wire it carries so none of them is lost.
            instance.properties = model::properties_of(&node.params);
            opaque.insert(node.id.clone());
        }
        graph.add_node(instance);
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
        let entry_pins: Vec<PinDesc> = section.inputs.iter().map(SectionPort::to_pin).collect();
        let exit_pins: Vec<PinDesc> = section.outputs.iter().map(SectionPort::to_pin).collect();
        let mut entry = NodeInstance::new(
            ENTRY_ID,
            ENTRY_TYPE,
            Position::new(f64::from(left - 360.0), f64::from(top)),
        );
        entry.outputs = pin_instances(&entry_pins, PinType::Output);
        let mut exit = NodeInstance::new(
            EXIT_ID,
            EXIT_TYPE,
            Position::new(f64::from(right + 560.0), f64::from(top)),
        );
        exit.inputs = pin_instances(&exit_pins, PinType::Input);
        graph.add_node(entry);
        graph.add_node(exit);
    }

    let node_name = |id: &str| -> String {
        match id {
            helio_graph_asset::SECTION_ENTRY if io.is_some() => ENTRY_ID.to_string(),
            helio_graph_asset::SECTION_EXIT if io.is_some() => EXIT_ID.to_string(),
            other => other.to_string(),
        }
    };
    for connection in connections {
        let (from_node, to_node) = (
            node_name(&connection.from.node),
            node_name(&connection.to.node),
        );
        // Pins of a node without a definition are made from its wires.
        for (node, pin, output) in [
            (&from_node, &connection.from.pin, true),
            (&to_node, &connection.to.pin, false),
        ] {
            if !opaque.contains(node) {
                continue;
            }
            if let Some(instance) = graph.nodes.get_mut(node) {
                let pins = if output {
                    &mut instance.outputs
                } else {
                    &mut instance.inputs
                };
                if !pins.iter().any(|p| &p.id == pin) {
                    if output {
                        instance.add_output_pin(pin.clone(), model::data_type(OPAQUE_PIN));
                    } else {
                        instance.add_input_pin(pin.clone(), model::data_type(OPAQUE_PIN));
                    }
                }
            }
        }
        let exec = graph
            .nodes
            .get(&from_node)
            .and_then(|n| n.outputs.iter().find(|p| p.id == connection.from.pin))
            .map(|p| p.data_type().is_execution());
        let has_target = graph
            .nodes
            .get(&to_node)
            .is_some_and(|n| n.inputs.iter().any(|p| p.id == connection.to.pin));
        let Some(exec) = exec.filter(|_| has_target) else {
            // A wire to a pin the node no longer has is dropped here; validation
            // of the saved asset reports the same thing.
            log::warn!(
                "render graph `{name}`: dropped wire {}.{} -> {}.{}",
                connection.from.node,
                connection.from.pin,
                connection.to.node,
                connection.to.pin
            );
            continue;
        };
        graph.add_connection(Connection::new(
            &from_node,
            &connection.from.pin,
            &to_node,
            &connection.to.pin,
            if exec {
                ConnectionType::Execution
            } else {
                ConnectionType::Data
            },
        ));
    }

    graph.comments = comments
        .iter()
        .map(|c| {
            let mut comment = GraphComment::new(
                c.text.clone(),
                Position::new(f64::from(c.position[0]), f64::from(c.position[1])),
                (f64::from(c.size[0]), f64::from(c.size[1])),
            );
            comment.id = c.id.clone();
            comment.color = c.color;
            comment.contained_node_ids = c.nodes.clone();
            comment
        })
        .collect();
    graph
}

// ── Document -> asset ────────────────────────────────────────────────────────

/// The nodes of a graph in authoring order, then any added since by id. Entry
/// and exit nodes are left out: they are the section's ports.
fn nodes_of(graph: &GraphRef<'_>, registry: &NodeRegistry) -> Vec<AssetNode> {
    let mut listed: HashSet<&str> = HashSet::new();
    let mut ids: Vec<&str> = graph
        .order
        .iter()
        .filter(|id| graph.graph.nodes.contains_key(*id) && listed.insert(id.as_str()))
        .map(String::as_str)
        .collect();
    let mut added: Vec<&str> = graph
        .graph
        .nodes
        .keys()
        .map(String::as_str)
        .filter(|id| !listed.contains(id))
        .collect();
    added.sort_unstable();
    ids.extend(added);

    ids.into_iter()
        .filter_map(|id| {
            let node = &graph.graph.nodes[id];
            if matches!(node.node_type.as_str(), ENTRY_TYPE | EXIT_TYPE) {
                return None;
            }
            Some(asset_node(node, registry.get(&node.node_type).map(|d| &**d)))
        })
        .collect()
}

fn asset_node(node: &NodeInstance, descriptor: Option<&NodeDescriptor>) -> AssetNode {
    let params = if node.node_type == REROUTE {
        let type_name = node
            .inputs
            .first()
            .map_or_else(|| OPAQUE_PIN.to_string(), |p| model::type_name(p.data_type()));
        Params::from_iter([(REROUTE_TYPE.to_string(), Value::from(type_name))])
    } else if sections::section_id(&node.node_type).is_some() {
        Params::new()
    } else {
        model::params_of(descriptor, node)
    };
    AssetNode {
        id: node.id.clone(),
        kind: node.node_type.clone(),
        params,
        position: [node.position.x as f32, node.position.y as f32],
    }
}

/// The wires of a graph; the entry and exit nodes of a section become the
/// section's own `$entry` and `$exit`.
fn connections_of(graph: &GraphDescription) -> Vec<helio_graph_asset::Connection> {
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
        .map(|c| helio_graph_asset::Connection {
            from: PinRef::new(name_of(&c.source_node), &c.source_pin),
            to: PinRef::new(name_of(&c.target_node), &c.target_pin),
        })
        .collect()
}

fn comments_of(
    graph: &GraphDescription,
    contained: Option<&[Vec<String>]>,
) -> Vec<helio_graph_asset::GraphComment> {
    graph
        .comments
        .iter()
        .enumerate()
        .map(|(index, c)| helio_graph_asset::GraphComment {
            id: c.id.clone(),
            text: c.text.clone(),
            position: [c.position.x as f32, c.position.y as f32],
            size: [c.size.0 as f32, c.size.1 as f32],
            color: c.color,
            nodes: contained
                .and_then(|all| all.get(index))
                .unwrap_or(&c.contained_node_ids)
                .clone(),
        })
        .collect()
}

/// The ports of a section: the pins of its entry node (`inputs`) or exit node.
fn ports_of(graph: &GraphDescription, node: &str, entry: bool) -> Vec<SectionPort> {
    let Some(node) = graph.nodes.get(node) else {
        return Vec::new();
    };
    let pins: &[PinInstance] = if entry { &node.outputs } else { &node.inputs };
    pins.iter()
        .map(|pin| port_from_type(&pin.id, &model::type_name(pin.data_type())))
        .collect()
}

pub fn port_from_type(name: &str, type_name: &str) -> SectionPort {
    SectionPort {
        name: name.to_string(),
        ty: pin_type_of(type_name),
        resource: type_name
            .split_once(':')
            .map(|(_, resource)| resource.to_string())
            .unwrap_or_default(),
    }
}

/// Wires through reroute nodes joined end to end, and the reroutes removed.
fn bypass_reroutes(nodes: &mut Vec<AssetNode>, connections: &mut Vec<helio_graph_asset::Connection>) {
    let reroutes: HashSet<String> = nodes
        .iter()
        .filter(|n| n.kind == REROUTE)
        .map(|n| n.id.clone())
        .collect();
    if reroutes.is_empty() {
        return;
    }
    let mut joined = Vec::new();
    for wire in connections
        .iter()
        .filter(|w| !reroutes.contains(&w.from.node))
    {
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
                    joined.push(helio_graph_asset::Connection {
                        from: wire.from.clone(),
                        to: next.to.clone(),
                    });
                }
            }
        }
    }
    nodes.retain(|n| n.kind != REROUTE);
    *connections = joined;
}

/// One graph as the asset the layout reads: its nodes and wires with reroutes
/// joined, and a section per section node (its pins are what the node shows).
pub fn layout_asset(graph: &GraphDescription, registry: &NodeRegistry) -> GraphAsset {
    let mut ids: Vec<&String> = graph.nodes.keys().collect();
    ids.sort();
    let mut asset = GraphAsset {
        nodes: ids
            .iter()
            .filter(|id| {
                !matches!(graph.nodes[**id].node_type.as_str(), ENTRY_TYPE | EXIT_TYPE)
            })
            .map(|id| {
                let node = &graph.nodes[*id];
                asset_node(node, registry.get(&node.node_type).map(|d| &**d))
            })
            .collect(),
        connections: connections_of(graph),
        ..GraphAsset::default()
    };
    bypass_reroutes(&mut asset.nodes, &mut asset.connections);
    asset.sections = graph
        .nodes
        .values()
        .filter_map(|node| {
            sections::section_id(&node.node_type).map(|id| GraphSection {
                id: id.to_string(),
                name: id.to_string(),
                inputs: ports_from_pins(&node.inputs),
                outputs: ports_from_pins(&node.outputs),
                nodes: Vec::new(),
                connections: Vec::new(),
            })
        })
        .collect();
    asset
}

fn ports_from_pins(pins: &[PinInstance]) -> Vec<SectionPort> {
    pins.iter()
        .map(|pin| port_from_type(&pin.id, &model::type_name(pin.data_type())))
        .collect()
}

/// The pins of the section `id`, as its placeholder node shows them: the
/// document's section graph is the one source of its ports.
pub fn section_node_pins(section: &GraphDescription) -> NodePins {
    NodePins {
        inputs: ports_of(section, ENTRY_ID, true)
            .iter()
            .map(SectionPort::to_pin)
            .collect(),
        outputs: ports_of(section, EXIT_ID, false)
            .iter()
            .map(SectionPort::to_pin)
            .collect(),
    }
}

/// The id -> name table of an owned document's sections.
pub fn section_names(doc: &DocRef<'_>) -> HashMap<String, String> {
    doc.sections
        .iter()
        .map(|s| (s.id.to_string(), s.name.to_string()))
        .collect()
}


// ── Text ─────────────────────────────────────────────────────────────────────

/// Reads a `.rendergraph` file into a document. A new file is a marker the
/// editor expands: the default deferred pipeline, wired from the pins its passes
/// declare (known only once the pin probe has run).
pub fn decode(text: &str, registry: &NodeRegistry) -> Result<RenderDocument, String> {
    let marker: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid render graph: {error}"))?;
    let is_empty = marker
        .get("nodes")
        .and_then(Value::as_array)
        .map_or(true, |nodes| nodes.is_empty());
    let asset = if is_empty && marker.get("template").and_then(Value::as_str) == Some("deferred") {
        helio_default_passes::graphs::deferred_graph_asset(registry)
    } else {
        GraphAsset::from_json(text)?
    };
    Ok(RenderDocument::from_asset(&asset, registry))
}

/// The text of a document, as written to its file.
pub fn encode(doc: &DocRef<'_>, registry: &NodeRegistry) -> String {
    doc.to_asset(registry).to_json()
}
