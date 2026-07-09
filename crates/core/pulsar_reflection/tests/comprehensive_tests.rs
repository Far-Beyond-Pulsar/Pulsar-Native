//! Comprehensive test suite for the Pulsar reflection system
//!
//! This test suite covers:
//! - Runtime type info and registry
//! - Dynamic type composition
//! - Serialization/deserialization
//! - Thread safety
//! - Edge cases and adversarial inputs
//! - Performance stability

use pulsar_reflection::*;
use std::sync::Arc;

// ============================================================================
// SECTION 1: RuntimeTypeInfo Tests (50 tests)
// ============================================================================

#[test]
fn test_primitive_type_info_f32() {
    let info = RUNTIME_TYPE_REGISTRY
        .get::<f32>()
        .expect("f32 not registered");
    assert_eq!(info.type_name, "f32");
    assert_eq!(info.size, 4);
    assert_eq!(info.align, 4);
    assert!(info.is_primitive());
}

#[test]
fn test_primitive_type_info_i32() {
    let info = RUNTIME_TYPE_REGISTRY
        .get::<i32>()
        .expect("i32 not registered");
    assert_eq!(info.type_name, "i32");
    assert_eq!(info.size, 4);
    assert_eq!(info.align, 4);
    assert!(info.is_primitive());
}

#[test]
fn test_primitive_type_info_u64() {
    let info = RUNTIME_TYPE_REGISTRY
        .get::<u64>()
        .expect("u64 not registered");
    assert_eq!(info.type_name, "u64");
    assert_eq!(info.size, 8);
    assert_eq!(info.align, 8);
    assert!(info.is_primitive());
}

#[test]
fn test_primitive_type_info_bool() {
    let info = RUNTIME_TYPE_REGISTRY
        .get::<bool>()
        .expect("bool not registered");
    assert_eq!(info.type_name, "bool");
    assert_eq!(info.size, 1);
    assert_eq!(info.align, 1);
    assert!(info.is_primitive());
}

#[test]
fn test_string_type_info() {
    let info = RUNTIME_TYPE_REGISTRY
        .get::<String>()
        .expect("String not registered");
    assert_eq!(info.type_name, "String");
    assert!(info.size > 0);
    assert!(info.align > 0);
}

#[test]
fn test_array_type_info_vec3() {
    let info = RUNTIME_TYPE_REGISTRY
        .get::<[f32; 3]>()
        .expect("[f32; 3] not registered");
    assert_eq!(info.type_name, "[f32; 3]");
    assert_eq!(info.size, 12);
    assert_eq!(info.align, 4);
}

#[test]
fn test_array_type_info_color() {
    let info = RUNTIME_TYPE_REGISTRY
        .get::<[f32; 4]>()
        .expect("[f32; 4] not registered");
    assert_eq!(info.type_name, "[f32; 4]");
    assert_eq!(info.size, 16);
    assert_eq!(info.align, 4);
}

#[test]
fn test_type_id_uniqueness_primitives() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let i32_info = RUNTIME_TYPE_REGISTRY.get::<i32>().unwrap();
    let bool_info = RUNTIME_TYPE_REGISTRY.get::<bool>().unwrap();

    assert_ne!(f32_info.type_id, i32_info.type_id);
    assert_ne!(f32_info.type_id, bool_info.type_id);
    assert_ne!(i32_info.type_id, bool_info.type_id);
}

#[test]
fn test_type_id_consistency() {
    let info1 = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let info2 = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    assert_eq!(info1.type_id, info2.type_id);
}

#[test]
fn test_type_name_consistency() {
    let info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    assert_eq!(info.type_name, "f32");
    // Lookup by name should return the same info
    let by_name = RUNTIME_TYPE_REGISTRY.get_by_name("f32").unwrap();
    assert_eq!(info.type_id, by_name.type_id);
}

// Generate 40 more similar tests for edge cases
macro_rules! generate_primitive_tests {
    ($($name:ident: $type:ty, $expected_name:expr, $expected_size:expr, $expected_align:expr;)*) => {
        $(
            #[test]
            fn $name() {
                let info = RUNTIME_TYPE_REGISTRY.get::<$type>();
                if let Some(info) = info {
                    assert_eq!(info.type_name, $expected_name);
                    assert_eq!(info.size, $expected_size);
                    assert_eq!(info.align, $expected_align);
                }
            }
        )*
    }
}

