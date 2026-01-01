//! # Data Flow Resolver
//!
//! Resolves data dependencies between nodes to determine:
//! 1. Where each input value comes from (connection, constant, or default)
//! 2. The order in which pure nodes should be evaluated
//! 3. Variable names for intermediate results
//!
//! This module builds a data dependency graph and performs topological sorting
//! to ensure pure nodes are evaluated in the correct order before being used.

use std::collections::{HashMap,HashSet,VecDeque};
use crate::graph::{GraphDescription,ConnectionType};
use crate::compiler::core::{NodeMetadata, NodeTypes};

/// Resolves data flow for a graph
pub struct DataResolver {
    /// Maps (node_id, input_pin) -> DataSource
    input_sources: HashMap<(String, String), DataSource>,

    /// Maps node_id -> unique variable name for its result
    result_variables: HashMap<String, String>,

    /// Topologically sorted list of pure nodes that need evaluation
    pure_evaluation_order: Vec<String>,

    /// Class variables (name -> type)
    variables: HashMap<String, String>,
}

/// Where an input value comes from
#[derive(Debug, Clone)]
pub enum DataSource {
    /// Connected to another node's output
    Connection {
        source_node_id: String,
        source_pin: String,
    },

    /// Constant value from node properties
    Constant(String),

    /// Default value (use parameter default)
    Default,
}

impl DataResolver {
    /// Build a data resolver from a graph
    pub fn build(graph: &GraphDescription, metadata: &HashMap<String, NodeMetadata>) -> Result<Self, String> {
        Self::build_with_variables(graph, metadata, HashMap::new())
    }

    /// Build a data resolver from a graph with variable definitions
    pub fn build_with_variables(
        graph: &GraphDescription,
        metadata: &HashMap<String, NodeMetadata>,
        variables: HashMap<String, String>,
    ) -> Result<Self, String> {
        let mut resolver = DataResolver {
            input_sources: HashMap::new(),
            result_variables: HashMap::new(),
            pure_evaluation_order: Vec::new(),
            variables,
        };

        // Phase 1: Map all data connections
        resolver.map_data_connections(graph)?;

        // Phase 2: Generate variable names for node results
        resolver.generate_variable_names(graph);

        // Phase 3: Determine evaluation order for pure nodes
        resolver.compute_pure_evaluation_order(graph, metadata)?;

        Ok(resolver)
    }

    /// Map all data connections in the graph
    fn map_data_connections(&mut self, graph: &GraphDescription) -> Result<(), String> {
        for connection in &graph.connections {
            if matches!(connection.connection_type, ConnectionType::Data) {
                let key = (connection.target_node.clone(), connection.target_pin.clone());
                let source = DataSource::Connection {
                    source_node_id: connection.source_node.clone(),
                    source_pin: connection.source_pin.clone(),
                };

                self.input_sources.insert(key, source);
            }
        }

        // For inputs not connected, check properties or use defaults
        for (node_id, node) in &graph.nodes {
            for pin_instance in &node.inputs {
                let pin_name = &pin_instance.id;
                let key = (node_id.clone(), pin_name.clone());

                if !self.input_sources.contains_key(&key) {
                    // Check if there's a property value
                    if let Some(prop_value) = node.properties.get(pin_name) {
                        self.input_sources.insert(
                            key,
                            DataSource::Constant(property_value_to_string(prop_value)),
                        );
                    } else {
                        self.input_sources.insert(key, DataSource::Default);
                    }
                }
            }
        }

        Ok(())
    }

    /// Generate unique variable names for each node's result
    fn generate_variable_names(&mut self, graph: &GraphDescription) {
        for (node_id, node) in &graph.nodes {
            // Generate a safe variable name from node ID
            let var_name = format!("node_{}_result", sanitize_var_name(node_id));
            self.result_variables.insert(node_id.clone(), var_name);
        }
    }

