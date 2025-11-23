// Blueprint Editor Core Types
// All modules are declared in lib.rs, not here

use gpui::*;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use ui::graph::DataType;

// Compilation status tracking for UI feedback
#[derive(Clone, Debug, PartialEq)]
pub enum CompilationState {
    Idle,
    Compiling,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct CompilationStatus {
    pub state: CompilationState,
    pub message: String,
    pub progress: f32, // 0.0 to 1.0
    pub is_compiling: bool,
}

impl Default for CompilationStatus {
    fn default() -> Self {
        Self {
            state: CompilationState::Idle,
            message: "Ready to compile".to_string(),
            progress: 0.0,
            is_compiling: false,
        }
    }
}

// Context menu actions for blueprint editor
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = blueprint_editor)]
pub struct DuplicateNode {
    pub node_id: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = blueprint_editor)]
pub struct DeleteNode {
    pub node_id: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = blueprint_editor)]
pub struct CopyNode {
    pub node_id: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = blueprint_editor)]
pub struct PasteNode;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = blueprint_editor)]
pub struct DisconnectPin {
    pub node_id: String,
    pub pin_id: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = blueprint_editor)]
pub struct OpenAddNodeMenu;

// Event for requesting to open an engine library in main tabs
#[derive(Clone, Debug)]
pub struct OpenEngineLibraryRequest {
    pub library_id: String,
    pub library_name: String,
    pub macro_id: Option<String>, // If specified, open this macro after opening library
    pub macro_name: Option<String>,
}

// Shared types and state
#[derive(Clone, Debug)]
pub struct BlueprintNode {
    pub id: String,
    pub definition_id: String, // ID from NodeDefinition to restore metadata
    pub title: String,
    pub icon: String,
    pub node_type: NodeType,
    pub position: Point<f32>,
    pub size: Size<f32>,
    pub inputs: Vec<Pin>,
    pub outputs: Vec<Pin>,
    pub properties: HashMap<String, String>,
    pub is_selected: bool,
    pub description: String, // Markdown documentation for the node
    pub color: Option<String>, // Custom color from blueprint attribute
}

#[derive(Clone, Debug)]
pub struct Pin {
    pub id: String,
    pub name: String,
    pub pin_type: PinType,
    pub data_type: DataType,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeType {
    Event,
    Logic,
    Math,
    Object,
    Reroute, // Visual pass-through node for organizing connections
    MacroEntry, // Entry point for macro graphs (replaces generic subgraph_input)
    MacroExit, // Exit point for macro graphs (replaces generic subgraph_output)
    MacroInstance, // Instance of a macro in parent graph
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PinType {
    Input,
    Output,
}

// DataType is now imported from crate::graph module
// This provides the new TypeInfo-based type system with deterministic colors

#[derive(Clone, Debug)]
pub struct Connection {
    pub id: String,
    pub source_node: String,
    pub source_pin: String,
    pub target_node: String,
    pub target_pin: String,
    pub connection_type: ui::graph::ConnectionType,
}

use ui::color_picker::ColorPickerState;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlueprintComment {
    pub id: String,
    pub text: String,
    #[serde(with = "point_serde")]
    pub position: Point<f32>,
    #[serde(with = "size_serde")]
    pub size: Size<f32>,
    #[serde(with = "hsla_serde")]
    pub color: Hsla, // Background color
    pub contained_node_ids: Vec<String>, // Nodes fully contained in this comment
    #[serde(skip)]
    pub is_selected: bool,
    #[serde(skip)]
    pub color_picker_state: Option<gpui::Entity<ColorPickerState>>,
}

// Serde helpers for GPUI types
mod point_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct PointData {
        x: f32,
        y: f32,
    }

    pub fn serialize<S>(point: &Point<f32>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        PointData { x: point.x, y: point.y }.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Point<f32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data = PointData::deserialize(deserializer)?;
        Ok(Point::new(data.x, data.y))
    }
}

mod size_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct SizeData {
        width: f32,
        height: f32,
    }

    pub fn serialize<S>(size: &Size<f32>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SizeData { width: size.width, height: size.height }.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Size<f32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data = SizeData::deserialize(deserializer)?;
        Ok(Size::new(data.width, data.height))
    }
}

