//! Connection operations - dragging and managing connections between nodes

use gpui::*;
use super::core::{BlueprintEditorPanel, ConnectionDrag};
use super::super::{Connection, NodeType};
use ui::graph::DataType as GraphDataType;

impl BlueprintEditorPanel {
    /// Start dragging a connection from a pin
    pub fn start_connection_drag_from_pin(
        &mut self,
        node_id: String,
        pin_id: String,
        mouse_pos: Point<f32>,
        cx: &mut Context<Self>,
    ) {
        if let Some(node) = self.graph.nodes.iter().find(|n| n.id == node_id) {
            if let Some(pin) = node.outputs.iter().find(|p| p.id == pin_id) {
                println!("Starting connection drag from pin {} on node {}", pin_id, node_id);
                self.dragging_connection = Some(ConnectionDrag {
                    source_node: node_id,
                    source_pin: pin_id,
                    source_pin_type: pin.data_type.clone(),
                    current_mouse_pos: mouse_pos,
                    target_pin: None,
                });

                // Close tooltips when starting connection drag
                // Tooltip removed - use node picker instead
                cx.notify();
            }
        }
    }

    /// Update connection drag position
    pub fn update_connection_drag(&mut self, mouse_pos: Point<f32>, cx: &mut Context<Self>) {
        if let Some(ref mut drag) = self.dragging_connection {
            drag.current_mouse_pos = mouse_pos;
            cx.notify();
        }
    }

    /// Cancel connection drag
    pub fn cancel_connection_drag(&mut self, cx: &mut Context<Self>) {
        self.dragging_connection = None;
        cx.notify();
    }

    /// Set connection target (hovering over a pin)
    pub fn set_connection_target(
        &mut self,
        target_node_id: Option<String>,
        target_pin_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(ref mut drag) = self.dragging_connection {
            drag.target_pin = target_node_id.zip(target_pin_id);
            cx.notify();
        }
    }

    /// Complete connection on a pin
    pub fn complete_connection_on_pin(
        &mut self,
        node_id: String,
        pin_id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.dragging_connection.take() {
            // Validate connection
            if let Some(node) = self.graph.nodes.iter().find(|n| n.id == node_id) {
                if let Some(pin) = node.inputs.iter().find(|p| p.id == pin_id) {
                    // Check compatibility and not same node
                    if Self::are_types_compatible(&drag.source_pin_type, &pin.data_type)
                        && drag.source_node != node_id
                    {
                        // Check if source or target is a reroute node
                        let source_is_reroute = self.graph.nodes.iter()
                            .any(|n| n.id == drag.source_node && n.node_type == NodeType::Reroute);
                        let target_is_reroute = self.graph.nodes.iter()
                            .any(|n| n.id == node_id && n.node_type == NodeType::Reroute);

                        // Remove old connections based on pin types
                        if drag.source_pin_type == GraphDataType::Execution || source_is_reroute {
                            // Execution pins and reroute outputs: single connection from source
                            println!("Removing old connection from source {}:{}", drag.source_node, drag.source_pin);
                            self.graph.connections.retain(|conn| {
                                !(conn.source_node == drag.source_node && conn.source_pin == drag.source_pin)
                            });
                        }
                        
                        if drag.source_pin_type == GraphDataType::Execution || target_is_reroute || pin.data_type != GraphDataType::Execution {
                            // Execution targets, reroute inputs, or data inputs: single connection to target
                            println!("Removing old connection to target {}:{}", node_id, pin_id);
                            self.graph.connections.retain(|conn| {
                                !(conn.target_node == node_id && conn.target_pin == pin_id)
                            });
                        }

                        println!("Creating connection from {}:{} to {}:{}", 
                            drag.source_node, drag.source_pin, node_id, pin_id);

                        // Create new connection
                        let connection_type = if pin.data_type == GraphDataType::Execution {
                            ui::graph::ConnectionType::Execution
                        } else {
                            ui::graph::ConnectionType::Data
                        };
                        
                        let connection = Connection {
                            id: uuid::Uuid::new_v4().to_string(),
                            source_node: drag.source_node.clone(),
                            source_pin: drag.source_pin.clone(),
                            target_node: node_id.clone(),
                            target_pin: pin_id.clone(),
                            connection_type,
                        };
                        self.graph.connections.push(connection);
                        println!("Connection created successfully!");

                        // Propagate types through reroute nodes
                        if target_is_reroute {
                            self.propagate_reroute_types(node_id.clone(), drag.source_pin_type, cx);
                        } else if source_is_reroute {
                            self.propagate_reroute_types(drag.source_node.clone(), pin.data_type.clone(), cx);
                        }

                        cx.notify();
                    } else {
                        println!("Incompatible pin types or same node");
                    }
                }
            }
        }
    }