    /// Compute evaluation order for pure nodes using topological sort
    fn compute_pure_evaluation_order(&mut self, graph: &GraphDescription, metadata: &HashMap<String, NodeMetadata>) -> Result<(), String> {
        // Build dependency graph for pure nodes
        let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
        let mut pure_nodes: HashSet<String> = HashSet::new();

        // Identify pure nodes using metadata
        for (node_id, node) in &graph.nodes {
            // Check if this node is pure according to its metadata
            if let Some(node_meta) = metadata.get(&node.node_type) {
                // A node is pure if metadata says so AND it has a return type
                if node_meta.node_type == NodeTypes::pure && node_meta.return_type.is_some() {
                    pure_nodes.insert(node_id.clone());
                    dependencies.insert(node_id.clone(), Vec::new());
                }
            }
        }

        // Build dependency edges
        for connection in &graph.connections {
            if matches!(connection.connection_type, ConnectionType::Data) {
                if pure_nodes.contains(&connection.target_node)
                    && pure_nodes.contains(&connection.source_node)
                {
                    dependencies
                        .entry(connection.target_node.clone())
                        .or_insert_with(Vec::new)
                        .push(connection.source_node.clone());
                }
            }
        }

        // Build reverse dependency map: dependents[X] = [nodes that depend on X]
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for (target, sources) in &dependencies {
            for source in sources {
                dependents
                    .entry(source.clone())
                    .or_insert_with(Vec::new)
                    .push(target.clone());
            }
        }

        // Topological sort using Kahn's algorithm
        // in_degree[node] = number of dependencies this node has
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for node_id in &pure_nodes {
            let num_deps = dependencies.get(node_id).map(|v| v.len()).unwrap_or(0);
            in_degree.insert(node_id.clone(), num_deps);
        }

        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(id, _)| id.clone())
            .collect();

