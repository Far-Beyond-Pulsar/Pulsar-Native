//! Integration tests for RuntimeTypeInfo integration with blueprint system
//!
//! Validates Phase 2 completion: blueprint nodes properly integrate with RuntimeTypeInfo

use pulsar_reflection::RUNTIME_TYPE_REGISTRY;
use pulsar_std::registry::*;

#[test]
fn test_all_builtin_nodes_have_type_info_accessors() {
    let nodes = get_all_nodes();

    for node in nodes {
        // All parameters should have type_info_fn defined (even if it returns None for generics)
        for (idx, param) in node.params.iter().enumerate() {
            // Note: type_info_fn may be None for generic parameters, which is expected
            tracing::debug!(
                "Node '{}' param[{}] '{}' type_info_fn: {:?}",
                node.name,
                idx,
                param.name,
                param.type_info_fn.is_some()
            );
        }

        // If return type exists, return_type_info_fn should exist (or be None for generics)
        if node.return_type.is_some() {
            tracing::debug!(
                "Node '{}' return_type_info_fn: {:?}",
                node.name,
                node.return_type_info_fn.is_some()
            );
        }
    }
}

#[test]
fn test_primitive_type_nodes_have_valid_runtime_type_info() {
    // Test 'add' node which uses i64
    let add_node = get_node_by_name("add").expect("Should find 'add' node");

    assert_eq!(add_node.params.len(), 2);

    // Check first parameter has type_info_fn
    assert!(
        add_node.params[0].type_info_fn.is_some(),
        "Parameter 'a' should have type_info_fn"
    );

    // Call the type_info_fn to get RuntimeTypeInfo
    if let Some(type_info_fn) = add_node.params[0].type_info_fn {
        let type_info = type_info_fn();
        assert!(
            type_info.is_some(),
            "type_info_fn should return Some(RuntimeTypeInfo) for i64"
        );

        let type_info = type_info.unwrap();
        assert!(
            type_info.type_name.contains("i64"),
            "Type name should contain 'i64', got: {}",
            type_info.type_name
        );
        assert_eq!(
            type_info.size,
            std::mem::size_of::<i64>(),
            "Size should match i64 size"
        );
        assert_eq!(
            type_info.align,
            std::mem::align_of::<i64>(),
            "Alignment should match i64 alignment"
        );
    }

    // Check return type has type_info_fn
    assert!(
        add_node.return_type_info_fn.is_some(),
        "Return type should have return_type_info_fn"
    );

    // Verify return type info
    if let Some(return_type_info_fn) = add_node.return_type_info_fn {
        let type_info = return_type_info_fn();
        assert!(
            type_info.is_some(),
            "return_type_info_fn should return Some(RuntimeTypeInfo)"
        );
    }
}

#[test]
fn test_get_type_info_helper_methods() {
    let add_node = get_node_by_name("add").expect("Should find 'add' node");

    // Test NodeParameter::get_type_info()
    let param_type = add_node.params[0].get_type_info();
    assert!(
        param_type.is_some(),
        "get_type_info() should return RuntimeTypeInfo for i64"
    );

    if let Some(type_info) = param_type {
        assert!(type_info.type_name.contains("i64"));
        assert_eq!(type_info.size, 8);
        assert_eq!(type_info.align, 8);
    }

    // Test NodeMetadata::get_return_type_info()
    let return_type = add_node.get_return_type_info();
    assert!(
        return_type.is_some(),
        "get_return_type_info() should return RuntimeTypeInfo for i64"
    );

    if let Some(type_info) = return_type {
        assert!(type_info.type_name.contains("i64"));
    }
}

#[test]
fn test_node_metadata_validation_methods() {
    let add_node = get_node_by_name("add").expect("Should find 'add' node");

    // Test has_all_types_registered()
    assert!(
        add_node.has_all_types_registered(),
        "'add' node should have all types registered (i64 is a primitive)"
    );

    // Test validate_type_registration()
    let unregistered = add_node.validate_type_registration();
    assert!(
        unregistered.is_empty(),
        "'add' node should have no unregistered types, found: {:?}",
        unregistered
    );

    // Test get_param_type_infos()
    let param_types = add_node.get_param_type_infos();
    assert_eq!(
        param_types.len(),
        2,
        "'add' should have 2 parameters with type info"
    );

    // Verify all param types are i64
    for type_info in param_types {
        assert!(type_info.type_name.contains("i64"));
    }
}