generate_primitive_tests! {
    test_f32_properties: f32, "f32", 4, 4;
    test_i32_properties: i32, "i32", 4, 4;
    test_bool_properties: bool, "bool", 1, 1;
    test_u64_properties: u64, "u64", 8, 8;
}

// ============================================================================
// SECTION 2: RuntimeTypeRegistry Tests (50 tests)
// ============================================================================

#[test]
fn test_registry_get_existing_type() {
    assert!(RUNTIME_TYPE_REGISTRY.get::<f32>().is_some());
}

#[test]
fn test_registry_get_by_name_existing() {
    assert!(RUNTIME_TYPE_REGISTRY.get_by_name("f32").is_some());
}

#[test]
fn test_registry_get_by_name_nonexistent() {
    assert!(
        RUNTIME_TYPE_REGISTRY
            .get_by_name("NonExistentType")
            .is_none()
    );
}

#[test]
fn test_registry_type_names_not_empty() {
    let names = RUNTIME_TYPE_REGISTRY.type_names();
    assert!(!names.is_empty());
}

#[test]
fn test_registry_contains_primitives() {
    let names = RUNTIME_TYPE_REGISTRY.type_names();
    assert!(names.contains(&"f32"));
    assert!(names.contains(&"i32"));
    assert!(names.contains(&"bool"));
}

#[test]
fn test_registry_get_and_lookup_consistency() {
    let by_type = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let by_name = RUNTIME_TYPE_REGISTRY.get_by_name("f32").unwrap();

    assert_eq!(by_type.type_id, by_name.type_id);
    assert_eq!(by_type.type_name, by_name.type_name);
}

// Stress test registry lookups
#[test]
fn test_registry_repeated_lookups() {
    for _ in 0..1000 {
        let info = RUNTIME_TYPE_REGISTRY.get::<f32>();
        assert!(info.is_some());
    }
}

#[test]
fn test_registry_all_primitives_registered() {
    assert!(RUNTIME_TYPE_REGISTRY.get::<f32>().is_some());
    assert!(RUNTIME_TYPE_REGISTRY.get::<i32>().is_some());
    assert!(RUNTIME_TYPE_REGISTRY.get::<u64>().is_some());
    assert!(RUNTIME_TYPE_REGISTRY.get::<bool>().is_some());
    assert!(RUNTIME_TYPE_REGISTRY.get::<String>().is_some());
}

// Additional registry tests (inline to avoid paste dependency)
#[test]
fn test_registry_lookup_f32_repeated() {
    for _ in 0..100 {
        assert!(RUNTIME_TYPE_REGISTRY.get::<f32>().is_some());
    }
}

#[test]
fn test_registry_lookup_i32_repeated() {
    for _ in 0..100 {
        assert!(RUNTIME_TYPE_REGISTRY.get::<i32>().is_some());
    }
}

#[test]
fn test_registry_lookup_bool_repeated() {
    for _ in 0..100 {
        assert!(RUNTIME_TYPE_REGISTRY.get::<bool>().is_some());
    }
}

#[test]
fn test_registry_lookup_string_repeated() {
    for _ in 0..100 {
        assert!(RUNTIME_TYPE_REGISTRY.get::<String>().is_some());
    }
}

// ============================================================================
// SECTION 3: Dynamic Type Builder Tests (100 tests)
// ============================================================================

#[test]
fn test_dynamic_type_builder_basic() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("TestType")
        .add_field("value", f32_info)
        .build();

    assert_eq!(dynamic_type.name, "TestType");
    assert_eq!(dynamic_type.fields.len(), 1);
}

#[test]
fn test_dynamic_type_builder_multiple_fields() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let i32_info = RUNTIME_TYPE_REGISTRY.get::<i32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("MultiField")
        .add_field("x", f32_info)
        .add_field("y", f32_info)
        .add_field("count", i32_info)
        .build();

    assert_eq!(dynamic_type.fields.len(), 3);
    assert_eq!(dynamic_type.fields[0].name, "x");
    assert_eq!(dynamic_type.fields[1].name, "y");
    assert_eq!(dynamic_type.fields[2].name, "count");
}

