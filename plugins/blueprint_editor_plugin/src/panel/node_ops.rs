//! Node operations - create, delete, duplicate, copy/paste
//!
//! All operations related to node manipulation in the graph.

use gpui::*;
use super::core::BlueprintEditorPanel;
use super::super::{BlueprintNode, NodeType};

impl BlueprintEditorPanel {
    /// Add a node to the graph
    pub fn add_node(&mut self, node: BlueprintNode, cx: &mut Context<Self>) {
        println!("Adding node: {} at position {:?}", node.title, node.position);
        self.graph.nodes.push(node);
        
        // Mark tab as dirty
        if let Some(tab) = self.open_tabs.get_mut(self.active_tab_index) {
            tab.is_dirty = true;
        }
        cx.notify();
    }

    /// Duplicate a node
    pub fn duplicate_node(&mut self, node_id: String, cx: &mut Context<Self>) {
        if let Some(node) = self.graph.nodes.iter().find(|n| n.id == node_id).cloned() {
            let mut new_node = node;
            new_node.id = uuid::Uuid::new_v4().to_string();
            new_node.position.x += 20.0;
            new_node.position.y += 20.0;
            new_node.is_selected = false;
            self.graph.nodes.push(new_node);
            cx.notify();
        }
    }

    /// Delete a node and its connections
    pub fn delete_node(&mut self, node_id: String, cx: &mut Context<Self>) {
        self.graph.nodes.retain(|n| n.id != node_id);
        self.graph.connections.retain(|conn| {
            conn.source_node != node_id && conn.target_node != node_id
        });
        self.graph.selected_nodes.retain(|id| *id != node_id);
        cx.notify();
    }

    /// Copy node (placeholder)
    pub fn copy_node(&mut self, node_id: String, _cx: &mut Context<Self>) {
        if let Some(node) = self.graph.nodes.iter().find(|n| n.id == node_id) {
            println!("Copied node: {}", node.title);
            // TODO: Store in clipboard
        }
    }

    /// Paste node (placeholder)
    pub fn paste_node(&mut self, cx: &mut Context<Self>) {
        println!("Paste node not yet implemented");
        cx.notify();
    }

    /// Start dragging a node
    pub fn start_drag(&mut self, node_id: String, mouse_pos: Point<f32>, cx: &mut Context<Self>) {
        println!("Starting drag for node {} at mouse position {:?}", node_id, mouse_pos);
        
        if let Some(node) = self.graph.nodes.iter().find(|n| n.id == node_id) {
            self.dragging_node = Some(node_id.clone());
            self.drag_offset = Point::new(
                mouse_pos.x - node.position.x,
                mouse_pos.y - node.position.y
            );

            // Store initial positions for multi-select drag
            self.initial_drag_positions.clear();
            
            if self.graph.selected_nodes.contains(&node_id) {
                // Drag all selected nodes
                for selected_id in &self.graph.selected_nodes {
                    if let Some(selected_node) = self.graph.nodes.iter().find(|n| n.id == *selected_id) {
                        self.initial_drag_positions.insert(selected_id.clone(), selected_node.position);
                    }
                }
            } else {
                // Drag only this node
                self.initial_drag_positions.insert(node_id.clone(), node.position);
            }
            
            cx.notify();
        }
    }

    /// Update drag position
    pub fn update_drag(&mut self, mouse_pos: Point<f32>, cx: &mut Context<Self>) {
        if let Some(dragging_id) = &self.dragging_node.clone() {
            let new_position = Point::new(
                mouse_pos.x - self.drag_offset.x,
                mouse_pos.y - self.drag_offset.y
            );

            if let Some(initial_pos) = self.initial_drag_positions.get(dragging_id) {
                let delta = Point::new(
                    new_position.x - initial_pos.x,
                    new_position.y - initial_pos.y
                );

                // Move all nodes that were selected when dragging started
                for (node_id, initial_position) in &self.initial_drag_positions {
                    if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == *node_id) {
                        node.position = Point::new(
                            initial_position.x + delta.x,
                            initial_position.y + delta.y
                        );
                    }
                }
            }

            cx.notify();
        }
    }

    /// End drag operation
    pub fn end_drag(&mut self, cx: &mut Context<Self>) {
        // Update comment containment after drag
        for comment in self.graph.comments.iter_mut() {
            comment.update_contained_nodes(&self.graph.nodes);
        }

        self.dragging_node = None;
        cx.notify();
    }
}