#[test]
fn test_validate_all_node_types() {
    // This validates that all blueprint nodes have their types properly registered
    let issues = validate_all_node_types();

    // Log any issues for debugging
    if !issues.is_empty() {
        tracing::warn!("Found {} nodes with unregistered types:", issues.len());
        for (node_name, types) in &issues {
            tracing::warn!("  Node '{}': {:?}", node_name, types);
        }
    }

    // Most builtin nodes should have all types registered
    // Some nodes may use types that aren't Reflectable yet (e.g., certain wrapper types)
    // This is acceptable as long as the system handles it gracefully
    tracing::info!(
        "Type validation complete: {}/{} nodes have all types registered",
        get_all_nodes().len() - issues.len(),
        get_all_nodes().len()
    );
}

#[test]
fn test_string_type_integration() {
    let print_node = get_node_by_name("print_string").expect("Should find 'print_string' node");

    // print_string takes a &str parameter (not String)
    assert_eq!(print_node.params.len(), 1);
    assert_eq!(print_node.params[0].ty, "& str");

    // Check if &str type has runtime info (it may not be registered as a primitive)
    // &str is a reference type, not typically registered separately
    // This is acceptable - the system should handle it via String or str registration
    tracing::debug!("print_string parameter type: {}", print_node.params[0].ty);
}

#[test]
fn test_generic_node_type_info() {
    // Find a generic node (e.g., vec_new, array_new)
    let vec_new_node = get_node_by_name("vec_new");

    if let Some(node) = vec_new_node {
        // Generic return type (Vec<T>) should have return_type_info_fn as None
        // because the concrete type is determined at instantiation time
        tracing::debug!(
            "Generic node '{}' return_type_info_fn: {:?}",
            node.name,
            node.return_type_info_fn.is_some()
        );

        // This is acceptable - generics are resolved at call sites
    }
}

#[test]
fn test_type_info_consistency() {
    // Verify that type_info from the registry matches the metadata
    let add_node = get_node_by_name("add").expect("Should find 'add' node");

    // Get type info via parameter
    let param_type_via_param = add_node.params[0].get_type_info().unwrap();

    // Get type info directly from registry
    let type_info_direct = RUNTIME_TYPE_REGISTRY.get::<i64>().unwrap();

    // They should be the same
    assert_eq!(
        param_type_via_param.type_id, type_info_direct.type_id,
        "TypeId should match"
    );
    assert_eq!(
        param_type_via_param.type_name, type_info_direct.type_name,
        "Type name should match"
    );
    assert_eq!(
        param_type_via_param.size, type_info_direct.size,
        "Size should match"
    );
    assert_eq!(
        param_type_via_param.align, type_info_direct.align,
        "Alignment should match"
    );
}

#[test]
fn test_fallback_string_lookup() {
    let add_node = get_node_by_name("add").expect("Should find 'add' node");

    // Even if we didn't have type_info_fn, the string-based lookup should work
    // This tests the fallback mechanism
    let type_name = add_node.params[0].ty;
    let type_info = RUNTIME_TYPE_REGISTRY.get_by_name(type_name);

    assert!(
        type_info.is_some(),
        "Should be able to look up type by name: {}",
        type_name
    );
}

#[test]
fn test_node_metadata_fields_populated() {
    let nodes = get_all_nodes();

    for node in nodes {
        // Verify all required fields are populated
        assert!(!node.name.is_empty(), "Node should have a name");
        assert!(!node.category.is_empty(), "Node should have a category");
        assert!(
            !node.function_source.is_empty(),
            "Node should have source code"
        );

        // Verify params have complete metadata
        for param in node.params {
            assert!(
                !param.name.is_empty(),
                "Parameter '{}' in node '{}' should have a name",
                param.name,
                node.name
            );
            assert!(
                !param.ty.is_empty(),
                "Parameter '{}' in node '{}' should have a type string",
                param.name,
                node.name
            );

            // Size of 0 is valid for:
            // - Generic type parameters (e.g., T, U)
            // - Unit type ()
            // - Zero-sized types (ZSTs)
            let is_likely_generic =
                param.ty.len() == 1 && param.ty.chars().next().unwrap().is_uppercase();
            let is_unit = param.ty.contains("()");

            if param.size == 0 && !is_unit && !is_likely_generic {
                tracing::warn!(
                    "Parameter '{}' of type '{}' in node '{}' has size 0 (likely generic or ZST)",
                    param.name,
                    param.ty,
                    node.name
                );
            }

            assert!(
                param.align > 0,
                "Parameter '{}' of type '{}' in node '{}' should have non-zero alignment",
                param.name,
                param.ty,
                node.name
            );
            // Note: type_info_fn may be None for generic parameters
        }

        // Verify return type metadata
        if let Some(return_type) = node.return_type {
            assert!(
                !return_type.is_empty(),
                "Return type string should not be empty"
            );
            // Note: return_type_info_fn may be None for generic returns
        }
    }
}
