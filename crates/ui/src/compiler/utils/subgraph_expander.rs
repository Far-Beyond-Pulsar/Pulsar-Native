//! Sub-graph expansion for blueprint compiler
//!
//! This module handles the expansion of sub-graph instances into their constituent nodes
//! before compilation. Sub-graphs can be nested, so expansion happens recursively in
//! topological order (innermost first).

use std::collections::{HashMap, HashSet};
use crate::graph::{
    GraphDescription, NodeInstance, Connection, SubGraphDefinition, LibraryManager,
};

/// Expands all sub-graph instances in a graph recursively
pub struct SubGraphExpander {
    /// Library manager for loading sub-graph definitions
    library_manager: LibraryManager,

    /// Track expansion depth to prevent infinite recursion
    max_depth: usize,
}

impl SubGraphExpander {
    /// Create a new sub-graph expander
    pub fn new(library_manager: LibraryManager) -> Self {
        Self {
            library_manager,
            max_depth: 100, // Prevent infinite recursion
        }
    }

    /// Expand all sub-graphs in a graph recursively
    pub fn expand_all(&self, graph: &mut GraphDescription) -> Result<(), String> {
        self.expand_recursive(graph, 0)
    }

    /// Recursively expand sub-graphs
    fn expand_recursive(&self, graph: &mut GraphDescription, depth: usize) -> Result<(), String> {
        if depth > self.max_depth {
            return Err(format!(
                "Maximum sub-graph nesting depth ({}) exceeded - possible circular reference",
                self.max_depth
            ));
        }

        // Find all sub-graph instances (both old "subgraph:" and new "macro:" prefixes)
        let subgraph_instances: Vec<(String, String)> = graph
            .nodes
            .iter()
            .filter_map(|(id, node)| {
                // Support both old "subgraph:" and new "macro:" prefixes
                if node.node_type.starts_with("subgraph:") {
                    let subgraph_id = node.node_type.strip_prefix("subgraph:").unwrap();
                    Some((id.clone(), subgraph_id.to_string()))
                } else if node.node_type.starts_with("macro:") {
                    let macro_id = node.node_type.strip_prefix("macro:").unwrap();
                    Some((id.clone(), macro_id.to_string()))
                } else {
                    None
                }
            })
            .collect();

        if subgraph_instances.is_empty() {
            // No more sub-graphs to expand
            return Ok(());
        }

        tracing::info!(
            "[COMPILER] Expanding {} sub-graph instances at depth {}",
            subgraph_instances.len(),
            depth
        );

        // Expand each sub-graph instance
        for (instance_id, subgraph_id) in subgraph_instances {
            self.expand_single_instance(graph, &instance_id, &subgraph_id)?;
        }

        // Recursively expand any nested sub-graphs
        self.expand_recursive(graph, depth + 1)
    }

    /// Expand a single sub-graph instance
    fn expand_single_instance(
        &self,
        graph: &mut GraphDescription,
        instance_id: &str,
        subgraph_id: &str,
    ) -> Result<(), String> {
        // Get the sub-graph definition
        let definition = self
            .library_manager
            .get_subgraph(subgraph_id)
            .ok_or_else(|| format!("Sub-graph '{}' not found", subgraph_id))?
            .clone();

        // Get the instance node (we need its connections)
        let instance = graph
            .nodes
            .get(instance_id)
            .ok_or_else(|| format!("Instance node '{}' not found", instance_id))?
            .clone();

        tracing::info!(
            "[COMPILER] Expanding sub-graph '{}' instance '{}'",
            subgraph_id, instance_id
        );

        // Expand the sub-graph
        self.expand_instance(graph, &instance, &definition)?;

        Ok(())
    }

