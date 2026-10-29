//! Collapse nodes into a section, and expand a section back into its nodes.
//!
//! A section is organisation only (`helio_graph_asset::sections`): collapsing
//! moves nodes into a graph of their own and leaves one node in their place, and
//! the wires that crossed the boundary become the section's ports; expanding is
//! the reverse. Both are pure operations on Graphy documents that return edit
//! commands for the graph they act on.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use graphy::{
    Connection, ConnectionType, GraphCommand, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position,
};
use helio_graph_asset::sections;

use crate::document::{ENTRY_ID, ENTRY_TYPE, EXIT_ID, EXIT_TYPE};

/// A collapse: the new section's graph, the node that places it and the edits to
/// the graph the nodes came from.
pub struct Collapsed {
    pub id: String,
    pub name: String,
    pub graph: GraphDescription,
    /// Authoring order of the section's nodes.
    pub order: Vec<String>,
    /// The node placing the section.
    pub node: NodeInstance,
    /// Removes the nodes, adds `node` and wires it where the nodes were wired.
    pub commands: Vec<GraphCommand>,
}

fn kind_of(data_type: &graphy::DataType) -> ConnectionType {
    if data_type.is_execution() {
        ConnectionType::Execution
    } else {
        ConnectionType::Data
    }
}

fn pin(id: &str, label: &str, data_type: &graphy::DataType, direction: PinType) -> PinInstance {
    PinInstance {
        id: id.to_string(),
        pin: Pin {
            id: id.to_string(),
            name: label.to_string(),
            data_type: data_type.clone(),
            pin_type: direction,
        },
    }
}

/// `base`, or `base_2`, `base_3`… if that is taken.
fn unique(base: &str, taken: &mut HashSet<String>) -> String {
    let mut name = base.to_string();
    let mut n = 1;
    while !taken.insert(name.clone()) {
        n += 1;
        name = format!("{base}_{n}");
    }
    name
}

/// A port made from the wires crossing the boundary on one pin.
struct Port {
    name: String,
    label: String,
    data_type: graphy::DataType,
}