    /// Disconnect a pin
    pub fn disconnect_pin(&mut self, node_id: String, pin_id: String, cx: &mut Context<Self>) {
        self.graph.connections.retain(|conn| {
            !(conn.source_node == node_id && conn.source_pin == pin_id)
                && !(conn.target_node == node_id && conn.target_pin == pin_id)
        });
        cx.notify();
    }

    /// Check if two data types are compatible for connection
    fn are_types_compatible(from_type: &GraphDataType, to_type: &GraphDataType) -> bool {
        from_type == to_type || *from_type == GraphDataType::Any || *to_type == GraphDataType::Any
    }

    /// Propagate types through connected reroute nodes
    pub(super) fn propagate_reroute_types(
        &mut self,
        start_node_id: String,
        data_type: GraphDataType,
        cx: &mut Context<Self>,
    ) {
        use std::collections::{HashSet, VecDeque};
        
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(start_node_id);

        while let Some(node_id) = queue.pop_front() {
            if visited.contains(&node_id) {
                continue;
            }
            visited.insert(node_id.clone());

            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == node_id) {
                if node.node_type == NodeType::Reroute {
                    // Update pin types
                    for pin in &mut node.inputs {
                        pin.data_type = data_type.clone();
                    }
                    for pin in &mut node.outputs {
                        pin.data_type = data_type.clone();
                    }

                    // Find connected reroute nodes
                    for connection in &self.graph.connections {
                        if connection.source_node == node_id {
                            if let Some(target_node) = self.graph.nodes.iter()
                                .find(|n| n.id == connection.target_node)
                            {
                                if target_node.node_type == NodeType::Reroute {
                                    queue.push_back(connection.target_node.clone());
                                }
                            }
                        } else if connection.target_node == node_id {
                            if let Some(source_node) = self.graph.nodes.iter()
                                .find(|n| n.id == connection.source_node)
                            {
                                if source_node.node_type == NodeType::Reroute {
                                    queue.push_back(connection.source_node.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        cx.notify();
    }

    /// Get data type of a connection
    pub(super) fn get_connection_data_type(&self, connection: &Connection) -> Option<GraphDataType> {
        let from_node = self.graph.nodes.iter().find(|n| n.id == connection.source_node)?;
        let output_pin = from_node.outputs.iter().find(|p| p.id == connection.source_pin)?;
        Some(output_pin.data_type.clone())
    }

    /// Find connection near a point (for double-click reroute creation)
    pub(super) fn find_connection_near_point(&self, point: Point<f32>) -> Option<Connection> {
        const CLICK_THRESHOLD: f32 = 30.0;

        for connection in &self.graph.connections {
            let from_node = self.graph.nodes.iter().find(|n| n.id == connection.source_node)?;
            let to_node = self.graph.nodes.iter().find(|n| n.id == connection.target_node)?;

            // Calculate pin positions (simplified - using node edges)
            let from_pos = Point::new(
                from_node.position.x + from_node.size.width,
                from_node.position.y + from_node.size.height / 2.0,
            );
            let to_pos = Point::new(
                to_node.position.x,
                to_node.position.y + to_node.size.height / 2.0,
            );

            // Check if point is near connection line
            if Self::point_near_bezier(point, from_pos, to_pos, CLICK_THRESHOLD) {
                return Some(connection.clone());
            }
        }

        None
    }

    /// Check if point is near a bezier curve (simplified linear approximation)
    fn point_near_bezier(point: Point<f32>, start: Point<f32>, end: Point<f32>, threshold: f32) -> bool {
        // Simplified: check distance to line segment
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length_sq = dx * dx + dy * dy;
        
        if length_sq == 0.0 {
            let dist = ((point.x - start.x).powi(2) + (point.y - start.y).powi(2)).sqrt();
            return dist <= threshold;
        }

        let t = ((point.x - start.x) * dx + (point.y - start.y) * dy) / length_sq;
        let t = t.clamp(0.0, 1.0);
        
        let closest_x = start.x + t * dx;
        let closest_y = start.y + t * dy;
        
        let dist = ((point.x - closest_x).powi(2) + (point.y - closest_y).powi(2)).sqrt();
        dist <= threshold
    }
}