        while let Some(node_id) = queue.pop_front() {
            self.pure_evaluation_order.push(node_id.clone());

            // Find all nodes that depend on this node and decrement their in_degree
            if let Some(dependent_nodes) = dependents.get(&node_id) {
                for dependent in dependent_nodes {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
        }

        // Check for cycles
        if self.pure_evaluation_order.len() != pure_nodes.len() {
            return Err("Cyclic dependency detected in pure node graph".to_string());
        }

        Ok(())
    }

    /// Get the source of data for a specific node input
    pub fn get_input_source(&self, node_id: &str, pin_name: &str) -> Option<&DataSource> {
        self.input_sources
            .get(&(node_id.to_string(), pin_name.to_string()))
    }

    /// Get the variable name for a node's result
    pub fn get_result_variable(&self, node_id: &str) -> Option<&String> {
        self.result_variables.get(node_id)
    }

    /// Get the evaluation order for pure nodes
    pub fn get_pure_evaluation_order(&self) -> &[String] {
        &self.pure_evaluation_order
    }

    /// Generate Rust code expression for an input value (inlines pure nodes recursively)
    pub fn generate_input_expression(
        &self,
        node_id: &str,
        pin_name: &str,
        graph: &GraphDescription,
    ) -> Result<String, String> {
        match self.get_input_source(node_id, pin_name) {
            Some(DataSource::Connection {
                source_node_id,
                source_pin,
            }) => {
                // Get the source node
                let source_node = graph.nodes.get(source_node_id)
                    .ok_or_else(|| format!("Source node not found: {}", source_node_id))?;

                // Check if source is a pure node - if so, inline it recursively
                use crate::compiler::get_node_metadata;
                let metadata = get_node_metadata();

                tracing::info!("[DATA_RESOLVER] Checking node '{}' (type: {})", source_node_id, source_node.node_type);

                // Special case: variable getter nodes
                if source_node.node_type.starts_with("get_") {
                    let var_name = source_node.node_type.strip_prefix("get_").unwrap();
                    let var_type = self.variables.get(var_name)
                        .ok_or_else(|| format!("Variable '{}' not found in variable definitions", var_name))?;

                    // Generate getter code based on type (Cell vs RefCell)
                    let is_copy_type = Self::is_copy_type(var_type);
                    if is_copy_type {
                        // Cell: VAR_NAME.with(|v| v.get())
                        return Ok(format!("{}.with(|v| v.get())", var_name.to_uppercase()));
                    } else {
                        // RefCell: VAR_NAME.with(|v| v.borrow().clone())
                        return Ok(format!("{}.with(|v| v.borrow().clone())", var_name.to_uppercase()));
                    }
                }

                // Special case: reroute nodes are transparent passthroughs
                if source_node.node_type == "reroute" {
                    tracing::info!("[DATA_RESOLVER] Reroute node detected - passing through to its input");
                    // Reroutes can have different pin names, find the actual input pin
                    let input_pin = source_node.inputs.get(0)
                        .map(|p| &p.id)
                        .ok_or_else(|| format!("Reroute node {} has no input pins", source_node_id))?;
                    tracing::info!("[DATA_RESOLVER] Reroute node input pin name: '{}'", input_pin);
                    // Recursively resolve what's connected to the reroute's input
                    return self.generate_input_expression(source_node_id, input_pin, graph);
                }

                let is_pure = if let Some(node_meta) = metadata.get(&source_node.node_type) {
                    tracing::info!("[DATA_RESOLVER] Found metadata for '{}': type={:?}", source_node.node_type, node_meta.node_type);
                    node_meta.node_type == NodeTypes::pure
                } else {
                    tracing::warn!("[DATA_RESOLVER] NO METADATA FOUND for node type '{}'", source_node.node_type);
                    false
                };

                tracing::info!("[DATA_RESOLVER] is_pure = {}", is_pure);

                if is_pure {
                    tracing::info!("[DATA_RESOLVER] Inlining pure node '{}'", source_node.node_type);
                    self.generate_pure_node_expression(source_node, graph)
                } else {
                    // Non-pure nodes (function nodes with return values) use variables
                    if let Some(var_name) = self.get_result_variable(source_node_id) {
                        if source_node.outputs.len() > 1 {
                            // Multiple outputs: access tuple element or field
                            let output_names: Vec<_> = source_node.outputs.iter().map(|p| &p.id).collect();
                            if let Some(index) = output_names.iter().position(|name| *name == source_pin) {
                                Ok(format!("{}.{}", var_name, index))
                            } else {
                                Ok(var_name.clone())
                            }
                        } else {
                            // Single output: use variable directly
                            Ok(var_name.clone())
                        }
                    } else {
                        Err(format!("No variable found for source node: {}", source_node_id))
                    }
                }
            }

            Some(DataSource::Constant(value)) => Ok(value.clone()),

            Some(DataSource::Default) => {
                // Use Rust default value based on type
                if let Some(node) = graph.nodes.get(node_id) {
                    if let Some(pin) = node.inputs.iter().find(|p| p.id == pin_name) {
                        Ok(get_default_value_for_type(&pin.pin.data_type))
                    } else {
                        Err(format!("Pin not found: {}.{}", node_id, pin_name))
                    }
                } else {
                    Err(format!("Node not found: {}", node_id))
                }
            }

            None => Err(format!("No data source for input: {}.{}", node_id, pin_name)),
        }
    }

    /// Generate inlined expression for a pure node (recursive)
    fn generate_pure_node_expression(
        &self,
        node: &crate::graph::NodeInstance,
        graph: &GraphDescription,
    ) -> Result<String, String> {
        // Get node metadata to find function name and parameters
        use crate::compiler::get_node_metadata;
        let metadata = get_node_metadata();

        let node_meta = metadata.get(&node.node_type)
            .ok_or_else(|| format!("Unknown node type: {}", node.node_type))?;

        // Recursively generate arguments
        let mut args = Vec::new();
        for param in node_meta.params.iter() {
            let arg_expr = self.generate_input_expression(&node.id, &param.name, graph)?;
            args.push(arg_expr);
        }

        // Return inlined function call
        Ok(format!("{}({})", node_meta.name, args.join(", ")))
    }

    /// Check if a type is Copy (uses Cell) or not (uses RefCell)
    fn is_copy_type(type_str: &str) -> bool {
        matches!(type_str, "i32" | "i64" | "u32" | "u64" | "f32" | "f64" | "bool" | "char" | "usize" | "isize" | "i8" | "i16" | "u8" | "u16")
    }
}

/// Convert a property value to a Rust code string
fn property_value_to_string(value: &crate::graph::PropertyValue) -> String {
    match value {
        crate::graph::PropertyValue::String(s) => format!("\"{}\"", s.escape_default()),
        crate::graph::PropertyValue::Number(n) => n.to_string(),
        crate::graph::PropertyValue::Boolean(b) => b.to_string(),
        crate::graph::PropertyValue::Vector2(x, y) => format!("({}, {})", x, y),
        crate::graph::PropertyValue::Vector3(x, y, z) => format!("({}, {}, {})", x, y, z),
        crate::graph::PropertyValue::Color(r, g, b, a) => format!("({}, {}, {}, {})", r, g, b, a),
    }
}

/// Get default value for a data type
fn get_default_value_for_type(data_type: &crate::graph::DataType) -> String {
    match data_type {
        crate::graph::DataType::Execution => "()".to_string(),
        crate::graph::DataType::Typed(type_info) => {
            let type_str = type_info.to_string();
            match type_str.as_str() {
                // Floats don't implement Default in Rust, so we need explicit defaults
                "f32" | "f64" => "0.0".to_string(),
                // Tuples with floats also need explicit defaults
                _ if type_str.contains("f32") || type_str.contains("f64") => {
                    // For tuple types like (f32, f32), we can't use Default::default()
                    // Parse and construct appropriate default
                    if type_str.starts_with('(') && type_str.ends_with(')') {
                        let inner = &type_str[1..type_str.len()-1];
                        let count = inner.split(',').count();
                        let zeros = vec!["0.0"; count].join(", ");
                        format!("({})", zeros)
                    } else {
                        "Default::default()".to_string()
                    }
                }
                // Everything else implements Default - use it!
                _ => "Default::default()".to_string(),
            }
        }
        // Legacy types - being explicit for clarity
        crate::graph::DataType::Number => "0.0".to_string(), // f64 doesn't impl Default
        crate::graph::DataType::Vector2 => "(0.0, 0.0)".to_string(), // Tuple with floats
        crate::graph::DataType::Vector3 => "(0.0, 0.0, 0.0)".to_string(), // Tuple with floats
        crate::graph::DataType::Color => "(0.0, 0.0, 0.0, 1.0)".to_string(), // Tuple with floats
        _ => "Default::default()".to_string(),
    }
}

/// Sanitize a string to be a valid Rust variable name
fn sanitize_var_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Connection, DataType, GraphMetadata, NodeInstance, Pin, PinType, Position, PropertyValue};

    fn create_test_graph() -> GraphDescription {
        let mut graph = GraphDescription {
            nodes: HashMap::new(),
            connections: Vec::new(),
            comments: Vec::new(),
            metadata: GraphMetadata {
                name: "test".to_string(),
                description: String::new(),
                version: "1.0.0".to_string(),
                created_at: String::new(),
                modified_at: String::new(),
            },
        };

        // Add node: add(a, b) -> result
        let mut add_node = NodeInstance::new("add_1", "add", Position { x: 0.0, y: 0.0 });
        add_node.add_input_pin("a", DataType::Number);
        add_node.add_input_pin("b", DataType::Number);
        add_node.add_output_pin("result", DataType::Number);
        add_node.set_property("a", PropertyValue::Number(5.0));
        add_node.set_property("b", PropertyValue::Number(3.0));
        graph.add_node(add_node);

        graph
    }
    
    #[test]
    fn test_data_resolver_with_constants() {
        let graph = create_test_graph();
        let metadata = HashMap::new(); // Empty metadata for this test
        let resolver = DataResolver::build(&graph, &metadata).unwrap();

        let a_source = resolver.get_input_source("add_1", "a").unwrap();
        let b_source = resolver.get_input_source("add_1", "b").unwrap();

        assert!(matches!(a_source, DataSource::Constant(_)));
        assert!(matches!(b_source, DataSource::Constant(_)));

        let a_expr = resolver.generate_input_expression("add_1", "a", &graph).unwrap();
        let b_expr = resolver.generate_input_expression("add_1", "b", &graph).unwrap();

        assert_eq!(a_expr, "5");
        assert_eq!(b_expr, "3");
    }

    #[test]
    fn test_variable_name_generation() {
        let graph = create_test_graph();
        let metadata = HashMap::new(); // Empty metadata for this test
        let resolver = DataResolver::build(&graph, &metadata).unwrap();

        let var_name = resolver.get_result_variable("add_1").unwrap();
        assert_eq!(var_name, "node_add_1_result");
    }
}
