//! # Function Node Code Generation
//!
//! Strategy for generating code from function nodes (side effects with linear exec flow).
//!
//! Function nodes have:
//! - Side effects (I/O, state changes, etc.)
//! - One exec input, one exec output
//! - Linear execution flow
//!
//! ## Generation Strategy
//!
//! Function nodes are generated as **sequential statements** in the execution chain.
//!
//! ## Example
//!
//! Graph: `begin_play -> print("A") -> print("B") -> print("C")`
//!
//! Generated:
//! ```rust,ignore
//! pub fn main() {
//!     print_string("A".to_string());
//!     print_string("B".to_string());
//!     print_string("C".to_string());
//! }
//! ```

use crate::compiler::core::NodeMetadata;
use crate::graph::NodeInstance;
use super::code_generator::CodeGenerator;

impl<'a> CodeGenerator<'a> {
    /// Generate code for a function node
    pub fn generate_function_node(
        &mut self,
        node: &NodeInstance,
        node_meta: &NodeMetadata,
        output: &mut String,
        indent_level: usize,
    ) -> Result<(), String> {
        let indent = "    ".repeat(indent_level);

        // Collect arguments
        let args = self.collect_arguments(node, node_meta)?;

        // Check if this function returns a value
        let has_return = node_meta.return_type.is_some();

        if has_return {
            // Store result in variable
            let result_var = self
                .data_resolver
                .get_result_variable(&node.id)
                .ok_or_else(|| format!("No result variable for node: {}", node.id))?;

            output.push_str(&format!(
                "{}let {} = {}({});\n",
                indent,
                result_var,
                node_meta.name,
                args.join(", ")
            ));
        } else {
            // Just call the function
            output.push_str(&format!(
                "{}{}({});\n",
                indent,
                node_meta.name,
                args.join(", ")
            ));
        }

        // Follow execution chain
        if let Some(exec_out) = node_meta.exec_outputs.first() {
            let connected = self.exec_routing.get_connected_nodes(&node.id, exec_out);
            for next_node_id in connected {
                if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                    self.generate_exec_chain(next_node, output, indent_level)?;
                }
            }
        }

        Ok(())
    }
}
