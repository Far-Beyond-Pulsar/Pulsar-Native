//! Menu operations - node picker (command palette-style)

use gpui::*;
use super::core::BlueprintEditorPanel;
use super::super::events::ShowNodePickerRequest;

impl BlueprintEditorPanel {
    /// Show node picker at graph position
    /// Emits an event that the parent app handles by showing the global palette
    pub fn show_node_picker(
        &mut self,
        graph_pos: Point<f32>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Emit event to request node picker from global palette
        cx.emit(ShowNodePickerRequest {
            graph_position: graph_pos,
        });
    }

    /// Add a node to the graph (called by parent when node is selected from picker)
    pub fn add_node_from_definition(
        &mut self,
        node_def: &super::super::NodeDefinition,
        pos: Point<f32>,
        cx: &mut Context<Self>,
    ) {
        let node = super::super::BlueprintNode::from_definition(node_def, pos);
        self.add_node(node, cx);
    }
}