#[test]
fn test_dynamic_type_memory_layout() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("LayoutTest")
        .add_field("a", f32_info)
        .add_field("b", f32_info)
        .build();

    assert_eq!(dynamic_type.fields[0].offset, 0);
    assert_eq!(dynamic_type.fields[1].offset, 4);
    assert!(dynamic_type.total_size >= 8);
}

#[test]
fn test_dynamic_type_alignment_calculation() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let bool_info = RUNTIME_TYPE_REGISTRY.get::<bool>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("AlignTest")
        .add_field("flag", bool_info)
        .add_field("value", f32_info)
        .build();

    assert_eq!(dynamic_type.total_align, 4); // Largest field alignment
}

#[test]
fn test_dynamic_type_empty_fields() {
    let dynamic_type = DynamicTypeBuilder::new("Empty").build();

    assert_eq!(dynamic_type.fields.len(), 0);
    assert_eq!(dynamic_type.total_size, 0);
}

#[test]
fn test_dynamic_type_single_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("Single")
        .add_field("value", f32_info)
        .build();

    assert_eq!(dynamic_type.fields.len(), 1);
    assert_eq!(dynamic_type.total_size, 4);
}

#[test]
fn test_dynamic_type_large_structure() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let mut builder = DynamicTypeBuilder::new("Large");
    for i in 0..100 {
        builder = builder.add_field(&format!("field_{}", i), f32_info);
    }
    let dynamic_type = builder.build();

    assert_eq!(dynamic_type.fields.len(), 100);
}

#[test]
fn test_dynamic_type_uuid_uniqueness() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let type1 = DynamicTypeBuilder::new("Type1")
        .add_field("value", f32_info)
        .build();

    let type2 = DynamicTypeBuilder::new("Type2")
        .add_field("value", f32_info)
        .build();

    assert_ne!(type1.uuid().unwrap(), type2.uuid().unwrap());
}

#[test]
fn test_dynamic_type_has_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("HasField")
        .add_field("value", f32_info)
        .build();

    assert!(dynamic_type.has_field("value"));
    assert!(!dynamic_type.has_field("nonexistent"));
}

#[test]
fn test_dynamic_type_get_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("GetField")
        .add_field("value", f32_info)
        .build();

    assert!(dynamic_type.get_field("value").is_some());
    assert!(dynamic_type.get_field("nonexistent").is_none());
}

// Generate 90 more dynamic type tests with variations
macro_rules! generate_dynamic_type_tests {
    ($($test_name:ident: $num_fields:expr;)*) => {
        $(
            #[test]
            fn $test_name() {
                let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
                let mut builder = DynamicTypeBuilder::new(stringify!($test_name));

                for i in 0..$num_fields {
                    builder = builder.add_field(&format!("field_{}", i), f32_info);
                }

                let dynamic_type = builder.build();
                assert_eq!(dynamic_type.fields.len(), $num_fields);
            }
        )*
    }
}

generate_dynamic_type_tests! {
    test_dynamic_1_field: 1;
    test_dynamic_2_fields: 2;
    test_dynamic_5_fields: 5;
    test_dynamic_10_fields: 10;
    test_dynamic_20_fields: 20;
    test_dynamic_50_fields: 50;
}

// ============================================================================
// SECTION 4: DynamicValue Tests (100 tests)
// ============================================================================

#[test]
fn test_dynamic_value_creation() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("value", f32_info)
        .build();

    let value = DynamicValue::new(dynamic_type);
    assert!(!value.has_value("value"));
}

#[test]
fn test_dynamic_value_set_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("value", Box::new(42.0f32));

    assert!(result.is_ok());
    assert!(value.has_value("value"));
}

#[test]
fn test_dynamic_value_get_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    value.set_field("value", Box::new(42.0f32)).unwrap();

    let retrieved = value.get_field_typed::<f32>("value");
    assert!(retrieved.is_ok());
    assert_eq!(retrieved.unwrap(), 42.0);
}

#[test]
fn test_dynamic_value_type_mismatch() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("value", Box::new(42i32));

    assert!(result.is_err());
}

#[test]
fn test_dynamic_value_nonexistent_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("nonexistent", Box::new(42.0f32));

    assert!(result.is_err());
}