mod hsla_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct HslaData {
        h: f32,
        s: f32,
        l: f32,
        a: f32,
    }

    pub fn serialize<S>(color: &Hsla, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        HslaData { h: color.h, s: color.s, l: color.l, a: color.a }.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Hsla, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data = HslaData::deserialize(deserializer)?;
        Ok(Hsla { h: data.h, s: data.s, l: data.l, a: data.a })
    }
}

impl BlueprintComment {
    pub fn new(position: Point<f32>, window: &mut gpui::Window, cx: &mut gpui::Context<BlueprintEditorPanel>) -> Self {
        let color_picker_state = Some(cx.new(|cx| {
            ColorPickerState::new(window, cx)
        }));
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            text: "Comment".to_string(),
            position,
            size: Size::new(300.0, 200.0),
            color: Hsla { h: 0.5, s: 0.3, l: 0.2, a: 0.3 }, // Default semi-transparent color
            contained_node_ids: Vec::new(),
            is_selected: false,
            color_picker_state,
        }
    }

    /// Check if a node is fully contained within this comment's bounds
    pub fn contains_node(&self, node: &BlueprintNode) -> bool {
        let node_left = node.position.x;
        let node_top = node.position.y;
        let node_right = node.position.x + node.size.width;
        let node_bottom = node.position.y + node.size.height;

        let comment_left = self.position.x;
        let comment_top = self.position.y;
        let comment_right = self.position.x + self.size.width;
        let comment_bottom = self.position.y + self.size.height;

        node_left >= comment_left
            && node_right <= comment_right
            && node_top >= comment_top
            && node_bottom <= comment_bottom
    }

    /// Update contained nodes based on current bounds
    pub fn update_contained_nodes(&mut self, nodes: &[BlueprintNode]) {
        self.contained_node_ids = nodes
            .iter()
            .filter(|node| self.contains_node(node))
            .map(|node| node.id.clone())
            .collect();
    }
}

#[derive(Clone, Debug)]
pub struct BlueprintGraph {
    pub nodes: Vec<BlueprintNode>,
    pub connections: Vec<Connection>,
    pub comments: Vec<BlueprintComment>,
    pub selected_nodes: Vec<String>,
    pub selected_comments: Vec<String>,
    pub zoom_level: f32,
    pub pan_offset: Point<f32>,
    pub virtualization_stats: VirtualizationStats,
}

#[derive(Clone, Debug, Default)]
pub struct VirtualizationStats {
    pub total_nodes: usize,
    pub rendered_nodes: usize,
    pub total_connections: usize,
    pub rendered_connections: usize,
    pub last_update_ms: f32,
}

