//! # Blueprint Node Registry
//!
//! Automatic registration system for blueprint nodes using compile-time collection.
//!
//! The `#[blueprint]` macro automatically adds nodes to the registry, which can then
//! be queried by the engine to build the node library.

use linkme::distributed_slice;
use crate::{NodeTypes};

/// Parameter metadata
#[derive(Debug, Clone)]
pub struct NodeParameter {
    pub name: &'static str,
    pub ty: &'static str,
}

/// Import statement metadata for a blueprint node
#[derive(Debug, Clone)]
pub struct NodeImport {
    /// The crate/module being imported (e.g., "reqwest", "std::collections")
    pub crate_name: &'static str,

    /// The specific items being imported (e.g., ["Client", "Error"])
    /// Empty slice means import the whole crate/module
    pub items: &'static [&'static str],
}

/// Complete metadata about a blueprint node
#[derive(Debug, Clone)]
pub struct NodeMetadata {
    /// Name of the function
    pub name: &'static str,

    /// Type of node (pure, fn, control_flow, event)
    pub node_type: NodeTypes,

    /// Function parameters (inputs)
    pub params: &'static [NodeParameter],

    /// Return type (if any)
    pub return_type: Option<&'static str>,

    /// Execution input pins
    pub exec_inputs: &'static [&'static str],

    /// Execution output pins
    pub exec_outputs: &'static [&'static str],

    /// The complete function source code
    pub function_source: &'static str,

    /// Documentation from doc comments (///)
    pub documentation: &'static [&'static str],

    /// Category for grouping nodes
    pub category: &'static str,

    /// Optional hex color for the node
    pub color: Option<&'static str>,

    /// External imports required by this node
    pub imports: &'static [NodeImport],
}

/// Global registry of all blueprint nodes
///
/// This is automatically populated by the #[blueprint] macro using linkme's
/// distributed slice feature. All nodes decorated with #[blueprint] are
/// collected here at link time.
#[distributed_slice]
pub static BLUEPRINT_REGISTRY: [NodeMetadata] = [..];

/// Get all registered blueprint nodes
///
/// This function returns a slice of all nodes that have been registered
/// via the #[blueprint] macro across all modules.
///
/// # Example
///
/// ```ignore
/// use pulsar_std::get_all_nodes;
///
/// let nodes = get_all_nodes();
/// for node in nodes {
///     println!("Node: {} (category: {})", node.name, node.category);
/// }
/// ```
pub fn get_all_nodes() -> &'static [NodeMetadata] {
    &BLUEPRINT_REGISTRY
}

/// Get nodes filtered by category
pub fn get_nodes_by_category(category: &str) -> Vec<&'static NodeMetadata> {
    BLUEPRINT_REGISTRY
        .iter()
        .filter(|node| node.category == category)
        .collect()
}

/// Type constructor metadata for the type system
#[derive(Debug, Clone)]
pub struct TypeConstructorMetadata {
    /// Name of the constructor (e.g., "Box", "Arc", "Result")
    pub name: &'static str,

    /// Number of type parameters
    pub params_count: usize,

    /// Category for grouping
    pub category: &'static str,

    /// Description of what this type does
    pub description: &'static str,

    /// Example usage
    pub example: &'static str,
}

/// Global registry of all type constructors
///
/// This is automatically populated by the #[blueprint_type] macro using linkme's
/// distributed slice feature.
#[distributed_slice]
pub static TYPE_CONSTRUCTOR_REGISTRY: [TypeConstructorMetadata] = [..];

/// Get all registered type constructors
pub fn get_all_type_constructors() -> &'static [TypeConstructorMetadata] {
    &TYPE_CONSTRUCTOR_REGISTRY
}

/// Get type constructors filtered by category
pub fn get_type_constructors_by_category(category: &str) -> Vec<&'static TypeConstructorMetadata> {
    TYPE_CONSTRUCTOR_REGISTRY
        .iter()
        .filter(|tc| tc.category == category)
        .collect()
}

/// Get a specific node by name
pub fn get_node_by_name(name: &str) -> Option<&'static NodeMetadata> {
    BLUEPRINT_REGISTRY.iter().find(|node| node.name == name)
}

/// Get all unique categories
pub fn get_all_categories() -> Vec<&'static str> {
    let mut categories: Vec<_> = BLUEPRINT_REGISTRY
        .iter()
        .map(|node| node.category)
        .collect();
    categories.sort_unstable();
    categories.dedup();
    categories
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_not_empty() {
        let nodes = get_all_nodes();
        assert!(!nodes.is_empty(), "Blueprint registry should contain nodes");
    }

    #[test]
    fn test_get_categories() {
        let categories = get_all_categories();
        assert!(!categories.is_empty(), "Should have at least one category");
    }
}