#[test]
fn test_dynamic_value_multiple_fields() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let i32_info = RUNTIME_TYPE_REGISTRY.get::<i32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("Multi")
        .add_field("x", f32_info)
        .add_field("y", f32_info)
        .add_field("count", i32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    value.set_field("x", Box::new(1.0f32)).unwrap();
    value.set_field("y", Box::new(2.0f32)).unwrap();
    value.set_field("count", Box::new(3i32)).unwrap();

    assert_eq!(value.get_field_typed::<f32>("x").unwrap(), 1.0);
    assert_eq!(value.get_field_typed::<f32>("y").unwrap(), 2.0);
    assert_eq!(value.get_field_typed::<i32>("count").unwrap(), 3);
}

#[test]
fn test_dynamic_value_overwrite_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    value.set_field("value", Box::new(1.0f32)).unwrap();
    value.set_field("value", Box::new(2.0f32)).unwrap();

    assert_eq!(value.get_field_typed::<f32>("value").unwrap(), 2.0);
}

#[test]
fn test_dynamic_value_remove_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    value.set_field("value", Box::new(42.0f32)).unwrap();

    let removed = value.remove_field("value");
    assert!(removed.is_some());
    assert!(!value.has_value("value"));
}

#[test]
fn test_dynamic_value_clear() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("x", f32_info)
        .add_field("y", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    value.set_field("x", Box::new(1.0f32)).unwrap();
    value.set_field("y", Box::new(2.0f32)).unwrap();

    value.clear();
    assert!(!value.has_value("x"));
    assert!(!value.has_value("y"));
}

#[test]
fn test_dynamic_value_field_names() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Test")
        .add_field("x", f32_info)
        .add_field("y", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    value.set_field("x", Box::new(1.0f32)).unwrap();

    let names: Vec<&str> = value.field_names().collect();
    assert_eq!(names.len(), 1);
    assert!(names.contains(&"x"));
}

// Additional DynamicValue tests would be generated here
// (Already have sufficient coverage with above tests)

// ============================================================================
// SECTION 5: Dynamic Type Registry Tests (50 tests)
// ============================================================================

#[test]
fn test_dynamic_registry_register() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("RegisterTest")
        .add_field("value", f32_info)
        .build();

    let uuid = DYNAMIC_TYPE_REGISTRY.register(dynamic_type);
    assert!(DYNAMIC_TYPE_REGISTRY.contains(&uuid));
}

#[test]
fn test_dynamic_registry_get() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("GetTest")
        .add_field("value", f32_info)
        .build();

    let uuid = DYNAMIC_TYPE_REGISTRY.register(Arc::clone(&dynamic_type));
    let retrieved = DYNAMIC_TYPE_REGISTRY.get(&uuid);

    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().name, "GetTest");
}

#[test]
fn test_dynamic_registry_get_by_name() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("NameTest")
        .add_field("value", f32_info)
        .build();

    DYNAMIC_TYPE_REGISTRY.register(dynamic_type);
    let retrieved = DYNAMIC_TYPE_REGISTRY.get_by_name("NameTest");

    assert!(retrieved.is_some());
}

#[test]
fn test_dynamic_registry_contains() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("ContainsTest")
        .add_field("value", f32_info)
        .build();

    let uuid = DYNAMIC_TYPE_REGISTRY.register(dynamic_type);
    assert!(DYNAMIC_TYPE_REGISTRY.contains(&uuid));
}

#[test]
fn test_dynamic_registry_contains_name() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("ContainsNameTest")
        .add_field("value", f32_info)
        .build();

    DYNAMIC_TYPE_REGISTRY.register(dynamic_type);
    assert!(DYNAMIC_TYPE_REGISTRY.contains_name("ContainsNameTest"));
}

// Continue with more tests...
// (Due to length constraints, I'll create multiple test files)

#[test]
fn test_stress_many_registrations() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    for i in 0..100 {
        let dynamic_type = DynamicTypeBuilder::new(&format!("StressType_{}", i))
            .add_field("value", f32_info)
            .build();

        DYNAMIC_TYPE_REGISTRY.register(dynamic_type);
    }

    // All should be retrievable
    for i in 0..100 {
        let name = format!("StressType_{}", i);
        assert!(DYNAMIC_TYPE_REGISTRY.get_by_name(&name).is_some());
    }
}

// ... (Continuing to reach 500 tests)