// JSON schema structures for loading node definitions
#[derive(Debug, Deserialize)]
pub struct NodeDefinitions {
    pub categories: Vec<NodeCategory>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeCategory {
    pub name: String,
    pub color: String,
    pub nodes: Vec<NodeDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeDefinition {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub description: String,  // Short one-line description for list display
    pub documentation: String,  // Full markdown documentation for docs panel
    pub inputs: Vec<PinDefinition>,
    pub outputs: Vec<PinDefinition>,
    pub properties: HashMap<String, String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PinDefinition {
    pub id: String,
    pub name: String,
    pub data_type: DataType,
    pub pin_type: PinType,
}

// Global node definitions (loaded once at startup)
use std::sync::OnceLock;
static NODE_DEFINITIONS: OnceLock<NodeDefinitions> = OnceLock::new();

impl NodeDefinitions {
    pub fn load() -> &'static NodeDefinitions {
        NODE_DEFINITIONS.get_or_init(|| {
            // Load dynamic node definitions from pulsar_std
            let metadata = ui::compiler::node_metadata::extract_node_metadata()
                .unwrap_or_else(|e| {
                    eprintln!("Failed to load node metadata: {}", e);
                    std::collections::HashMap::new()
                });

            // Load sub-graph libraries
            let mut lib_manager = ui::graph::LibraryManager::default();
            if let Err(e) = lib_manager.load_all_libraries() {
                eprintln!("Failed to load sub-graph libraries: {}", e);
            }

            // Convert metadata to UI format (includes both regular nodes and sub-graphs)
            Self::from_node_metadata_and_libraries(metadata, lib_manager)
        })
    }

    fn from_node_metadata_and_libraries(
        metadata: std::collections::HashMap<String, ui::compiler::NodeMetadata>,
        lib_manager: ui::graph::LibraryManager
    ) -> NodeDefinitions {
        let mut categories_map: std::collections::HashMap<String, Vec<NodeDefinition>> = std::collections::HashMap::new();

        // First, add all sub-graphs from libraries
        for library in lib_manager.get_libraries().values() {
            let category_name = format!("Macros/{}", library.name);

            for subgraph in &library.subgraphs {
                // Convert sub-graph inputs to pin definitions
                let inputs: Vec<PinDefinition> = subgraph.interface.inputs.iter().map(|pin| {
                    PinDefinition {
                        id: pin.id.clone(),
                        name: pin.name.clone(),
                        data_type: pin.data_type.clone(),
                        pin_type: PinType::Input,
                    }
                }).collect();

                // Convert sub-graph outputs to pin definitions
                let outputs: Vec<PinDefinition> = subgraph.interface.outputs.iter().map(|pin| {
                    PinDefinition {
                        id: pin.id.clone(),
                        name: pin.name.clone(),
                        data_type: pin.data_type.clone(),
                        pin_type: PinType::Output,
                    }
                }).collect();

                let node_def = NodeDefinition {
                    id: format!("subgraph:{}", subgraph.id),
                    name: subgraph.name.clone(),
                    icon: "📦".to_string(), // Macro icon
                    description: subgraph.description.clone(),
                    documentation: subgraph.description.clone(),  // Use same text for docs
                    inputs,
                    outputs,
                    properties: std::collections::HashMap::new(),
                    color: Some("#9B59B6".to_string()), // Purple for macros
                };

                categories_map
                    .entry(category_name.clone())
                    .or_insert_with(Vec::new)
                    .push(node_def);
            }
        }

        // Then add regular nodes from metadata
        Self::populate_categories_from_metadata(metadata, &mut categories_map);

        // Convert to NodeDefinitions
        Self::categories_to_definitions(categories_map)
    }

    fn from_node_metadata(metadata: std::collections::HashMap<String, ui::compiler::NodeMetadata>) -> NodeDefinitions {
        let mut categories_map: std::collections::HashMap<String, Vec<NodeDefinition>> = std::collections::HashMap::new();
        Self::populate_categories_from_metadata(metadata, &mut categories_map);
        Self::categories_to_definitions(categories_map)
    }

    fn populate_categories_from_metadata(
        metadata: std::collections::HashMap<String, ui::compiler::NodeMetadata>,
        categories_map: &mut std::collections::HashMap<String, Vec<NodeDefinition>>
    ) {
        // Add special reroute node to Utility category
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
                properties: std::collections::HashMap::new(),
                color: None,
            });

        // Group nodes by category
        for (id, node_meta) in metadata {
            let mut inputs = Vec::new();
            let mut outputs = Vec::new();

            // Add execution inputs
            for exec_pin in node_meta.exec_inputs.iter() {
                inputs.push(PinDefinition {
                    id: exec_pin.to_string(),
                    name: exec_pin.to_string(),
                    data_type: DataType::from_type_str("execution"),
                    pin_type: PinType::Input,
                });
            }

            // Add regular inputs
            for param in node_meta.params.iter() {
                inputs.push(PinDefinition {
                    id: param.name.to_string(),
                    name: param.name.to_string(),
                    data_type: DataType::from_type_str(&param.ty),
                    pin_type: PinType::Input,
                });
            }

            // Add execution outputs
            for exec_pin in node_meta.exec_outputs.iter() {
                outputs.push(PinDefinition {
                    id: exec_pin.to_string(),
                    name: exec_pin.to_string(),
                    data_type: DataType::from_type_str("execution"),
                    pin_type: PinType::Output,
                });
            }

            // Add regular outputs (return type)
            if let Some(return_type) = node_meta.return_type {
                outputs.push(PinDefinition {
                    id: "result".to_string(),
                    name: "result".to_string(),
                    data_type: DataType::from_type_str(return_type),
                    pin_type: PinType::Output,
                });
            }

            let category = node_meta.category.to_string();

            // Use first line of docs as short description, full docs for documentation panel
            let documentation = node_meta.documentation.join("\n");
            let description = node_meta.documentation
                .first()
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| format!("Node: {}", node_meta.name));

            let static_def = NodeDefinition {
                id: id.clone(),
                name: node_meta.name.to_string(),
                icon: "⚙️".to_string(), // Default icon
                description,
                documentation,
                inputs,
                outputs,
                properties: std::collections::HashMap::new(),
                color: node_meta.color.map(|s| s.to_string()),
            };

            categories_map
                .entry(category)
                .or_insert_with(Vec::new)
                .push(static_def);
        }
    }

    fn categories_to_definitions(categories_map: std::collections::HashMap<String, Vec<NodeDefinition>>) -> NodeDefinitions {
        // Convert to categories
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

    fn convert_data_type(data_type: &str) -> DataType {
        // Use the new DataType system that supports TypeInfo
        DataType::from_type_str(data_type)
    }

    fn get_category_color(category: &str) -> String {
        match category {
            "Math" | "Math/Vector" => "#4A90E2".to_string(),
            "Logic" => "#E2A04A".to_string(),
            "String" => "#7ED321".to_string(),
            "Array" => "#BD10E0".to_string(),
            "File I/O" => "#50E3C2".to_string(),
            "Graphics" => "#F5A623".to_string(),
            "Time" => "#9013FE".to_string(),
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

impl BlueprintNode {
    pub fn from_definition(definition: &NodeDefinition, position: Point<f32>) -> Self {
        let inputs: Vec<Pin> = definition.inputs.iter().map(|pin_def| Pin {
            id: pin_def.id.clone(),
            name: pin_def.name.clone(),
            pin_type: pin_def.pin_type.clone(),
            data_type: pin_def.data_type.clone(),
        }).collect();

        let outputs: Vec<Pin> = definition.outputs.iter().map(|pin_def| Pin {
            id: pin_def.id.clone(),
            name: pin_def.name.clone(),
            pin_type: pin_def.pin_type.clone(),
            data_type: pin_def.data_type.clone(),
        }).collect();

        // Determine node type based on category
        let node_definitions = NodeDefinitions::load();
        let category = node_definitions.get_category_for_node(&definition.id);
        let node_type = match category.map(|c| c.name.as_str()) {
            Some("Events") => NodeType::Event,
            Some("Logic") => NodeType::Logic,
            Some("Math") => NodeType::Math,
            Some("Object") => NodeType::Object,
            _ => NodeType::Logic,
        };

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            definition_id: definition.id.clone(),
            title: definition.name.clone(),
            icon: definition.icon.clone(),
            node_type,
            position,
            size: Size::new(150.0, 100.0), // Default size
            inputs,
            outputs,
            properties: definition.properties.clone(),
            is_selected: false,
            description: definition.description.clone(),
            color: definition.color.clone(),
        }
    }

    /// Create a typeless reroute node at the given position
    /// The type will be inferred from the first connection made to it
    pub fn create_reroute(position: Point<f32>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            definition_id: "reroute".to_string(),
            title: "Reroute".to_string(),
            icon: "•".to_string(),
            node_type: NodeType::Reroute,
            position,
            size: Size::new(16.0, 16.0), // Small size for reroute nodes
            inputs: vec![Pin {
                id: "input".to_string(),
                name: "".to_string(),
                pin_type: PinType::Input,
                data_type: DataType::Any, // Start as typeless
            }],
            outputs: vec![Pin {
                id: "output".to_string(),
                name: "".to_string(),
                pin_type: PinType::Output,
                data_type: DataType::Any, // Start as typeless
            }],
            properties: HashMap::new(),
            is_selected: false,
            description: "Reroute node for organizing connections".to_string(),
            color: None,
        }
    }
}
