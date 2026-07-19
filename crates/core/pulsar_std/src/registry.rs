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
    /// Direct runtime type info accessor (populated by macro for Reflectable types)
    pub type_info_fn: Option<fn() -> Option<&'static pulsar_reflection::RuntimeTypeInfo>>,
}

impl NodeParameter {
    /// Get runtime type information by looking up the type name in the registry.
    ///
    /// First tries the direct type_info_fn accessor (for Reflectable types),
    /// then falls back to string-based lookup for compatibility.
    pub fn get_type_info(&self) -> Option<&'static pulsar_reflection::RuntimeTypeInfo> {
        // Try direct accessor first (faster, O(1))
        if let Some(type_info_fn) = self.type_info_fn {
            if let Some(info) = type_info_fn() {
                return Some(info);
            }
        }

        // Fallback to string-based lookup for non-Reflectable types
        pulsar_reflection::RUNTIME_TYPE_REGISTRY.get_by_name(self.ty)
    }
}

/// Import statement metadata for a blueprint node
#[derive(Debug, Clone)]
pub struct NodeImport {
    pub crate_name: &'static str,
    pub items: &'static [&'static str],
}

/// Output parameter metadata — describes a named output pin on a multi-output node.
/// Baked at compile time by the `#[blueprint]` macro from `#[output]` attributes or `bp_return!`.
#[derive(Debug, Clone)]
pub struct OutputParamMeta {
    pub name: &'static str,
    pub ty: &'static str,
    /// `std::mem::size_of::<T>()` for this output's type, set by the macro.
    pub size: usize,
    /// `std::mem::align_of::<T>()` for this output's type, set by the macro.
    pub align: usize,
}

/// Conversion metadata — declares that this node converts one type to another.
/// Baked at compile time by the `#[blueprint]` macro from the `#[conversion]` attribute.
#[derive(Debug, Clone)]
pub struct ConversionMeta {
    pub from_type: &'static str,
    pub to_type: &'static str,
    pub lossless: bool,
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
    /// Direct runtime type info accessor for return type (populated by macro for Reflectable types)
    pub return_type_info_fn: Option<fn() -> Option<&'static pulsar_reflection::RuntimeTypeInfo>>,
    pub exec_inputs: &'static [&'static str],
    pub exec_outputs: &'static [&'static str],
    pub function_source: &'static str,
    pub documentation: &'static [&'static str],
    pub category: &'static str,
    pub color: Option<&'static str>,
    pub imports: &'static [NodeImport],
    /// Named output pins for multi-output nodes. Empty `&[]` for single-output nodes.
    pub output_params: &'static [OutputParamMeta],
    /// If `Some`, this node performs an explicit type conversion from
    /// `conversion.from_type` to `conversion.to_type`.  The compiler uses
    /// this to auto-insert conversion nodes when connecting mismatched types.
    pub conversion: Option<ConversionMeta>,
}

impl NodeMetadata {
    /// Get runtime type information for the return type by looking up in the registry.
    ///
    /// First tries the direct return_type_info_fn accessor (for Reflectable types),
    /// then falls back to string-based lookup. Returns None if the function returns void.
    pub fn get_return_type_info(&self) -> Option<&'static pulsar_reflection::RuntimeTypeInfo> {
        // Try direct accessor first (faster, O(1))
        if let Some(type_info_fn) = self.return_type_info_fn {
            if let Some(info) = type_info_fn() {
                return Some(info);
            }
        }

        // Fallback to string-based lookup for non-Reflectable types
        self.return_type
            .and_then(|type_name| pulsar_reflection::RUNTIME_TYPE_REGISTRY.get_by_name(type_name))
    }
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

// ── RuntimeTypeInfo Integration ──────────────────────────────────────────────

impl NodeMetadata {
    /// Validate that all parameter and return types are registered in RuntimeTypeRegistry
    ///
    /// Returns a list of unregistered type names. Empty list means all types are registered.
    pub fn validate_type_registration(&self) -> Vec<&'static str> {
        let mut unregistered = Vec::new();

        // Check parameters
        for param in self.params {
            if param.get_type_info().is_none() {
                unregistered.push(param.ty);
            }
        }

        // Check return type
        if self.return_type.is_some() && self.get_return_type_info().is_none() {
            if let Some(ty) = self.return_type {
                unregistered.push(ty);
            }
        }

        // Check output param types
        for out in self.output_params {
            let info = pulsar_reflection::RUNTIME_TYPE_REGISTRY.get_by_name(out.ty);
            if info.is_none() {
                unregistered.push(out.ty);
            }
        }

        unregistered
    }

    /// Check if all types used by this node are properly registered
    pub fn has_all_types_registered(&self) -> bool {
        self.validate_type_registration().is_empty()
    }

    /// Get all parameter type info as a Vec
    ///
    /// Skips parameters that don't have runtime type info available.
    pub fn get_param_type_infos(&self) -> Vec<&'static pulsar_reflection::RuntimeTypeInfo> {
        self.params
            .iter()
            .filter_map(|p| p.get_type_info())
            .collect()
    }
}

/// Validate all blueprint nodes have their types registered
///
/// Returns a report of nodes with unregistered types. Call this during
/// engine initialization to catch missing Reflectable implementations.
#[cfg(feature = "native")]
pub fn validate_all_node_types() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut issues = Vec::new();

    for node in get_all_nodes() {
        let unregistered = node.validate_type_registration();
        if !unregistered.is_empty() {
            issues.push((node.name, unregistered));
        }
    }

    if !issues.is_empty() {
        tracing::warn!(
            "Blueprint type validation: {} nodes have unregistered types",
            issues.len()
        );
        for (node_name, types) in &issues {
            tracing::warn!("  Node '{}': {:?}", node_name, types);
        }
    }

    issues
}

#[cfg(not(feature = "native"))]
pub fn validate_all_node_types() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![]
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
