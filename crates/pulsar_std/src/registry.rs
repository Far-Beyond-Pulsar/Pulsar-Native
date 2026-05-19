//! # Blueprint Node Registry
//!
//! Automatic registration system for blueprint nodes using compile-time collection.
//! On native targets, uses linkme distributed_slice for zero-cost compile-time registration.
//! When building as a cdylib, the registry is not used — __bp_dispatch_* symbols are the interface.

use crate::NodeTypes;

/// Parameter metadata — sizes baked in at compile time by the `#[blueprint]` macro.
#[derive(Debug, Clone)]
pub struct NodeParameter {
    pub name: &'static str,
    pub ty: &'static str,
    /// `std::mem::size_of::<T>()` for this parameter's type, set by the macro.
    pub size: usize,
    /// `std::mem::align_of::<T>()` for this parameter's type, set by the macro.
    pub align: usize,
}

/// Import statement metadata for a blueprint node
#[derive(Debug, Clone)]
pub struct NodeImport {
    pub crate_name: &'static str,
    pub items: &'static [&'static str],
}

/// Complete metadata about a blueprint node
#[derive(Debug, Clone)]
pub struct NodeMetadata {
    pub name: &'static str,
    pub node_type: NodeTypes,
    pub params: &'static [NodeParameter],
    pub return_type: Option<&'static str>,
    /// `std::mem::size_of::<ReturnType>()`, set by the `#[blueprint]` macro. 0 for void.
    pub return_size: usize,
    /// `std::mem::align_of::<ReturnType>()`, set by the `#[blueprint]` macro. 1 for void.
    pub return_align: usize,
    pub exec_inputs: &'static [&'static str],
    pub exec_outputs: &'static [&'static str],
    pub function_source: &'static str,
    pub documentation: &'static [&'static str],
    pub category: &'static str,
    pub color: Option<&'static str>,
    pub imports: &'static [NodeImport],
}

// ── Native registry (linkme distributed_slice) ───────────────────────────────

pub mod native_registry {
    use super::NodeMetadata;

    #[cfg(feature = "native")]
    use linkme::distributed_slice;

    #[cfg(feature = "native")]
    #[distributed_slice]
    pub static BLUEPRINT_REGISTRY: [NodeMetadata] = [..];
}

#[cfg(feature = "native")]
pub use native_registry::BLUEPRINT_REGISTRY;

#[cfg(feature = "native")]
pub fn get_all_nodes() -> &'static [NodeMetadata] {
    &native_registry::BLUEPRINT_REGISTRY
}

#[cfg(not(feature = "native"))]
pub fn get_all_nodes() -> &'static [NodeMetadata] {
    &[]
}

#[cfg(feature = "native")]
pub fn get_nodes_by_category(category: &str) -> Vec<&'static NodeMetadata> {
    native_registry::BLUEPRINT_REGISTRY
        .iter()
        .filter(|n| n.category == category)
        .collect()
}

#[cfg(not(feature = "native"))]
pub fn get_nodes_by_category(_category: &str) -> Vec<&'static NodeMetadata> {
    vec![]
}

#[cfg(feature = "native")]
pub fn get_node_by_name(name: &str) -> Option<&'static NodeMetadata> {
    native_registry::BLUEPRINT_REGISTRY
        .iter()
        .find(|n| n.name == name)
}

#[cfg(not(feature = "native"))]
pub fn get_node_by_name(_name: &str) -> Option<&'static NodeMetadata> {
    None
}

#[cfg(feature = "native")]
pub fn get_all_categories() -> Vec<&'static str> {
    let mut cats: Vec<_> = native_registry::BLUEPRINT_REGISTRY
        .iter()
        .map(|n| n.category)
        .collect();
    cats.sort_unstable();
    cats.dedup();
    cats
}

#[cfg(not(feature = "native"))]
pub fn get_all_categories() -> Vec<&'static str> {
    vec![]
}

// ── Type constructor registry ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypeConstructorMetadata {
    pub name: &'static str,
    pub params_count: usize,
    pub category: &'static str,
    pub description: &'static str,
    pub example: &'static str,
}

pub mod native_type_registry {
    use super::TypeConstructorMetadata;

    #[cfg(feature = "native")]
    use linkme::distributed_slice;

    #[cfg(feature = "native")]
    #[distributed_slice]
    pub static TYPE_CONSTRUCTOR_REGISTRY: [TypeConstructorMetadata] = [..];
}

#[cfg(feature = "native")]
pub use native_type_registry::TYPE_CONSTRUCTOR_REGISTRY;

#[cfg(feature = "native")]
pub fn get_all_type_constructors() -> &'static [TypeConstructorMetadata] {
    &native_type_registry::TYPE_CONSTRUCTOR_REGISTRY
}

#[cfg(not(feature = "native"))]
pub fn get_all_type_constructors() -> &'static [TypeConstructorMetadata] {
    &[]
}

#[cfg(feature = "native")]
pub fn get_type_constructors_by_category(category: &str) -> Vec<&'static TypeConstructorMetadata> {
    native_type_registry::TYPE_CONSTRUCTOR_REGISTRY
        .iter()
        .filter(|tc| tc.category == category)
        .collect()
}

#[cfg(not(feature = "native"))]
pub fn get_type_constructors_by_category(_category: &str) -> Vec<&'static TypeConstructorMetadata> {
    vec![]
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