/// Collapses `selected` (nodes of `graph`) into a new section. `taken` holds the
/// ids of the sections that exist.
pub fn collapse(
    graph: &GraphDescription,
    selected: &[String],
    taken: &HashSet<String>,
) -> Result<Collapsed, String> {
    let inside: BTreeSet<&str> = selected
        .iter()
        .map(String::as_str)
        .filter(|id| {
            graph
                .nodes
                .get(*id)
                .is_some_and(|n| !matches!(n.node_type.as_str(), ENTRY_TYPE | EXIT_TYPE))
        })
        .collect();
    if inside.is_empty() {
        return Err("Select the nodes to collapse into a section".to_string());
    }

    let number = (1..)
        .find(|n| !taken.contains(&format!("section_{n}")))
        .expect("section numbers do not run out");
    let id = format!("section_{number}");
    let name = format!("Section {number}");

    // Ports: one per outer pin feeding in, one per inner pin feeding out.
    let mut in_ports: BTreeMap<(String, String), Port> = BTreeMap::new();
    let mut out_ports: BTreeMap<(String, String), Port> = BTreeMap::new();
    let mut in_names = HashSet::new();
    let mut out_names = HashSet::new();
    for c in &graph.connections {
        let (from_in, to_in) = (
            inside.contains(c.source_node.as_str()),
            inside.contains(c.target_node.as_str()),
        );
        if !from_in && to_in {
            let key = (c.source_node.clone(), c.source_pin.clone());
            if !in_ports.contains_key(&key) {
                let Some(target) = graph
                    .nodes
                    .get(&c.target_node)
                    .and_then(|n| n.inputs.iter().find(|p| p.id == c.target_pin))
                else {
                    continue;
                };
                in_ports.insert(
                    key,
                    Port {
                        name: unique(&c.source_pin, &mut in_names),
                        label: target.pin.name.clone(),
                        data_type: target.data_type().clone(),
                    },
                );
            }
        } else if from_in && !to_in {
            let key = (c.source_node.clone(), c.source_pin.clone());
            if !out_ports.contains_key(&key) {
                let Some(source) = graph
                    .nodes
                    .get(&c.source_node)
                    .and_then(|n| n.outputs.iter().find(|p| p.id == c.source_pin))
                else {
                    continue;
                };
                out_ports.insert(
                    key,
                    Port {
                        name: unique(&c.source_pin, &mut out_names),
                        label: source.pin.name.clone(),
                        data_type: source.data_type().clone(),
                    },
                );
            }
        }
    }

    // The section's graph: the nodes, their wires, and entry and exit nodes
    // carrying the ports.
    let mut section = GraphDescription::new(&name);
    let mut order = Vec::new();
    let positions: Vec<(f64, f64)> = inside
        .iter()
        .map(|id| (graph.nodes[*id].position.x, graph.nodes[*id].position.y))
        .collect();
    for node_id in &inside {
        section.add_node(graph.nodes[*node_id].clone());
        order.push((*node_id).to_string());
    }
    let left = positions.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let right = positions.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let top = positions.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let bottom = positions.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let mut entry = NodeInstance::new(ENTRY_ID, ENTRY_TYPE, Position::new(left - 360.0, top));
    entry.outputs = in_ports
        .values()
        .map(|p| pin(&p.name, &p.label, &p.data_type, PinType::Output))
        .collect();
    let mut exit = NodeInstance::new(EXIT_ID, EXIT_TYPE, Position::new(right + 560.0, top));
    exit.inputs = out_ports
        .values()
        .map(|p| pin(&p.name, &p.label, &p.data_type, PinType::Input))
        .collect();
    section.add_node(entry);
    section.add_node(exit);
    for c in &graph.connections {
        let (from_in, to_in) = (
            inside.contains(c.source_node.as_str()),
            inside.contains(c.target_node.as_str()),
        );
        match (from_in, to_in) {
            (true, true) => section.add_connection(c.clone()),
            (false, true) => {
                if let Some(port) = in_ports.get(&(c.source_node.clone(), c.source_pin.clone())) {
                    section.add_connection(Connection::new(
                        ENTRY_ID,
                        &port.name,
                        &c.target_node,
                        &c.target_pin,
                        c.connection_type,
                    ));
                }
            }
            (true, false) => {
                if let Some(port) = out_ports.get(&(c.source_node.clone(), c.source_pin.clone())) {
                    // Several outer targets share one port: one inner wire.
                    let wire = Connection::new(
                        &c.source_node,
                        &c.source_pin,
                        EXIT_ID,
                        &port.name,
                        c.connection_type,
                    );
                    if !section.connections.iter().any(|w| {
                        w.source_node == wire.source_node
                            && w.source_pin == wire.source_pin
                            && w.target_node == wire.target_node
                            && w.target_pin == wire.target_pin
                    }) {
                        section.add_connection(wire);
                    }
                }
            }
            (false, false) => {}
        }
    }

    // The node placing it, where the nodes were.
    let node_id = uuid::Uuid::new_v4().to_string();
    let mut node = NodeInstance::new(
        &node_id,
        sections::section_kind(&id),
        Position::new((left + right) / 2.0, (top + bottom) / 2.0),
    );
    node.inputs = in_ports
        .values()
        .map(|p| pin(&p.name, &p.label, &p.data_type, PinType::Input))
        .collect();
    node.outputs = out_ports
        .values()
        .map(|p| pin(&p.name, &p.label, &p.data_type, PinType::Output))
        .collect();

    let mut commands: Vec<GraphCommand> = inside
        .iter()
        .map(|id| GraphCommand::RemoveNode { id: (*id).to_string() })
        .collect();
    commands.push(GraphCommand::AddNode(node.clone()));
    let mut seen_in = HashSet::new();
    for c in &graph.connections {
        let (from_in, to_in) = (
            inside.contains(c.source_node.as_str()),
            inside.contains(c.target_node.as_str()),
        );
        if !from_in && to_in {
            // Every outer wire into the section ends at the section's input port.
            let Some(port) = in_ports.get(&(c.source_node.clone(), c.source_pin.clone())) else {
                continue;
            };
            if seen_in.insert(port.name.clone()) {
                commands.push(GraphCommand::Connect(Connection::new(
                    &c.source_node,
                    &c.source_pin,
                    &node_id,
                    &port.name,
                    kind_of(&port.data_type),
                )));
            }
        } else if from_in && !to_in {
            let Some(port) = out_ports.get(&(c.source_node.clone(), c.source_pin.clone())) else {
                continue;
            };
            commands.push(GraphCommand::Connect(Connection::new(
                &node_id,
                &port.name,
                &c.target_node,
                &c.target_pin,
                c.connection_type,
            )));
        }
    }

    Ok(Collapsed {
        id,
        name,
        graph: section,
        order,
        node,
        commands,
    })
}