    /// Expand a sub-graph instance into the parent graph
    fn expand_instance(
        &self,
        parent_graph: &mut GraphDescription,
        instance: &NodeInstance,
        definition: &SubGraphDefinition,
    ) -> Result<(), String> {
        let instance_id = &instance.id;

        // Step 1: Clone all internal nodes with prefixed IDs
        let prefix = format!("{}_", instance_id);
        let mut id_mapping: HashMap<String, String> = HashMap::new();

        for (old_id, node) in &definition.graph.nodes {
            let new_id = format!("{}{}", prefix, old_id);
            id_mapping.insert(old_id.clone(), new_id.clone());

            let mut new_node = node.clone();
            new_node.id = new_id.clone();

            // Offset position relative to instance position
            new_node.position.x += instance.position.x;
            new_node.position.y += instance.position.y;

            parent_graph.add_node(new_node);
        }

        // Step 2: Clone all internal connections with updated node IDs
        for conn in &definition.graph.connections {
            let new_source_node = id_mapping
                .get(&conn.source_node)
                .ok_or_else(|| format!("Source node '{}' not found in mapping", conn.source_node))?;

            let new_target_node = id_mapping
                .get(&conn.target_node)
                .ok_or_else(|| format!("Target node '{}' not found in mapping", conn.target_node))?;

            let new_conn = Connection::new(
                &format!("{}{}", prefix, conn.id),
                new_source_node,
                &conn.source_pin,
                new_target_node,
                &conn.target_pin,
                conn.connection_type.clone(),
            );

            parent_graph.add_connection(new_conn);
        }

        // Step 3: Map external connections to/from the instance node
        // to the internal macro_entry and macro_exit nodes (or legacy subgraph_input/output)

        // Try to find macro_entry first, fallback to subgraph_input for backward compatibility
        let input_node_id = if id_mapping.contains_key("macro_entry") {
            format!("{}macro_entry", prefix)
        } else {
            format!("{}subgraph_input", prefix)
        };
        
        // Try to find macro_exit first, fallback to subgraph_output for backward compatibility
        let output_node_id = if id_mapping.contains_key("macro_exit") {
            format!("{}macro_exit", prefix)
        } else {
            format!("{}subgraph_output", prefix)
        };

        // Find connections to the instance's input pins -> connect to internal input node outputs
        let incoming_connections: Vec<Connection> = parent_graph
            .connections
            .iter()
            .filter(|c| c.target_node == *instance_id)
            .cloned()
            .collect();

        for conn in incoming_connections {
            // Find the matching output pin on the internal input node
            let target_pin = &conn.target_pin;

            let new_conn = Connection::new(
                &format!("{}_to_input", conn.id),
                &conn.source_node,
                &conn.source_pin,
                &input_node_id,
                target_pin, // Same pin ID
                conn.connection_type.clone(),
            );

            parent_graph.add_connection(new_conn);
        }

        // Find connections from the instance's output pins -> connect from internal output node inputs
        let outgoing_connections: Vec<Connection> = parent_graph
            .connections
            .iter()
            .filter(|c| c.source_node == *instance_id)
            .cloned()
            .collect();

        for conn in outgoing_connections {
            // Find the matching input pin on the internal output node
            let source_pin = &conn.source_pin;

            let new_conn = Connection::new(
                &format!("{}_from_output", conn.id),
                &output_node_id,
                source_pin, // Same pin ID
                &conn.target_node,
                &conn.target_pin,
                conn.connection_type.clone(),
            );

            parent_graph.add_connection(new_conn);
        }

        // Step 4: Remove old connections to/from the instance
        parent_graph.connections.retain(|c| {
            c.source_node != *instance_id && c.target_node != *instance_id
        });

        // Step 5: Remove the instance node itself
        parent_graph.remove_node(instance_id);

        tracing::info!(
            "[COMPILER] Expanded sub-graph '{}' with {} nodes",
            definition.name,
            definition.graph.nodes.len()
        );

        Ok(())
    }

    /// Validate a sub-graph definition for circular references
    pub fn validate_no_circular_refs(
        &self,
        definition: &SubGraphDefinition,
    ) -> Result<(), String> {
        let mut visited = HashSet::new();
        self.check_circular_refs(definition, &mut visited)
    }

    /// Recursively check for circular references
    fn check_circular_refs(
        &self,
        definition: &SubGraphDefinition,
        visited: &mut HashSet<String>,
    ) -> Result<(), String> {
        if visited.contains(&definition.id) {
            return Err(format!(
                "Circular reference detected: sub-graph '{}' references itself",
                definition.id
            ));
        }

        visited.insert(definition.id.clone());

        // Check all nodes in the sub-graph
        for node in definition.graph.nodes.values() {
            if node.node_type.starts_with("subgraph:") {
                let subgraph_id = node.node_type.strip_prefix("subgraph:").unwrap();

                if let Some(nested_definition) = self.library_manager.get_subgraph(subgraph_id) {
                    self.check_circular_refs(nested_definition, visited)?;
                } else {
                    return Err(format!(
                        "Sub-graph '{}' references unknown sub-graph '{}'",
                        definition.id, subgraph_id
                    ));
                }
            }
        }

        visited.remove(&definition.id);
        Ok(())
    }
}