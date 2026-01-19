//! # Code Generator
//!
//! The core code generation logic for transforming node graphs into Rust code.
//!
//! This module implements different generation strategies for each node type:
//! - **Pure nodes**: Recursively inlined as expressions where used (no allocations)
//! - **Function nodes**: Generate function calls with exec chain
//! - **Control flow nodes**: Inline function body with substitutions

use crate::compiler::utils::ast_transform as ast_utils;
use crate::compiler::analysis::{DataResolver, ExecutionRouting};
use crate::compiler::core::{NodeMetadata, NodeTypes};
use crate::graph::{GraphDescription, NodeInstance};
use std::collections::{HashMap, HashSet};

// Import functions from modularized modules
use super::imports::collect_node_imports;

/// Variable metadata for code generation
#[derive(Debug, Clone)]
pub struct VariableInfo {
    pub name: String,
    pub var_type: String,
}

/// Main code generator
pub struct CodeGenerator<'a> {
    /// Node metadata from pulsar_std
    pub(super) metadata: &'a HashMap<String, NodeMetadata>,

    /// Data flow resolver
    pub(super) data_resolver: &'a DataResolver,

    /// Execution routing table
    pub(super) exec_routing: &'a ExecutionRouting,

    /// The graph being compiled
    pub(super) graph: &'a GraphDescription,

    /// Class variables (name -> type)
    pub(super) variables: HashMap<String, String>,

    /// Tracks visited nodes to prevent infinite loops
    pub(super) visited: HashSet<String>,
}

impl<'a> CodeGenerator<'a> {
    pub fn new(
        metadata: &'a HashMap<String, NodeMetadata>,
        data_resolver: &'a DataResolver,
        exec_routing: &'a ExecutionRouting,
        graph: &'a GraphDescription,
        variables: HashMap<String, String>,
    ) -> Self {
        Self {
            metadata,
            data_resolver,
            exec_routing,
            graph,
            variables,
            visited: HashSet::new(),
        }
    }

    /// Generate execution chain starting from a node
    pub(super) fn generate_exec_chain(
        &mut self,
        node: &NodeInstance,
        output: &mut String,
        indent_level: usize,
    ) -> Result<(), String> {
        // Prevent infinite loops
        if self.visited.contains(&node.id) {
            return Ok(());
        }
        self.visited.insert(node.id.clone());

        // Check if this is a variable getter or setter node
        if node.node_type.starts_with("get_") {
            // Getter nodes are pure (no exec chain), skip
            return Ok(());
        } else if node.node_type.starts_with("set_") {
            // Setter nodes have exec chain
            return self.generate_setter_node(node, output, indent_level);
        }

        let node_meta = self
            .metadata
            .get(&node.node_type)
            .ok_or_else(|| format!("Unknown node type: {}", node.node_type))?;

        match node_meta.node_type {
            NodeTypes::pure => {
                // Pure nodes are pre-evaluated, skip in exec chain
                Ok(())
            }

            NodeTypes::fn_ => {
                self.generate_function_node(node, node_meta, output, indent_level)
            }

            NodeTypes::control_flow => {
                self.generate_control_flow_node(node, node_meta, output, indent_level)
            }

            NodeTypes::event => {
                // Event nodes define the outer function, skip in exec chain
                // Their "Body" output defines where execution starts
                Ok(())
            }
        }
    }

    /// Collect arguments for a function call
    pub(super) fn collect_arguments(
        &self,
        node: &NodeInstance,
        node_meta: &NodeMetadata,
    ) -> Result<Vec<String>, String> {
        let mut args = Vec::new();

        for param in node_meta.params.iter() {
            let value =
                self.data_resolver
                    .generate_input_expression(&node.id, &param.name, self.graph)?;
            args.push(value);
        }

        Ok(args)
    }

    /// Generate code for a variable setter node
    fn generate_setter_node(
        &mut self,
        node: &NodeInstance,
        output: &mut String,
        indent_level: usize,
    ) -> Result<(), String> {
        let indent = "    ".repeat(indent_level);

        // Extract variable name from node type (remove "set_" prefix)
        let var_name = node
            .node_type
            .strip_prefix("set_")
            .ok_or_else(|| format!("Invalid setter node type: {}", node.node_type))?;

        // Get the value to set from the "value" input pin
        let value_expr = self
            .data_resolver
            .generate_input_expression(&node.id, "value", self.graph)?;

        // Get variable type to determine Cell vs RefCell
        let var_type = self
            .variables
            .get(var_name)
            .ok_or_else(|| format!("Variable '{}' not found in variable definitions", var_name))?;

        // Determine if this is a Copy type (uses Cell) or not (uses RefCell)
        let is_copy_type = Self::is_copy_type(var_type);

        if is_copy_type {
            // Cell: VAR_NAME.with(|v| v.set(value));
            output.push_str(&format!(
                "{}{}.with(|v| v.set({}));\n",
                indent,
                var_name.to_uppercase(),
                value_expr
            ));
        } else {
            // RefCell: VAR_NAME.with(|v| *v.borrow_mut() = value);
            output.push_str(&format!(
                "{}{}.with(|v| *v.borrow_mut() = {});\n",
                indent,
                var_name.to_uppercase(),
                value_expr
            ));
        }

        // Follow execution chain from "exec_out" pin
        if node.outputs.iter().any(|p| p.id == "exec_out") {
            let connected = self.exec_routing.get_connected_nodes(&node.id, "exec_out");
            for next_node_id in connected {
                if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                    self.generate_exec_chain(next_node, output, indent_level)?;
                }
            }
        }

        Ok(())
    }

    /// Check if a type is Copy (uses Cell) or not (uses RefCell)
    fn is_copy_type(type_str: &str) -> bool {
        matches!(
            type_str,
            "i32"
                | "i64"
                | "u32"
                | "u64"
                | "f32"
                | "f64"
                | "bool"
                | "char"
                | "usize"
                | "isize"
                | "i8"
                | "i16"
                | "u8"
                | "u16"
        )
    }
}

/// Generate complete Rust program from graph
pub fn generate_program(
    graph: &GraphDescription,
    metadata: &HashMap<String, NodeMetadata>,
    data_resolver: &DataResolver,
    exec_routing: &ExecutionRouting,
    variables: HashMap<String, String>,
) -> Result<String, String> {
    let mut code = String::new();

    // Add imports
    code.push_str("// Auto-generated code from Pulsar Blueprint\n");
    code.push_str("// DO NOT EDIT - Changes will be overwritten\n\n");
    code.push_str("use pulsar_std::*;\n");

    // Collect and add node-specific imports
    let node_imports = collect_node_imports(graph, metadata);
    for import_stmt in node_imports {
        code.push_str(&import_stmt);
        code.push_str("\n");
    }
    code.push_str("\n");

    // Find event nodes using metadata
    let event_nodes: Vec<_> = graph
        .nodes
        .values()
        .filter(|node| {
            // Check if this node's type is an event in metadata
            metadata
                .get(&node.node_type)
                .map(|meta| meta.node_type == NodeTypes::event)
                .unwrap_or(false)
        })
        .collect();

    if event_nodes.is_empty() {
        return Err(
            "No event nodes found in graph - add a 'main' or 'begin_play' event".to_string(),
        );
    }

    // Generate each event function
    for event_node in event_nodes {
        let mut generator = CodeGenerator::new(
            metadata,
            data_resolver,
            exec_routing,
            graph,
            variables.clone(),
        );
        let event_code = generator.generate_event_function(event_node)?;
        code.push_str(&event_code);
        code.push_str("\n");
    }

    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: Add tests once we have the full compiler pipeline
}
