//! # Import Collection
//!
//! Collects all required `use` statements from nodes in the graph.
//!
//! Each node type may require specific imports. This module analyzes
//! the graph and generates the minimal set of imports needed.
//!
//! ## Example
//!
//! If the graph uses:
//! - `thread_spawn` (requires `std::thread`)
//! - `fs_read` (requires `std::fs`)
//! - Math nodes (requires `pulsar_std::*`)
//!
//! Generates:
//! ```rust,ignore
//! use pulsar_std::*;
//! use std::thread;
//! use std::fs;
//! ```

use crate::compiler::core::NodeMetadata;
use crate::graph::GraphDescription;
use std::collections::{HashMap, HashSet};

/// Extract unique crate dependencies needed by nodes in the graph
/// Returns a set of crate names that should be added to Cargo.toml
pub fn collect_node_dependencies(
    graph: &GraphDescription,
    metadata: &HashMap<String, NodeMetadata>,
) -> HashSet<String> {
    let mut dependencies = HashSet::new();

    // Iterate through all nodes in the graph
    for node in graph.nodes.values() {
        // Get metadata for this node type
        if let Some(node_meta) = metadata.get(&node.node_type) {
            // Extract crate name from each import
            for import in node_meta.imports {
                // Get the base crate name (before any ::)
                let crate_name = import.crate_name.split("::").next().unwrap_or(import.crate_name);

                // Skip standard library crates
                if !crate_name.starts_with("std") && !crate_name.starts_with("core") {
                    dependencies.insert(crate_name.to_string());
                }
            }
        }
    }

    dependencies
}

/// Collect all unique imports needed by nodes in the graph
pub fn collect_node_imports(
    graph: &GraphDescription,
    metadata: &HashMap<String, NodeMetadata>,
) -> Vec<String> {
    let mut import_stmts = HashSet::new();

    // Iterate through all nodes in the graph
    for node in graph.nodes.values() {
        // Get metadata for this node type
        if let Some(node_meta) = metadata.get(&node.node_type) {
            // Process each import
            for import in node_meta.imports {
                let import_stmt = if import.items.is_empty() {
                    // Import entire crate: use crate_name;
                    format!("use {};", import.crate_name)
                } else if import.items.len() == 1 {
                    // Single item: use crate_name::item;
                    format!("use {}::{};", import.crate_name, import.items[0])
                } else {
                    // Multiple items: use crate_name::{item1, item2};
                    let items = import.items.join(", ");
                    format!("use {}::{{{}}};", import.crate_name, items)
                };
                import_stmts.insert(import_stmt);
            }
        }
    }

    // Convert to sorted vector for deterministic output
    let mut imports: Vec<_> = import_stmts.into_iter().collect();
    imports.sort();
    imports
}
