//! Render graph domain rules, in the graph editor's terms.
//!
//! Pure (no UI), so it is unit tested: the node registry, the pin type strings
//! the editor compares, pin styles, the node catalogue and factory, parameters as
//! the editor stores them (text), dynamic pins, auto layout and the checks that
//! stop a graph from building. The document itself, and how it becomes a
//! `.rendergraph` asset, is [`crate::document`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use graphy::{DataType, GraphCommand, GraphDescription, NodeInstance, Pin, PinInstance, PinType, Position};
use graphy_gpui::CatalogEntry;
use graphy_render::geom::{rgba, Color};
use graphy_render::presenter::{PinShape, PinStyle};
use helio_graph_asset::{
    GraphAsset, NodeDescriptor, NodeRegistry, ParamType, Params, PinDesc, PinKind,
    PinType as RenderPinType, Severity,
};
use serde_json::Value;

/// The node types render graphs can use.
pub fn registry() -> &'static Arc<NodeRegistry> {
    static REGISTRY: OnceLock<Arc<NodeRegistry>> = OnceLock::new();
    // The engine's registry: Helio's stages with the pins their passes declare
    // (the first call waits for the probe that reads them), and every node any
    // crate in this binary registered.
    REGISTRY.get_or_init(|| {
        Arc::new(
            engine_backend::subsystems::render::helio_renderer::pipelines::graph_node_registry(),
        )
    })
}

// ── Pin types ────────────────────────────────────────────────────────────────

/// The type string of an execution pin.
pub const EXECUTION: &str = "execution";

