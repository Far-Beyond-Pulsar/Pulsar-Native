use gpui::Point;
use ui::IconName;
use ui_common::command_palette::{PaletteDelegate, PaletteItem};

use super::{NodeCategory, NodeDefinition, NodeDefinitions};

/// Event emitted when a node is selected from the palette
#[derive(Clone)]
pub struct NodeSelected {
    pub node_def: NodeDefinition,
    pub position: Point<f32>,
}

/// Implement PaletteItem for NodeDefinition
impl PaletteItem for NodeDefinition {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn icon(&self) -> IconName {
        // Map the string icon to IconName
        // For now, use Code as default
        IconName::Code
    }

    fn documentation(&self) -> Option<String> {
        // Return the full markdown documentation from the documentation field
        if self.documentation.is_empty() {
            None
        } else {
            Some(self.documentation.clone())
        }
    }
}

/// Palette delegate for selecting blueprint nodes
pub struct NodePalette {
    categories: Vec<(String, Vec<NodeDefinition>)>,
    graph_position: Point<f32>,
    selected_node: Option<NodeDefinition>,
}

impl NodePalette {
    pub fn new(graph_position: Point<f32>) -> Self {
        let node_defs = NodeDefinitions::load();
        let categories: Vec<(String, Vec<NodeDefinition>)> = node_defs
            .categories
            .iter()
            .map(|cat| (cat.name.clone(), cat.nodes.clone()))
            .collect();

        Self {
            categories,
            graph_position,
            selected_node: None,
        }
    }

    pub fn take_selected_node(&mut self) -> Option<(NodeDefinition, Point<f32>)> {
        self.selected_node.take().map(|node| (node, self.graph_position))
    }
}

impl PaletteDelegate for NodePalette {
    type Item = NodeDefinition;

    fn placeholder(&self) -> &str {
        "Search for nodes..."
    }

    fn categories(&self) -> Vec<(String, Vec<Self::Item>)> {
        self.categories.clone()
    }

    fn confirm(&mut self, item: &Self::Item) {
        self.selected_node = Some(item.clone());
    }

    fn categories_collapsed_by_default(&self) -> bool {
        true  // Start with categories collapsed for cleaner UI
    }

    fn supports_docs(&self) -> bool {
        true
    }
}

