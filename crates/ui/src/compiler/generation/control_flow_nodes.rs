//! # Control Flow Node Code Generation
//!
//! Strategy for generating code from control flow nodes (branching execution).
//!
//! Control flow nodes have:
//! - Multiple exec outputs (e.g., True/False for branch)
//! - `exec_output!("Label")` calls in their body
//! - Must be inlined (cannot be called as functions)
//!
//! ## Generation Strategy
//!
//! Control flow nodes must be **inlined** with transformation:
//! 1. Parse the node's function body as AST
//! 2. Replace each `exec_output!("Label")` with code from connected nodes
//! 3. Substitute parameter values with actual expressions
//! 4. Inline the transformed body
//!
//! ## Example
//!
//! Graph: `branch(x > 5) -> [True: print("big"), False: print("small")]`
//!
//! Generated:
//! ```rust,ignore
//! if x > 5 {
//!     print_string("big".to_string());
//! } else {
//!     print_string("small".to_string());
//! }
//! ```

use crate::compiler::core::NodeMetadata;
use crate::compiler::utils::ast_transform as ast_utils;
use crate::graph::NodeInstance;
use super::code_generator::CodeGenerator;
use std::collections::HashMap;

impl<'a> CodeGenerator<'a> {
    /// Generate code for a control flow node (inline)
    pub fn generate_control_flow_node(
        &mut self,
        node: &NodeInstance,
        node_meta: &NodeMetadata,
        output: &mut String,
        indent_level: usize,
    ) -> Result<(), String> {
        let indent = "    ".repeat(indent_level);

        // Build exec_output replacements
        let mut exec_replacements = HashMap::new();
        tracing::error!(
            "[CODEGEN] Building exec replacements for control flow node '{}'",
            node.node_type
        );
        tracing::error!(
            "[CODEGEN] Node has {} exec outputs: {:?}",
            node_meta.exec_outputs.len(),
            node_meta.exec_outputs
        );

        for exec_pin in node_meta.exec_outputs.iter() {
            let connected = self.exec_routing.get_connected_nodes(&node.id, exec_pin);
            tracing::error!(
                "[CODEGEN] Exec pin '{}' has {} connected nodes: {:?}",
                exec_pin,
                connected.len(),
                connected
            );

            let mut exec_code = String::new();
            let mut local_visited = self.visited.clone();

            for next_node_id in connected {
                if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                    tracing::error!(
                        "[CODEGEN] Generating code for connected node '{}'",
                        next_node.node_type
                    );
                    // Create a sub-generator with local visited set
                    let mut sub_gen = CodeGenerator {
                        metadata: self.metadata,
                        data_resolver: self.data_resolver,
                        exec_routing: self.exec_routing,
                        graph: self.graph,
                        variables: self.variables.clone(),
                        visited: local_visited.clone(),
                    };

                    sub_gen.generate_exec_chain(next_node, &mut exec_code, 0)?;
                    local_visited = sub_gen.visited;
                }
            }

            tracing::error!(
                "[CODEGEN] Exec pin '{}' replacement code: '{}'",
                exec_pin,
                exec_code.trim()
            );
            exec_replacements.insert(exec_pin.to_string(), exec_code.trim().to_string());
        }

        tracing::error!(
            "[CODEGEN] Final exec_replacements map: {:?}",
            exec_replacements
        );

        // Build parameter substitutions
        let mut param_substitutions = HashMap::new();
        for param in node_meta.params.iter() {
            let value =
                self.data_resolver
                    .generate_input_expression(&node.id, &param.name, self.graph)?;
            param_substitutions.insert(param.name.to_string(), value);
        }

        // Inline the function with substitutions
        let inlined_body = ast_utils::inline_control_flow_function(
            &node_meta.function_source,
            exec_replacements,
            param_substitutions,
        )?;

        // Add inlined code with proper indentation
        for line in inlined_body.lines() {
            if !line.trim().is_empty() {
                output.push_str(&format!("{}{}\n", indent, line));
            }
        }

        Ok(())
    }
}