/// The editor connects two pins when their type strings are equal, so the
/// string carries everything a render pin's compatibility depends on: kind,
/// persistence and the Helio resource name. Flow pins are the editor's own
/// execution pins.
pub fn pin_type_string(pin: &PinDesc) -> String {
    let prefix = match (pin.ty.kind, pin.ty.persistent) {
        (PinKind::Flow, _) => return EXECUTION.to_string(),
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
pub fn pin_type_of(type_name: &str) -> RenderPinType {
    let prefix = type_name
        .split_once(':')
        .map_or(type_name, |(prefix, _)| prefix);
    match prefix {
        "tex" => RenderPinType::TEXTURE,
        "ptex" => RenderPinType::PERSISTENT_TEXTURE,
        "depth" => RenderPinType::DEPTH,
        "buf" => RenderPinType::BUFFER,
        "pbuf" => RenderPinType::PERSISTENT_BUFFER,
        "scene" => RenderPinType::SCENE,
        _ => RenderPinType::FLOW,
    }
}

/// The type string of an editor pin type.
pub fn type_name(data_type: &DataType) -> String {
    match data_type {
        DataType::Exec => EXECUTION.to_string(),
        DataType::Data(info) => info.type_string.clone(),
    }
}

/// The editor pin type for a type string.
pub fn data_type(type_name: &str) -> DataType {
    if type_name == EXECUTION {
        DataType::Exec
    } else {
        DataType::typed(type_name)
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
        color: rgba(r, g, b, 1.0),
    }
}

/// What the editor prints next to a pin: its resource, marked when it reads the
/// previous frame; nothing for a flow pin.
pub fn pin_label(pin: &PinDesc) -> String {
    if pin.ty.kind == PinKind::Flow {
        String::new()
    } else if pin.history {
        format!("{} (prev frame)", pin.resource)
    } else {
        pin.resource.clone()
    }
}

/// An editor pin for a node pin description.
pub fn pin_instance(pin: &PinDesc, direction: PinType) -> PinInstance {
    PinInstance {
        id: pin.name.clone(),
        pin: Pin {
            id: pin.name.clone(),
            name: pin_label(pin),
            data_type: data_type(&pin_type_string(pin)),
            pin_type: direction,
        },
    }
}

pub fn pin_instances(pins: &[PinDesc], direction: PinType) -> Vec<PinInstance> {
    pins.iter().map(|pin| pin_instance(pin, direction)).collect()
}

// ── Catalogue ────────────────────────────────────────────────────────────────

/// Accent colour and glyph of a palette category.
pub fn category_look(category: &str) -> (Color, char) {
    match category {
        "Graph" => (rgba(0x6B, 0x72, 0x80, 1.0), '⏹'),
        "Resources" => (rgba(0xB7, 0x79, 0x1F, 1.0), '▦'),
        "Helio Stages" => (rgba(0x2B, 0x6C, 0xB0, 1.0), '◈'),
        "SceneDB" => (rgba(0x2F, 0x85, 0x5A, 1.0), '▤'),
        _ => (rgba(0x4A, 0x55, 0x68, 1.0), '●'),
    }
}

/// A parameter value as the text the editor stores and shows.
pub fn param_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The palette for `registry`: one entry per node type.
pub fn catalog(registry: &NodeRegistry) -> Vec<CatalogEntry> {
    registry
        .list()
        .iter()
        .map(|descriptor| {
            let (color, glyph) = category_look(&descriptor.category);
            CatalogEntry {
                definition: descriptor.kind.clone(),
                title: descriptor.title.clone(),
                category: descriptor.category.clone(),
                keywords: vec![descriptor.kind.clone()],
                description: descriptor.description.clone(),
                icon: glyph.to_string(),
                color: Some(color),
                inputs: None,
            }
        })
        .collect()
}

/// A fresh node of a registered type, with its default parameters as properties
/// and the pins they give it. `None` for a type the registry does not know.
pub fn create_node(
    registry: &NodeRegistry,
    definition: &str,
    id: &str,
    position: Position,
) -> Option<NodeInstance> {
    let descriptor = registry.get(definition)?;
    let params = descriptor.effective_params(&Params::new());
    let pins = descriptor.pins(&params);
    let mut node = NodeInstance::new(id, definition, position);
    for (key, value) in &params {
        node.properties
            .insert(key.clone(), Value::from(param_string(value)));
    }
    node.inputs = pin_instances(&pins.inputs, PinType::Input);
    node.outputs = pin_instances(&pins.outputs, PinType::Output);
    Some(node)
}

// ── Params ───────────────────────────────────────────────────────────────────

/// A parameter as the editor stores it (text) read back as its declared type.
pub fn param_from_text(ty: &ParamType, text: &str) -> Value {
    match ty {
        ParamType::Bool => Value::from(text.trim().eq_ignore_ascii_case("true")),
        ParamType::Int { .. } => text
            .trim()
            .parse::<i64>()
            .map_or(Value::from(text), Value::from),
        ParamType::Float { .. } => text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::from(text), Value::Number),
        ParamType::Text | ParamType::Choice(_) => Value::from(text),
    }
}

/// A stored node property as the text the node descriptors read.
pub fn property_text(value: &Value) -> String {
    param_string(value)
}

/// A node's properties as text.
pub fn properties_as_text(node: &NodeInstance) -> HashMap<String, String> {
    node.properties
        .iter()
        .map(|(key, value)| (key.clone(), property_text(value)))
        .collect()
}

/// The asset params of a node's properties, exactly as the node holds them: a
/// value the author edited (text) is read back as its declared type; a value
/// loaded from the asset, or a property the node does not declare, is kept as it
/// was. Nothing is dropped for equalling a default, because Helio reads some
/// params raw (a Get or Set node's `buffer`), so an explicit value must stay.
pub fn params_of(descriptor: Option<&NodeDescriptor>, node: &NodeInstance) -> Params {
    let mut params = Params::new();
    for (key, value) in &node.properties {
        let declared = descriptor.and_then(|d| d.params.iter().find(|p| &p.key == key));
        let value = match (declared, value) {
            (Some(declared), Value::String(text)) => param_from_text(&declared.ty, text),
            _ => value.clone(),
        };
        params.insert(key.clone(), value);
    }
    params
}

/// The properties a node gets from asset params: the params, as they are. A
/// parameter the asset leaves out stays out (it reads as its default), so saving
/// writes back what was loaded.
pub fn properties_of(params: &Params) -> HashMap<String, Value> {
    params.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// A node's properties as text with the defaults of its declared parameters
/// filled in: what the node effectively has.
pub fn effective_text(
    descriptor: Option<&NodeDescriptor>,
    node: &NodeInstance,
) -> HashMap<String, String> {
    let mut text = HashMap::new();
    if let Some(descriptor) = descriptor {
        for param in &descriptor.params {
            text.insert(param.key.clone(), param_string(&param.default));
        }
    }
    text.extend(properties_as_text(node));
    text
}

// ── Editing ───────────────────────────────────────────────────────────────────

/// How a parameter is edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// Free text; numbers are edited as text too.
    Text,
    Bool,
    /// One of a fixed set.
    Choice(Vec<String>),
}

/// An editable node parameter, stored as text in the node's properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamField {
    pub key: String,
    pub label: String,
    pub kind: ParamKind,
}