/// Edits putting the nodes of `section` back in `graph` in place of the node
/// `node_id` that placed it, joining the wires across its ports. The nodes keep
/// their ids, so a section placed twice cannot be expanded.
pub fn expand(
    graph: &GraphDescription,
    node_id: &str,
    section: &GraphDescription,
) -> Result<Vec<GraphCommand>, String> {
    if !graph
        .nodes
        .get(node_id)
        .is_some_and(|n| sections::section_id(&n.node_type).is_some())
    {
        return Err("Select a section to expand".to_string());
    }
    let is_type = |id: &str, ty: &str| section.nodes.get(id).is_some_and(|n| n.node_type == ty);
    let inner: Vec<&NodeInstance> = {
        let mut nodes: Vec<&NodeInstance> = section
            .nodes
            .values()
            .filter(|n| !matches!(n.node_type.as_str(), ENTRY_TYPE | EXIT_TYPE))
            .collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        nodes
    };
    if let Some(clash) = inner.iter().find(|n| graph.nodes.contains_key(&n.id)) {
        return Err(format!("Node `{}` of the section is already in this graph", clash.id));
    }

    let incoming: Vec<&Connection> = graph
        .connections
        .iter()
        .filter(|c| c.target_node == node_id)
        .collect();
    let outgoing: Vec<&Connection> = graph
        .connections
        .iter()
        .filter(|c| c.source_node == node_id)
        .collect();

    let mut commands = vec![GraphCommand::RemoveNode {
        id: node_id.to_string(),
    }];
    commands.extend(inner.iter().map(|n| GraphCommand::AddNode((*n).clone())));
    for wire in &section.connections {
        let from_entry = is_type(&wire.source_node, ENTRY_TYPE);
        let to_exit = is_type(&wire.target_node, EXIT_TYPE);
        match (from_entry, to_exit) {
            (false, false) => commands.push(GraphCommand::Connect(wire.clone())),
            (true, false) => {
                for outer in incoming.iter().filter(|c| c.target_pin == wire.source_pin) {
                    commands.push(GraphCommand::Connect(Connection::new(
                        &outer.source_node,
                        &outer.source_pin,
                        &wire.target_node,
                        &wire.target_pin,
                        wire.connection_type,
                    )));
                }
            }
            (false, true) => {
                for outer in outgoing.iter().filter(|c| c.source_pin == wire.target_pin) {
                    commands.push(GraphCommand::Connect(Connection::new(
                        &wire.source_node,
                        &wire.source_pin,
                        &outer.target_node,
                        &outer.target_pin,
                        wire.connection_type,
                    )));
                }
            }
            (true, true) => {
                for inner_in in incoming.iter().filter(|c| c.target_pin == wire.source_pin) {
                    for outer in outgoing.iter().filter(|c| c.source_pin == wire.target_pin) {
                        commands.push(GraphCommand::Connect(Connection::new(
                            &inner_in.source_node,
                            &inner_in.source_pin,
                            &outer.target_node,
                            &outer.target_pin,
                            wire.connection_type,
                        )));
                    }
                }
            }
        }
    }
    Ok(commands)
}
