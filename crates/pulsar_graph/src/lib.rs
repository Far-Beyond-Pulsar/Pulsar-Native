use std::collections::HashMap;
use serde::{Deserialize, Serialize};

pub mod type_system;
pub use type_system::*;

/// Unified blueprint file format containing everything (like Unreal Engine)
/// This is the top-level structure that gets saved to .bp_graph files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueprintAsset {
    /// File format version for backward compatibility
    pub format_version: u32,
    
    /// The main event graph
    pub main_graph: GraphDescription,
    
    /// All local macro graphs defined in this blueprint
    #[serde(default)]
    pub local_macros: Vec<SubGraphDefinition>,
    
    /// Class variables
    #[serde(default)]
    pub variables: Vec<ClassVariable>,
    
    /// Editor-only data (open tabs, UI state, etc.)
    #[serde(default)]
    pub editor_state: Option<BlueprintEditorState>,

    /// Blueprint metadata (type, parent class, etc.)
    #[serde(default)]
    pub blueprint_metadata: BlueprintMetadata,
}

/// Blueprint metadata for context sensitivity and organization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueprintMetadata {
    /// Blueprint type (Actor, Widget, Component, etc.)
    #[serde(default)]
    pub blueprint_type: String,

    /// Parent class (if applicable)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_class: Option<String>,

    /// Blueprint description
    #[serde(default)]
    pub description: String,

    /// Blueprint category for organization
    #[serde(default)]
    pub category: String,

    /// Tags for search/filtering
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Default for BlueprintMetadata {
    fn default() -> Self {
        Self {
            blueprint_type: "Generic".to_string(),
            parent_class: None,
            description: String::new(),
            category: "Uncategorized".to_string(),
            tags: Vec::new(),
        }
    }
}

/// Editor state for restoring the blueprint editor UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueprintEditorState {
    /// Which tabs were open (by ID)
    pub open_tab_ids: Vec<String>,
    
    /// Which tab was active (index into open_tab_ids)
    #[serde(default)]
    pub active_tab_index: usize,
    
    /// Camera position and zoom for each graph
    #[serde(default)]
    pub graph_view_states: HashMap<String, GraphViewState>,
}

/// View state for a single graph (camera position, zoom, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphViewState {
    pub pan_offset_x: f32,
    pub pan_offset_y: f32,
    pub zoom: f32,
}

/// Class variable definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassVariable {
    pub id: String,
    pub name: String,
    pub data_type: DataType,
    pub default_value: Option<String>,
    #[serde(default)]
    pub description: String,
}