/// The parameters the editor shows for a node type.
pub fn parameter_fields(definition_id: &str, registry: &NodeRegistry) -> Vec<ParamField> {
    registry
        .get(definition_id)
        .map_or_else(Vec::new, |descriptor| {
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

/// The title of a Get or Set Buffer node: what it does and to which buffer
/// (`Get pre_aa`). `None` for any other node or a node without a buffer yet.
pub fn buffer_title(definition_id: &str, properties: &HashMap<String, String>) -> Option<String> {
    let verb = match definition_id {
        helio_graph_asset::core_nodes::BUFFER_GET => "Get",
        helio_graph_asset::core_nodes::BUFFER_SET => "Set",
        _ => return None,
    };
    let (_, name) = helio_graph_asset::buffers::parse_entry(properties.get("buffer")?)?;
    Some(format!("{verb} {name}"))
}

/// Edit commands re-deriving a node's pins after one of its parameters
/// changed. A resource node's pin is named after the resource, so its wires
/// are moved to the pin's new id rather than left pointing at one that no
/// longer exists.
pub fn refresh_pins(
    graph: &GraphDescription,
    node_id: &str,
    registry: &NodeRegistry,
) -> Vec<GraphCommand> {
    let Some(node) = graph.nodes.get(node_id) else {
        return Vec::new();
    };
    let Some(descriptor) = registry.get(&node.node_type) else {
        return Vec::new();
    };
    let params = params_of(Some(descriptor), node);
    let pins = descriptor.pins(&params);
    let inputs = pin_instances(&pins.inputs, PinType::Input);
    let outputs = pin_instances(&pins.outputs, PinType::Output);

    // Pins keep their position, so the i-th old pin becomes the i-th new one.
    let renamed = |old: &[PinInstance], new: &[PinInstance]| {
        old.iter()
            .zip(new)
            .filter(|(old, new)| old.id != new.id)
            .map(|(old, new)| (old.id.clone(), new.id.clone()))
            .collect::<Vec<_>>()
    };
    let renamed_inputs = renamed(&node.inputs, &inputs);
    let renamed_outputs = renamed(&node.outputs, &outputs);

    // Wires on renamed pins are lifted first and put back on the new ids.
    let mut lifted = Vec::new();
    let mut rewired = Vec::new();
    for connection in &graph.connections {
        let mut moved = connection.clone();
        let mut changed = false;
        if connection.source_node == node_id {
            if let Some((_, new)) = renamed_outputs
                .iter()
                .find(|(old, _)| *old == connection.source_pin)
            {
                moved.source_pin = new.clone();
                changed = true;
            }
        }
        if connection.target_node == node_id {
            if let Some((_, new)) = renamed_inputs
                .iter()
                .find(|(old, _)| *old == connection.target_pin)
            {
                moved.target_pin = new.clone();
                changed = true;
            }
        }
        if changed {
            lifted.push(GraphCommand::Disconnect(connection.clone()));
            rewired.push(GraphCommand::Connect(moved));
        }
    }

    let unchanged = node.inputs.len() == inputs.len()
        && node.outputs.len() == outputs.len()
        && node
            .inputs
            .iter()
            .zip(&inputs)
            .chain(node.outputs.iter().zip(&outputs))
            .all(|(a, b)| {
                a.id == b.id && a.pin.name == b.pin.name && a.pin.data_type == b.pin.data_type
            });
    if unchanged {
        return Vec::new();
    }

    let mut commands = lifted;
    commands.push(GraphCommand::SetPins {
        node: node_id.to_string(),
        inputs,
        outputs,
    });
    commands.extend(rewired);
    commands
}

// ── Layout ────────────────────────────────────────────────────────────────────

/// Arranges the graph as edit commands: positions every node and replaces the
/// lane comments, keeping comments the author made. Reroute nodes are the
/// editor's own and keep where they are.
pub fn auto_layout_graph(graph: &GraphDescription) -> Vec<GraphCommand> {
    let mut asset = crate::document::layout_asset(graph, registry());
    helio_graph_asset::auto_layout(&mut asset, registry());

    let positions: HashMap<String, [f32; 2]> = asset
        .nodes
        .iter()
        .map(|n| (n.id.clone(), n.position))
        .collect();
    let mut commands = Vec::new();
    let mut ids: Vec<&String> = graph.nodes.keys().collect();
    ids.sort();
    for id in ids {
        if let Some(position) = positions.get(id) {
            let node = &graph.nodes[id];
            let target = (f64::from(position[0]), f64::from(position[1]));
            if (node.position.x, node.position.y) != target {
                commands.push(GraphCommand::MoveNode {
                    id: id.clone(),
                    x: target.0,
                    y: target.1,
                });
            }
        }
    }

    // Drop the old lane comments (highest index first) and add the new ones.
    for (index, comment) in graph.comments.iter().enumerate().rev() {
        if comment.id.starts_with(helio_graph_asset::LANE_PREFIX) {
            commands.push(GraphCommand::RemoveComment { index });
        }
    }
    let kept = graph
        .comments
        .iter()
        .filter(|c| !c.id.starts_with(helio_graph_asset::LANE_PREFIX))
        .count();
    for (offset, c) in asset.comments.iter().enumerate() {
        let mut comment = graphy::core::GraphComment::new(
            c.text.clone(),
            Position::new(f64::from(c.position[0]), f64::from(c.position[1])),
            (f64::from(c.size[0]), f64::from(c.size[1])),
        );
        comment.id = c.id.clone();
        comment.color = c.color;
        comment.contained_node_ids = c.nodes.clone();
        commands.push(GraphCommand::AddComment {
            index: kept + offset,
            comment,
        });
    }
    commands
}

// ── Checks ────────────────────────────────────────────────────────────────────

/// Problems that stop a graph from building, as one line each.
pub fn problems(asset: &GraphAsset, registry: &NodeRegistry) -> Vec<String> {
    helio_graph_asset::validate(asset, registry)
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(ToString::to_string)
        .collect()
}

/// The palette categories of `registry`, in order, each with its entries.
pub fn catalog_by_category(registry: &NodeRegistry) -> BTreeMap<String, Vec<CatalogEntry>> {
    let mut by_category: BTreeMap<String, Vec<CatalogEntry>> = BTreeMap::new();
    for entry in catalog(registry) {
        by_category.entry(entry.category.clone()).or_default().push(entry);
    }
    by_category
}
