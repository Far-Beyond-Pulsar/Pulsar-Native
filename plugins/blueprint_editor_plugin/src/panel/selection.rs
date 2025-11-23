//! Selection operations - selection box and multi-selection

use gpui::*;
use super::core::BlueprintEditorPanel;
use super::super::{BlueprintNode, Connection};

impl BlueprintEditorPanel {
    /// Select a single node (or clear selection if None)
    pub fn select_node(&mut self, node_id: Option<String>, cx: &mut Context<Self>) {
        self.graph.selected_nodes.clear();
        if let Some(id) = node_id {
            self.graph.selected_nodes.push(id);
        }
        cx.notify();
    }

    /// Start selection drag (selection box)
    pub fn start_selection_drag(
        &mut self,
        start_pos: Point<f32>,
        _add_to_selection: bool,
        cx: &mut Context<Self>,
    ) {
        self.selection_start = Some(start_pos);
        self.selection_end = Some(start_pos);
        cx.notify();
    }

    /// Check if currently selecting
    pub fn is_selecting(&self) -> bool {
        self.selection_start.is_some() && self.selection_end.is_some()
    }

    /// Update selection drag
    pub fn update_selection_drag(&mut self, current_pos: Point<f32>, cx: &mut Context<Self>) {
        if self.selection_start.is_some() {
            self.selection_end = Some(current_pos);
            self.update_node_selection_from_drag(cx);
        }
    }

    /// End selection drag
    pub fn end_selection_drag(&mut self, cx: &mut Context<Self>) {
        // If selection box was very small, treat as click and clear selection
        if let (Some(start), Some(end)) = (self.selection_start, self.selection_end) {
            let distance = ((end.x - start.x).powi(2) + (end.y - start.y).powi(2)).sqrt();
            if distance < 5.0 {
                self.graph.selected_nodes.clear();
                println!("[SELECTION] Cleared selection (click on empty space)");
            }
        }

        self.selection_start = None;
        self.selection_end = None;
        cx.notify();
    }

    /// Update node selection based on current drag area
    fn update_node_selection_from_drag(&mut self, cx: &mut Context<Self>) {
        if let (Some(start), Some(end)) = (self.selection_start, self.selection_end) {
            let min_x = start.x.min(end.x);
            let max_x = start.x.max(end.x);
            let min_y = start.y.min(end.y);
            let max_y = start.y.max(end.y);

            // Check all nodes for intersection with selection box
            for node in &self.graph.nodes {
                let node_left = node.position.x;
                let node_top = node.position.y;
                let node_right = node.position.x + node.size.width;
                let node_bottom = node.position.y + node.size.height;

                // Check intersection
                let intersects = !(node_right < min_x
                    || node_left > max_x
                    || node_bottom < min_y
                    || node_top > max_y);

                if intersects {
                    if !self.graph.selected_nodes.contains(&node.id) {
                        self.graph.selected_nodes.push(node.id.clone());
                    }
                } else {
                    self.graph.selected_nodes.retain(|id| id != &node.id);
                }
            }
            cx.notify();
        }
    }

    /// Delete all selected nodes
    pub fn delete_selected_nodes(&mut self, cx: &mut Context<Self>) {
        println!("[DELETE] Selected nodes count: {}", self.graph.selected_nodes.len());
        println!("[DELETE] Selected node IDs: {:?}", self.graph.selected_nodes);

        if !self.graph.selected_nodes.is_empty() {
            let node_count_before = self.graph.nodes.len();

            // Remove selected nodes
            self.graph.nodes.retain(|node| !self.graph.selected_nodes.contains(&node.id));

            let node_count_after = self.graph.nodes.len();
            println!(
                "[DELETE] Deleted {} nodes ({} -> {})",
                node_count_before - node_count_after,
                node_count_before,
                node_count_after
            );

            // Remove connections involving deleted nodes
            self.graph.connections.retain(|connection| {
                !self.graph.selected_nodes.contains(&connection.source_node)
                    && !self.graph.selected_nodes.contains(&connection.target_node)
            });

            self.graph.selected_nodes.clear();
            cx.notify();
        } else {
            println!("[DELETE] No nodes selected, nothing to delete");
        }
    }

    /// Handle double-click on connection to create reroute node
    pub fn handle_empty_space_click(
        &mut self,
        graph_pos: Point<f32>,
        cx: &mut Context<Self>,
    ) -> bool {
        let now = std::time::Instant::now();
        let is_double_click = if let (Some(last_time), Some(last_pos)) =
            (self.last_click_time, self.last_click_pos)
        {
            let time_diff = now.duration_since(last_time).as_millis();
            let pos_diff =
                ((graph_pos.x - last_pos.x).powi(2) + (graph_pos.y - last_pos.y).powi(2)).sqrt();
            println!(
                "[REROUTE] Double-click check: time_diff={}ms, pos_diff={:.2}px",
                time_diff, pos_diff
            );
            time_diff < 500 && pos_diff < 50.0
        } else {
            false
        };

        if is_double_click {
            println!("[REROUTE] Double-click detected! Checking for nearby connections...");
            
            if let Some(connection) = self.find_connection_near_point(graph_pos) {
                println!("[REROUTE] Found connection near click point!");
                
                if let Some(data_type) = self.get_connection_data_type(&connection) {
                    // Create reroute node
                    let reroute_node = BlueprintNode::create_reroute(graph_pos);
                    let reroute_id = reroute_node.id.clone();

                    self.graph.nodes.push(reroute_node);

                    // Split connection
                    let from_node = connection.source_node.clone();
                    let from_pin = connection.source_pin.clone();
                    let to_node = connection.target_node.clone();
                    let to_pin = connection.target_pin.clone();

                    // Remove original connection
                    self.graph.connections.retain(|c| c.id != connection.id);

                    // Create two new connections through reroute
                    self.graph.connections.push(Connection {
                        id: uuid::Uuid::new_v4().to_string(),
                        source_node: from_node,
                        source_pin: from_pin,
                        target_node: reroute_id.clone(),
                        target_pin: "input".to_string(),
                        connection_type: connection.connection_type.clone(),
                    });

                    self.graph.connections.push(Connection {
                        id: uuid::Uuid::new_v4().to_string(),
                        source_node: reroute_id.clone(),
                        source_pin: "output".to_string(),
                        target_node: to_node,
                        target_pin: to_pin,
                        connection_type: connection.connection_type.clone(),
                    });

                    // Propagate types
                    self.propagate_reroute_types(reroute_id, data_type, cx);

                    cx.notify();
                    self.last_click_time = None;
                    self.last_click_pos = None;
                    return true;
                }
            }

            self.last_click_time = None;
            self.last_click_pos = None;
        } else {
            self.last_click_time = Some(now);
            self.last_click_pos = Some(graph_pos);
        }

        false
    }
}