impl BlueprintAsset {
    /// Create a new empty blueprint asset
    pub fn new(name: &str) -> Self {
        Self {
            format_version: 1,
            main_graph: GraphDescription::new(name),
            local_macros: Vec::new(),
            variables: Vec::new(),
            editor_state: Some(BlueprintEditorState {
                open_tab_ids: vec!["main".to_string()],
                active_tab_index: 0,
                graph_view_states: HashMap::new(),
            }),
            blueprint_metadata: BlueprintMetadata::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphDescription {
    pub nodes: HashMap<String, NodeInstance>,
    pub connections: Vec<Connection>,
    pub metadata: GraphMetadata,
    #[serde(default)]
    pub comments: Vec<BlueprintComment>,
}

/// Comment box in a blueprint graph
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlueprintComment {
    pub id: String,
    pub text: String,
    #[serde(with = "tuple_or_object_f32_pair")]
    pub position: (f32, f32), // Simple tuple for position
    #[serde(with = "tuple_or_object_f32_pair")]
    pub size: (f32, f32),     // Simple tuple for size  
    #[serde(with = "array_or_hsla_color")]
    pub color: [f32; 4],      // RGBA color as array
    pub contained_node_ids: Vec<String>,
}

// Helper module to deserialize both tuple and object formats for (f32, f32)
mod tuple_or_object_f32_pair {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum F32Pair {
        Tuple(f32, f32),
        Object { x: f32, y: f32 },
        ObjectAlt { width: f32, height: f32 },
    }

    pub fn serialize<S>(value: &(f32, f32), serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<(f32, f32), D::Error>
    where
        D: Deserializer<'de>,
    {
        match F32Pair::deserialize(deserializer)? {
            F32Pair::Tuple(x, y) => Ok((x, y)),
            F32Pair::Object { x, y } => Ok((x, y)),
            F32Pair::ObjectAlt { width, height } => Ok((width, height)),
        }
    }
}

// Helper module to deserialize both array and HSLA object formats for colors
mod array_or_hsla_color {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ColorFormat {
        Array([f32; 4]),
        HSLA { h: f32, s: f32, l: f32, a: f32 },
    }

    pub fn serialize<S>(value: &[f32; 4], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[f32; 4], D::Error>
    where
        D: Deserializer<'de>,
    {
        match ColorFormat::deserialize(deserializer)? {
            ColorFormat::Array(arr) => Ok(arr),
            ColorFormat::HSLA { h, s, l, a } => {
                // Convert HSLA to RGBA
                let (r, g, b) = hsl_to_rgb(h, s, l);
                Ok([r, g, b, a])
            }
        }
    }

    fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
        if s == 0.0 {
            return (l, l, l);
        }
        
        let q = if l < 0.5 {
            l * (1.0 + s)
        } else {
            l + s - l * s
        };
        let p = 2.0 * l - q;
        
        let hue_to_rgb = |p: f32, q: f32, mut t: f32| -> f32 {
            if t < 0.0 { t += 1.0; }
            if t > 1.0 { t -= 1.0; }
            if t < 1.0 / 6.0 { return p + (q - p) * 6.0 * t; }
            if t < 1.0 / 2.0 { return q; }
            if t < 2.0 / 3.0 { return p + (q - p) * (2.0 / 3.0 - t) * 6.0; }
            p
        };
        
        (
            hue_to_rgb(p, q, h + 1.0 / 3.0),
            hue_to_rgb(p, q, h),
            hue_to_rgb(p, q, h - 1.0 / 3.0),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinInstance {
    pub id: String,
    #[serde(flatten)]
    pub pin: Pin,
}

#[derive(Debug, Clone)]
pub struct NodeInstance {
    pub id: String,
    pub node_type: String,
    pub position: Position,
    pub properties: HashMap<String, PropertyValue>,
    pub inputs: Vec<PinInstance>,
    pub outputs: Vec<PinInstance>,
}

// Custom (de)serialization for NodeInstance to support both array and map for inputs/outputs
impl<'de> Deserialize<'de> for NodeInstance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct NodeInstanceHelper {
            id: String,
            node_type: String,
            position: Position,
            properties: HashMap<String, PropertyValue>,
            #[serde(default)]
            inputs: serde_json::Value,
            #[serde(default)]
            outputs: serde_json::Value,
        }

        let helper = NodeInstanceHelper::deserialize(deserializer)?;

        fn parse_pins(val: &serde_json::Value) -> Vec<PinInstance> {
            if let Some(arr) = val.as_array() {
                arr.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()
            } else if let Some(obj) = val.as_object() {
                obj.iter()
                    .filter_map(|(id, v)| {
                        let pin: Pin = serde_json::from_value(v.clone()).ok()?;
                        Some(PinInstance { id: id.clone(), pin })
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }

        Ok(NodeInstance {
            id: helper.id,
            node_type: helper.node_type,
            position: helper.position,
            properties: helper.properties,
            inputs: parse_pins(&helper.inputs),
            outputs: parse_pins(&helper.outputs),
        })
    }
}

impl Serialize for NodeInstance {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("NodeInstance", 6)?;
        s.serialize_field("id", &self.id)?;
        s.serialize_field("node_type", &self.node_type)?;
        s.serialize_field("position", &self.position)?;
        s.serialize_field("properties", &self.properties)?;
        s.serialize_field("inputs", &self.inputs)?;
        s.serialize_field("outputs", &self.outputs)?;
        s.end()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub source_node: String,
    pub source_pin: String,
    pub target_node: String,
    pub target_pin: String,
    pub connection_type: ConnectionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pin {
    pub name: String,
    pub pin_type: PinType,
    pub data_type: DataType,
    pub connected_to: Vec<String>, // Connection IDs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphMetadata {
    pub name: String,
    pub description: String,
    pub version: String,
    pub created_at: String,
    pub modified_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PropertyValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Vector2(f32, f32),
    Vector3(f32, f32, f32),
    Color(f32, f32, f32, f32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PinType {
    Input,
    Output,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DataType {
    Execution,
    Typed(TypeInfo),
    Any, // Typeless - matches with any type (used for reroute nodes)
    // Legacy types for backward compatibility
    String,
    Number,
    Boolean,
    Vector2,
    Vector3,
    Color,
    Object,
    Array(Box<DataType>),
}

impl DataType {
    /// Create a DataType from a type string
    pub fn from_type_str(type_str: &str) -> Self {
        match type_str {
            "execution" => DataType::Execution,
            // Parse as TypeInfo for proper type system support
            _ => DataType::Typed(TypeInfo::parse(type_str)),
        }
    }

    /// Get the TypeInfo for this DataType (if applicable)
    pub fn type_info(&self) -> Option<&TypeInfo> {
        match self {
            DataType::Typed(type_info) => Some(type_info),
            _ => None,
        }
    }

    /// Get the Rust type string representation for tooltip display
    pub fn rust_type_string(&self) -> String {
        match self {
            DataType::Execution => "()".to_string(), // Execution flow has no data type
            DataType::Typed(type_info) => type_info.to_string(),
            DataType::Any => "Any".to_string(), // Typeless reroute node
            DataType::String => "String".to_string(),
            DataType::Number => "f64".to_string(),
            DataType::Boolean => "bool".to_string(),
            DataType::Vector2 => "(f32, f32)".to_string(),
            DataType::Vector3 => "(f32, f32, f32)".to_string(),
            DataType::Color => "(f32, f32, f32, f32)".to_string(),
            DataType::Object => "dyn Any".to_string(),
            DataType::Array(inner) => format!("Vec<{}>", inner.rust_type_string()),
        }
    }

    /// Check if this DataType is compatible with another
    pub fn is_compatible_with(&self, other: &DataType) -> bool {
        match (self, other) {
            // Any type is compatible with everything (for typeless reroute nodes)
            (DataType::Any, _) | (_, DataType::Any) => true,
            (DataType::Execution, DataType::Execution) => true,
            (DataType::Typed(a), DataType::Typed(b)) => a.is_compatible_with(b),
            // Legacy compatibility
            (DataType::String, DataType::String) => true,
            (DataType::Number, DataType::Number) => true,
            (DataType::Boolean, DataType::Boolean) => true,
            (DataType::Vector2, DataType::Vector2) => true,
            (DataType::Vector3, DataType::Vector3) => true,
            (DataType::Color, DataType::Color) => true,
            (DataType::Object, DataType::Object) => true,
            // Array compatibility
            (DataType::Array(a), DataType::Array(b)) => a.is_compatible_with(b),
            _ => false,
        }
    }

    /// Generate a pin style for this data type
    pub fn generate_pin_style(&self) -> PinStyle {
        match self {
            DataType::Execution => PinStyle {
                color: PinColor { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }, // Red for execution
                icon: PinIcon::Triangle,
                is_rainbow: false,
            },
            DataType::Typed(type_info) => type_info.generate_pin_style(),
            DataType::Any => PinStyle {
                color: PinColor { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }, // Gray for typeless
                icon: PinIcon::Circle,
                is_rainbow: false,
            },
            // Legacy types
            DataType::String => PinStyle {
                color: PinColor { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }, // Green
                icon: PinIcon::Circle,
                is_rainbow: false,
            },
            DataType::Number => PinStyle {
                color: PinColor { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }, // Blue
                icon: PinIcon::Circle,
                is_rainbow: false,
            },
            DataType::Boolean => PinStyle {
                color: PinColor { r: 1.0, g: 1.0, b: 0.0, a: 1.0 }, // Yellow
                icon: PinIcon::Circle,
                is_rainbow: false,
            },
            DataType::Vector2 | DataType::Vector3 => PinStyle {
                color: PinColor { r: 1.0, g: 0.0, b: 1.0, a: 1.0 }, // Magenta
                icon: PinIcon::Circle,
                is_rainbow: false,
            },
            DataType::Color => PinStyle {
                color: PinColor { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }, // Gray
                icon: PinIcon::Circle,
                is_rainbow: false,
            },
            DataType::Object => PinStyle {
                color: PinColor { r: 0.8, g: 0.4, b: 0.2, a: 1.0 }, // Brown
                icon: PinIcon::Hexagon,
                is_rainbow: false,
            },
            DataType::Array(_) => PinStyle {
                color: PinColor { r: 0.0, g: 0.8, b: 0.8, a: 1.0 }, // Cyan
                icon: PinIcon::Square,
                is_rainbow: false,
            },
        }
    }
}

impl PartialEq<&str> for DataType {
    fn eq(&self, other: &&str) -> bool {
        match (self, *other) {
            (DataType::Execution, "execution") => true,
            (DataType::String, "string") => true,
            (DataType::Number, "number") => true,
            (DataType::Boolean, "boolean") => true,
            (DataType::Vector2, "vector2") => true,
            (DataType::Vector3, "vector3") => true,
            (DataType::Color, "color") => true,
            (DataType::Object, "object") => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataType::Execution => write!(f, "execution"),
            DataType::Typed(type_info) => write!(f, "{}", type_info),
            DataType::Any => write!(f, "any"),
            // Legacy types
            DataType::String => write!(f, "string"),
            DataType::Number => write!(f, "number"),
            DataType::Boolean => write!(f, "boolean"),
            DataType::Vector2 => write!(f, "vector2"),
            DataType::Vector3 => write!(f, "vector3"),
            DataType::Color => write!(f, "color"),
            DataType::Object => write!(f, "object"),
            DataType::Array(inner) => write!(f, "array<{}>", inner),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectionType {
    Execution,
    Data,
}

impl GraphDescription {
    pub fn new(name: &str) -> Self {
        Self {
            nodes: HashMap::new(),
            connections: Vec::new(),
            metadata: GraphMetadata {
                name: name.to_string(),
                description: String::new(),
                version: "1.0.0".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                modified_at: chrono::Utc::now().to_rfc3339(),
            },
            comments: Vec::new(),
        }
    }

    pub fn add_node(&mut self, node: NodeInstance) {
        self.nodes.insert(node.id.clone(), node);
        self.metadata.modified_at = chrono::Utc::now().to_rfc3339();
    }

    pub fn add_connection(&mut self, connection: Connection) {
        // Update pin connections
        if let Some(source_node) = self.nodes.get_mut(&connection.source_node) {
            if let Some(output_pin) = source_node.outputs.iter_mut().find(|p| p.id == connection.source_pin) {
                output_pin.pin.connected_to.push(connection.id.clone());
            }
        }

        if let Some(target_node) = self.nodes.get_mut(&connection.target_node) {
            if let Some(input_pin) = target_node.inputs.iter_mut().find(|p| p.id == connection.target_pin) {
                input_pin.pin.connected_to.push(connection.id.clone());
            }
        }

        self.connections.push(connection);
        self.metadata.modified_at = chrono::Utc::now().to_rfc3339();
    }

    pub fn remove_node(&mut self, node_id: &str) {
        // Remove all connections involving this node
        self.connections.retain(|conn| {
            conn.source_node != node_id && conn.target_node != node_id
        });

        // Remove the node
        self.nodes.remove(node_id);
        self.metadata.modified_at = chrono::Utc::now().to_rfc3339();
    }

    pub fn remove_connection(&mut self, connection_id: &str) {
        if let Some(index) = self.connections.iter().position(|conn| conn.id == connection_id) {
            let connection = &self.connections[index];

            // Update pin connections
            if let Some(source_node) = self.nodes.get_mut(&connection.source_node) {
                if let Some(output_pin) = source_node.outputs.iter_mut().find(|p| p.id == connection.source_pin) {
                    output_pin.pin.connected_to.retain(|id| id != connection_id);
                }
            }

            if let Some(target_node) = self.nodes.get_mut(&connection.target_node) {
                if let Some(input_pin) = target_node.inputs.iter_mut().find(|p| p.id == connection.target_pin) {
                    input_pin.pin.connected_to.retain(|id| id != connection_id);
                }
            }

            self.connections.remove(index);
            self.metadata.modified_at = chrono::Utc::now().to_rfc3339();
        }
    }

    pub fn get_execution_order(&self) -> Result<Vec<String>, String> {
        // Topological sort for execution order
        let mut visited = HashMap::new();
        let mut temp_visited = HashMap::new();
        let mut result = Vec::new();

        // Find entry points (nodes with no incoming execution connections)
        for node_id in self.nodes.keys() {
            if !visited.contains_key(node_id) {
                self.visit_node(node_id, &mut visited, &mut temp_visited, &mut result)?;
            }
        }

        Ok(result)
    }

    fn visit_node(
        &self,
        node_id: &str,
        visited: &mut HashMap<String, bool>,
        temp_visited: &mut HashMap<String, bool>,
        result: &mut Vec<String>,
    ) -> Result<(), String> {
        if temp_visited.contains_key(node_id) {
            return Err(format!("Circular dependency detected involving node {}", node_id));
        }

        if visited.contains_key(node_id) {
            return Ok(());
        }

        temp_visited.insert(node_id.to_string(), true);

        // Visit all dependent nodes (those connected via execution pins)
        for connection in &self.connections {
            if connection.source_node == node_id && matches!(connection.connection_type, ConnectionType::Execution) {
                self.visit_node(&connection.target_node, visited, temp_visited, result)?;
            }
        }

        temp_visited.remove(node_id);
        visited.insert(node_id.to_string(), true);
        result.push(node_id.to_string());

        Ok(())
    }
}

impl NodeInstance {
    pub fn new(id: &str, node_type: &str, position: Position) -> Self {
        Self {
            id: id.to_string(),
            node_type: node_type.to_string(),
            position,
            properties: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub fn add_input_pin(&mut self, name: &str, data_type: DataType) {
        let pin = Pin {
            name: name.to_string(),
            pin_type: PinType::Input,
            data_type,
            connected_to: Vec::new(),
        };
        self.inputs.push(PinInstance { id: name.to_string(), pin });
    }

    pub fn add_output_pin(&mut self, name: &str, data_type: DataType) {
        let pin = Pin {
            name: name.to_string(),
            pin_type: PinType::Output,
            data_type,
            connected_to: Vec::new(),
        };
        self.outputs.push(PinInstance { id: name.to_string(), pin });
    }

    pub fn set_property(&mut self, name: &str, value: PropertyValue) {
        self.properties.insert(name.to_string(), value);
    }
}

impl Connection {
    pub fn new(
        id: &str,
        source_node: &str,
        source_pin: &str,
        target_node: &str,
        target_pin: &str,
        connection_type: ConnectionType,
    ) -> Self {
        Self {
            id: id.to_string(),
            source_node: source_node.to_string(),
            source_pin: source_pin.to_string(),
            target_node: target_node.to_string(),
            target_pin: target_pin.to_string(),
            connection_type,
        }
    }
}

// ===== Sub-Graph System =====

/// Definition of a sub-graph (collapsed graph/macro)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubGraphDefinition {
    /// Unique identifier for this sub-graph type
    pub id: String,

    /// Display name
    pub name: String,

    /// Description for documentation
    pub description: String,

    /// The internal graph structure
    pub graph: GraphDescription,

    /// Custom interface pins defined by the user
    pub interface: SubGraphInterface,

    /// Metadata
    pub metadata: SubGraphMetadata,

    /// Macro-specific configuration
    #[serde(default)]
    pub macro_config: MacroConfiguration,
}

/// Interface definition for a sub-graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubGraphInterface {
    /// Custom input pins (can include both exec and data pins)
    pub inputs: Vec<SubGraphPin>,

    /// Custom output pins (can include both exec and data pins)
    pub outputs: Vec<SubGraphPin>,
}

/// A pin definition in a sub-graph interface
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubGraphPin {
    /// Pin identifier (used for connections)
    pub id: String,

    /// Display name
    pub name: String,

    /// Pin data type (Execution or Typed)
    pub data_type: DataType,

    /// Optional description (shown in tooltip)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Default value (serialized as string)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,

    /// If true, this pin can be edited in details panel for macro instances
    #[serde(default)]
    pub is_instance_editable: bool,

    /// Category for organizing in details panel
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// Metadata for a sub-graph definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubGraphMetadata {
    pub created_at: String,
    pub modified_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Macro-specific configuration (Unreal Engine style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroConfiguration {
    /// If true, macro has no exec pins (pure data transform)
    #[serde(default)]
    pub is_pure: bool,

    /// Optional single-line compact title for node display
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_node_title: Option<String>,

    /// Category for organizing in palette (e.g., "Math", "Utilities")
    #[serde(default)]
    pub category: String,

    /// Custom tooltip text (overrides auto-generated)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,

    /// Keywords for search/discovery
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Which pins are editable in details panel when instance is selected
    #[serde(default)]
    pub instance_editable_pins: Vec<String>,

    /// Optional custom node color (overrides default)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<(f32, f32, f32)>,

    /// Optional custom icon emoji or code
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Context sensitivity - which blueprint types can use this macro
    /// Empty = available in all blueprints
    /// Examples: ["Actor", "Widget", "Component"]
    #[serde(default)]
    pub parent_class_filter: Vec<String>,

    /// If true, hide from palette (internal macro)
    #[serde(default)]
    pub hide_in_palette: bool,
}

impl Default for MacroConfiguration {
    fn default() -> Self {
        Self {
            is_pure: false,
            compact_node_title: None,
            category: "Macros".to_string(),
            tooltip: None,
            keywords: Vec::new(),
            instance_editable_pins: Vec::new(),
            color: None,
            icon: None,
            parent_class_filter: Vec::new(),
            hide_in_palette: false,
        }
    }
}

impl SubGraphDefinition {
    /// Create a new sub-graph definition
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            graph: GraphDescription::new(name),
            interface: SubGraphInterface {
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
            metadata: SubGraphMetadata {
                created_at: chrono::Utc::now().to_rfc3339(),
                modified_at: chrono::Utc::now().to_rfc3339(),
                author: None,
                tags: Vec::new(),
            },
            macro_config: MacroConfiguration::default(),
        }
    }

    /// Add an input pin to the interface
    pub fn add_input_pin(&mut self, id: &str, name: &str, data_type: DataType) {
        self.interface.inputs.push(SubGraphPin {
            id: id.to_string(),
            name: name.to_string(),
            data_type,
            description: None,
            default_value: None,
            is_instance_editable: false,
            category: None,
        });
        self.metadata.modified_at = chrono::Utc::now().to_rfc3339();
    }

    /// Add an output pin to the interface
    pub fn add_output_pin(&mut self, id: &str, name: &str, data_type: DataType) {
        self.interface.outputs.push(SubGraphPin {
            id: id.to_string(),
            name: name.to_string(),
            data_type,
            description: None,
            default_value: None,
            is_instance_editable: false,
            category: None,
        });
        self.metadata.modified_at = chrono::Utc::now().to_rfc3339();
    }

    /// Update the internal Input/Output nodes to match the interface
    /// Now uses macro_entry and macro_exit node types for better clarity
    pub fn sync_interface_nodes(&mut self) {
        // Find or create the macro_entry node (replaces subgraph_input)
        let entry_node_id = "macro_entry";
        
        // Check both old and new node IDs for backward compatibility
        let has_entry = self.graph.nodes.contains_key(entry_node_id);
        let has_old_entry = self.graph.nodes.contains_key("subgraph_input");
        
        if has_entry || has_old_entry {
            // Determine which key to use
            let key = if has_entry { entry_node_id } else { "subgraph_input" };
            
            if let Some(entry_node) = self.graph.nodes.get_mut(key) {
                // Update node type if it's the old one
                if entry_node.node_type == "subgraph_input" {
                    entry_node.node_type = "macro_entry".to_string();
                    entry_node.id = entry_node_id.to_string();
                }
                
                // Update existing node pins
                entry_node.outputs.clear();
                for pin in &self.interface.inputs {
                    entry_node.outputs.push(PinInstance {
                        id: pin.id.clone(),
                        pin: Pin {
                            name: pin.name.clone(),
                            pin_type: PinType::Output,
                            data_type: pin.data_type.clone(),
                            connected_to: Vec::new(),
                        },
                    });
                }
            }
        } else {
            // Create new macro entry node
            let mut entry_node = NodeInstance::new(entry_node_id, "macro_entry", Position { x: 100.0, y: 200.0 });
            for pin in &self.interface.inputs {
                entry_node.add_output_pin(&pin.id, pin.data_type.clone());
            }
            self.graph.add_node(entry_node);
        }

        // Remove old subgraph_input if it exists and we created a new macro_entry
        if self.graph.nodes.contains_key("macro_entry") && self.graph.nodes.contains_key("subgraph_input") {
            self.graph.nodes.remove("subgraph_input");
        }

        // Find or create the macro_exit node (replaces subgraph_output)
        let exit_node_id = "macro_exit";
        
        // Check both old and new node IDs for backward compatibility
        let has_exit = self.graph.nodes.contains_key(exit_node_id);
        let has_old_exit = self.graph.nodes.contains_key("subgraph_output");
        
        if has_exit || has_old_exit {
            // Determine which key to use
            let key = if has_exit { exit_node_id } else { "subgraph_output" };
            
            if let Some(exit_node) = self.graph.nodes.get_mut(key) {
                // Update node type if it's the old one
                if exit_node.node_type == "subgraph_output" {
                    exit_node.node_type = "macro_exit".to_string();
                    exit_node.id = exit_node_id.to_string();
                }
                
                // Update existing node pins
                exit_node.inputs.clear();
                for pin in &self.interface.outputs {
                    exit_node.inputs.push(PinInstance {
                        id: pin.id.clone(),
                        pin: Pin {
                            name: pin.name.clone(),
                            pin_type: PinType::Input,
                            data_type: pin.data_type.clone(),
                            connected_to: Vec::new(),
                        },
                    });
                }
            }
        } else {
            // Create new macro exit node
            let mut exit_node = NodeInstance::new(exit_node_id, "macro_exit", Position { x: 800.0, y: 200.0 });
            for pin in &self.interface.outputs {
                exit_node.add_input_pin(&pin.id, pin.data_type.clone());
            }
            self.graph.add_node(exit_node);
        }

        // Remove old subgraph_output if it exists and we created a new macro_exit
        if self.graph.nodes.contains_key("macro_exit") && self.graph.nodes.contains_key("subgraph_output") {
            self.graph.nodes.remove("subgraph_output");
        }
    }

    /// Generate a node instance for this sub-graph/macro (to be placed in parent graphs)
    pub fn create_instance(&self, instance_id: &str, position: Position) -> NodeInstance {
        let mut node = NodeInstance::new(
            instance_id,
            &format!("macro:{}", self.id), // Changed from subgraph: to macro:
            position,
        );

        // Set macro_id property (for compiler expansion)
        node.set_property("macro_id", PropertyValue::String(self.id.clone()));
        
        // Set display properties from macro config
        if let Some(ref icon) = self.macro_config.icon {
            node.set_property("icon", PropertyValue::String(icon.clone()));
        }
        if let Some((r, g, b)) = self.macro_config.color {
            node.set_property("color_r", PropertyValue::Number(r as f64));
            node.set_property("color_g", PropertyValue::Number(g as f64));
            node.set_property("color_b", PropertyValue::Number(b as f64));
        }
        if let Some(ref title) = self.macro_config.compact_node_title {
            node.set_property("compact_title", PropertyValue::String(title.clone()));
        }

        // Add input pins from interface
        for pin in &self.interface.inputs {
            node.inputs.push(PinInstance {
                id: pin.id.clone(),
                pin: Pin {
                    name: pin.name.clone(),
                    pin_type: PinType::Input,
                    data_type: pin.data_type.clone(),
                    connected_to: Vec::new(),
                },
            });
        }

        // Add output pins from interface
        for pin in &self.interface.outputs {
            node.outputs.push(PinInstance {
                id: pin.id.clone(),
                pin: Pin {
                    name: pin.name.clone(),
                    pin_type: PinType::Output,
                    data_type: pin.data_type.clone(),
                    connected_to: Vec::new(),
                },
            });
        }

        node
    }
}

// ===== Sub-Graph Library System =====

/// A library containing reusable sub-graph definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubGraphLibrary {
    /// Library identifier
    pub id: String,

    /// Display name
    pub name: String,

    /// Library version
    pub version: String,

    /// Description
    pub description: String,

    /// Author information
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Library category (e.g., "std", "math", "ui", "custom")
    pub category: String,

    /// All sub-graph definitions in this library
    pub subgraphs: Vec<SubGraphDefinition>,

    /// Library metadata
    pub metadata: LibraryMetadata,

    /// Library-specific configuration
    #[serde(default)]
    pub library_config: LibraryConfiguration,
}

/// Library-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryConfiguration {
    /// If true, this is an engine library (read-only, shipped with engine)
    #[serde(default)]
    pub is_engine_library: bool,

    /// If true, this is a user library (user-created, editable)
    #[serde(default)]
    pub is_user_library: bool,

    /// Which blueprint types can use macros from this library
    /// Empty = available in all blueprint types
    #[serde(default)]
    pub target_blueprint_types: Vec<String>,

    /// Optional library icon emoji or code
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_icon: Option<String>,

    /// Optional library color (RGB 0-1)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_color: Option<(f32, f32, f32)>,

    /// If true, enable hot-reload for this library
    #[serde(default)]
    pub hot_reload_enabled: bool,
}

impl Default for LibraryConfiguration {
    fn default() -> Self {
        Self {
            is_engine_library: false,
            is_user_library: true,
            target_blueprint_types: Vec::new(),
            library_icon: None,
            library_color: None,
            hot_reload_enabled: true,
        }
    }
}

/// Metadata for a library
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryMetadata {
    pub created_at: String,
    pub modified_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl SubGraphLibrary {
    /// Create a new library
    pub fn new(id: &str, name: &str, category: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            author: None,
            category: category.to_string(),
            subgraphs: Vec::new(),
            metadata: LibraryMetadata {
                created_at: chrono::Utc::now().to_rfc3339(),
                modified_at: chrono::Utc::now().to_rfc3339(),
                tags: Vec::new(),
                icon: None,
            },
            library_config: LibraryConfiguration::default(),
        }
    }

    /// Add a sub-graph to the library
    pub fn add_subgraph(&mut self, subgraph: SubGraphDefinition) {
        self.subgraphs.push(subgraph);
        self.metadata.modified_at = chrono::Utc::now().to_rfc3339();
    }

    /// Get a sub-graph by ID
    pub fn get_subgraph(&self, id: &str) -> Option<&SubGraphDefinition> {
        self.subgraphs.iter().find(|sg| sg.id == id)
    }

    /// Save library to JSON file
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load library from JSON file
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let json = std::fs::read_to_string(path)?;
        let library = serde_json::from_str(&json)?;
        Ok(library)
    }
}

/// Manager for loading and caching sub-graph libraries
#[derive(Debug, Clone)]
pub struct LibraryManager {
    /// All loaded libraries
    libraries: HashMap<String, SubGraphLibrary>,

    /// Cache of sub-graph definitions by ID for quick lookup
    subgraph_cache: HashMap<String, SubGraphDefinition>,

    /// Library search paths
    search_paths: Vec<std::path::PathBuf>,
}

impl LibraryManager {
    /// Create a new library manager
    pub fn new() -> Self {
        Self {
            libraries: HashMap::new(),
            subgraph_cache: HashMap::new(),
            search_paths: Vec::new(),
        }
    }

    /// Add a search path for libraries
    pub fn add_search_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.search_paths.push(path.into());
    }

    /// Load all libraries from search paths
    pub fn load_all_libraries(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        for search_path in &self.search_paths.clone() {
            if search_path.exists() && search_path.is_dir() {
                self.load_libraries_from_dir(search_path)?;
            }
        }
        Ok(())
    }

    /// Load all libraries from a directory
    fn load_libraries_from_dir(&mut self, dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Load .json files as libraries
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                match SubGraphLibrary::load_from_file(&path) {
                    Ok(library) => {
                        self.register_library(library);
                    }
                    Err(e) => {
                        tracing::error!("Failed to load library from {:?}: {}", path, e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Register a library
    pub fn register_library(&mut self, library: SubGraphLibrary) {
        // Cache all subgraphs for quick lookup
        for subgraph in &library.subgraphs {
            self.subgraph_cache.insert(subgraph.id.clone(), subgraph.clone());
        }

        self.libraries.insert(library.id.clone(), library);
    }

    /// Get a sub-graph definition by ID
    pub fn get_subgraph(&self, id: &str) -> Option<&SubGraphDefinition> {
        self.subgraph_cache.get(id)
    }

    /// Get all libraries
    pub fn get_libraries(&self) -> &HashMap<String, SubGraphLibrary> {
        &self.libraries
    }

    /// Get all sub-graphs across all libraries (for node creation menu)
    pub fn get_all_subgraphs(&self) -> Vec<&SubGraphDefinition> {
        self.subgraph_cache.values().collect()
    }

    /// Get sub-graphs by category
    pub fn get_subgraphs_by_category(&self, category: &str) -> Vec<&SubGraphDefinition> {
        self.libraries
            .values()
            .filter(|lib| lib.category == category)
            .flat_map(|lib| lib.subgraphs.iter())
            .collect()
    }

    /// Create default standard library path
    pub fn default_stdlib_path() -> std::path::PathBuf {
        std::env::current_dir()
            .unwrap_or_default()
            .join("libraries")
            .join("std")
    }

    /// Create default user library path
    pub fn default_user_library_path() -> std::path::PathBuf {
        std::env::current_dir()
            .unwrap_or_default()
            .join("libraries")
            .join("user")
    }
}

impl Default for LibraryManager {
    fn default() -> Self {
        let mut manager = Self::new();

        // Add default search paths
        manager.add_search_path(Self::default_stdlib_path());
        manager.add_search_path(Self::default_user_library_path());

        manager
    }
}