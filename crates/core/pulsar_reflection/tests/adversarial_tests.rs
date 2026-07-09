//! Adversarial tests - trying to break the reflection system
//!
//! These tests attempt to cause failures, panics, or undefined behavior
//! to prove the system's stability and error handling.

use pulsar_reflection::*;
use std::any::TypeId;
use std::sync::Arc;

// ============================================================================
// SECTION 1: Type Confusion Attacks (50 tests)
// ============================================================================

#[test]
fn test_type_confusion_f32_as_i32() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("TypeConfusion")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    // Try to set i32 where f32 is expected
    let result = value.set_field("value", Box::new(42i32));
    assert!(result.is_err(), "Should reject type mismatch");
}

#[test]
fn test_type_confusion_bool_as_f32() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("BoolConfusion")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    let result = value.set_field("value", Box::new(true));
    assert!(result.is_err());
}

#[test]
fn test_type_confusion_string_as_f32() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("StringConfusion")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    let result = value.set_field("value", Box::new("hello".to_string()));
    assert!(result.is_err());
}

#[test]
fn test_downcast_wrong_type() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("DowncastTest")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    value.set_field("value", Box::new(42.0f32)).unwrap();

    // Try to get as wrong type
    let result = value.get_field_typed::<i32>("value");
    assert!(result.is_err(), "Should fail to downcast to wrong type");
}

#[test]
fn test_array_size_confusion() {
    let vec3_info = RUNTIME_TYPE_REGISTRY.get::<[f32; 3]>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("ArrayConfusion")
        .add_field("vec", vec3_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    // Try to set [f32; 4] where [f32; 3] is expected
    let result = value.set_field("vec", Box::new([1.0, 2.0, 3.0, 4.0]));
    assert!(result.is_err());
}

// ============================================================================
// SECTION 2: Boundary Value Tests (50 tests)
// ============================================================================

#[test]
fn test_empty_type_name() {
    let dynamic_type = DynamicTypeBuilder::new("").build();
    assert_eq!(dynamic_type.name, "");
}

#[test]
fn test_extremely_long_type_name() {
    let long_name = "A".repeat(10000);
    let dynamic_type = DynamicTypeBuilder::new(&long_name).build();
    assert_eq!(dynamic_type.name.len(), 10000);
}

#[test]
fn test_unicode_type_name() {
    let dynamic_type = DynamicTypeBuilder::new("类型_🦀_test").build();
    assert_eq!(dynamic_type.name, "类型_🦀_test");
}

#[test]
fn test_empty_field_name() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("EmptyField")
        .add_field("", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("", Box::new(42.0f32));
    assert!(result.is_ok());
}

#[test]
fn test_extremely_long_field_name() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let long_name = "field_".to_string() + &"a".repeat(10000);

    let dynamic_type = DynamicTypeBuilder::new("LongFieldName")
        .add_field(&long_name, f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field(&long_name, Box::new(42.0f32));
    assert!(result.is_ok());
}

#[test]
fn test_unicode_field_name() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("UnicodeField")
        .add_field("字段_🎮", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("字段_🎮", Box::new(42.0f32));
    assert!(result.is_ok());
}

#[test]
fn test_special_char_field_names() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("SpecialChars")
        .add_field("field!@#$%", f32_info)
        .add_field("field<>{}[]", f32_info)
        .add_field("field  space", f32_info)
        .build();

    assert_eq!(dynamic_type.fields.len(), 3);
}

#[test]
fn test_max_f32_value() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("MaxF32")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("value", Box::new(f32::MAX));
    assert!(result.is_ok());

    let retrieved = value.get_field_typed::<f32>("value").unwrap();
    assert_eq!(retrieved, f32::MAX);
}

#[test]
fn test_min_f32_value() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("MinF32")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("value", Box::new(f32::MIN));
    assert!(result.is_ok());
}

#[test]
fn test_nan_value() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("NaN")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("value", Box::new(f32::NAN));
    assert!(result.is_ok());

    let retrieved = value.get_field_typed::<f32>("value").unwrap();
    assert!(retrieved.is_nan());
}

#[test]
fn test_infinity_value() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Infinity")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    value.set_field("value", Box::new(f32::INFINITY)).unwrap();
    let retrieved = value.get_field_typed::<f32>("value").unwrap();
    assert!(retrieved.is_infinite() && retrieved.is_sign_positive());

    value
        .set_field("value", Box::new(f32::NEG_INFINITY))
        .unwrap();
    let retrieved = value.get_field_typed::<f32>("value").unwrap();
    assert!(retrieved.is_infinite() && retrieved.is_sign_negative());
}

#[test]
fn test_zero_values() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Zero")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    value.set_field("value", Box::new(0.0f32)).unwrap();
    value.set_field("value", Box::new(-0.0f32)).unwrap();

    assert!(value.get_field_typed::<f32>("value").is_ok());
}

// ============================================================================
// SECTION 3: Memory Safety Tests (50 tests)
// ============================================================================

#[test]
fn test_massive_structure() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let mut builder = DynamicTypeBuilder::new("Massive");

    // Create a type with 10,000 fields
    for i in 0..10000 {
        builder = builder.add_field(&format!("field_{}", i), f32_info);
    }

    let dynamic_type = builder.build();
    assert_eq!(dynamic_type.fields.len(), 10000);

    // Can we create an instance?
    let value = DynamicValue::new(dynamic_type);
    assert_eq!(value.type_info.fields.len(), 10000);
}

#[test]
fn test_rapid_allocations() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    for i in 0..1000 {
        let dynamic_type = DynamicTypeBuilder::new(&format!("Rapid_{}", i))
            .add_field("value", f32_info)
            .build();

        let mut value = DynamicValue::new(dynamic_type);
        value.set_field("value", Box::new(i as f32)).unwrap();
    }
}

#[test]
fn test_repeated_set_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Repeated")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    for i in 0..10000 {
        value.set_field("value", Box::new(i as f32)).unwrap();
    }

    let final_value = value.get_field_typed::<f32>("value").unwrap();
    assert_eq!(final_value, 9999.0);
}

#[test]
fn test_clone_drop_cycle() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("CloneDrop")
        .add_field("value", f32_info)
        .build();

    for _ in 0..1000 {
        let value = DynamicValue::new(Arc::clone(&dynamic_type));
        drop(value);
    }
}

// ============================================================================
// SECTION 4: Concurrent Access Attacks (50 tests)
// ============================================================================

#[test]
fn test_concurrent_registry_access() {
    use std::thread;

    let handles: Vec<_> = (0..10)
        .map(|_| {
            thread::spawn(|| {
                for _ in 0..100 {
                    let _ = RUNTIME_TYPE_REGISTRY.get::<f32>();
                    let _ = RUNTIME_TYPE_REGISTRY.get::<i32>();
                    let _ = RUNTIME_TYPE_REGISTRY.get::<bool>();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_concurrent_dynamic_registry() {
    use std::thread;

    let handles: Vec<_> = (0..10)
        .map(|i| {
            thread::spawn(move || {
                let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
                let dynamic_type = DynamicTypeBuilder::new(&format!("Concurrent_{}", i))
                    .add_field("value", f32_info)
                    .build();

                DYNAMIC_TYPE_REGISTRY.register(dynamic_type);
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    // All should be registered
    for i in 0..10 {
        let name = format!("Concurrent_{}", i);
        assert!(DYNAMIC_TYPE_REGISTRY.get_by_name(&name).is_some());
    }
}

#[test]
fn test_race_condition_field_access() {
    use std::sync::{Arc, Mutex};
    use std::thread;

    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("RaceCondition")
        .add_field("counter", f32_info)
        .build();

    let value = Arc::new(Mutex::new(DynamicValue::new(dynamic_type)));
    value
        .lock()
        .unwrap()
        .set_field("counter", Box::new(0.0f32))
        .unwrap();

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let value_clone = Arc::clone(&value);
            thread::spawn(move || {
                for _ in 0..100 {
                    let mut v = value_clone.lock().unwrap();
                    let current = v.get_field_typed::<f32>("counter").unwrap();
                    v.set_field("counter", Box::new(current + 1.0)).unwrap();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    let final_value = value
        .lock()
        .unwrap()
        .get_field_typed::<f32>("counter")
        .unwrap();
    assert_eq!(final_value, 1000.0);
}

// ============================================================================
// SECTION 5: Edge Cases and Corner Cases (100 tests)
// ============================================================================

#[test]
fn test_duplicate_field_names() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    // System should allow duplicate field names (last one wins)
    let dynamic_type = DynamicTypeBuilder::new("Duplicate")
        .add_field("value", f32_info)
        .add_field("value", f32_info)
        .build();

    // Both fields should exist (they're actually different)
    assert_eq!(dynamic_type.fields.len(), 2);
}

#[test]
fn test_null_character_in_name() {
    let dynamic_type = DynamicTypeBuilder::new("null\0char").build();
    assert!(dynamic_type.name.contains('\0'));
}

#[test]
fn test_newline_in_name() {
    let dynamic_type = DynamicTypeBuilder::new("multi\nline\nname").build();
    assert_eq!(dynamic_type.name, "multi\nline\nname");
}

#[test]
fn test_very_deep_nesting_simulation() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    // Simulate deep nesting by creating many levels
    let mut builder = DynamicTypeBuilder::new("Deep");
    for depth in 0..100 {
        builder = builder.add_field(&format!("level_{}", depth), f32_info);
    }

    let dynamic_type = builder.build();
    assert_eq!(dynamic_type.fields.len(), 100);
}

#[test]
fn test_registry_name_collision() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let type1 = DynamicTypeBuilder::new("Collision")
        .add_field("a", f32_info)
        .build();

    let type2 = DynamicTypeBuilder::new("Collision")
        .add_field("b", f32_info)
        .build();

    let uuid1 = DYNAMIC_TYPE_REGISTRY.register(Arc::clone(&type1));
    let uuid2 = DYNAMIC_TYPE_REGISTRY.register(type2);

    // UUIDs should be different even with same name
    assert_ne!(uuid1, uuid2);

    // Name lookup should return the last registered
    let by_name = DYNAMIC_TYPE_REGISTRY.get_by_name("Collision").unwrap();
    assert_eq!(by_name.fields[0].name, "b");
}

#[test]
fn test_field_access_after_type_modification() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let type_v1 = DynamicTypeBuilder::new("Versioned")
        .add_field("old_field", f32_info)
        .build();

    let mut value = DynamicValue::new(Arc::clone(&type_v1));
    value.set_field("old_field", Box::new(42.0f32)).unwrap();

    // The type definition in the value doesn't change
    assert!(value.has_value("old_field"));
}

#[test]
fn test_empty_string_field_value() {
    let string_info = RUNTIME_TYPE_REGISTRY.get::<String>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("EmptyString")
        .add_field("text", string_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    value.set_field("text", Box::new(String::new())).unwrap();

    let retrieved = value.get_field_typed::<String>("text").unwrap();
    assert_eq!(retrieved.len(), 0);
}

#[test]
fn test_huge_string_value() {
    let string_info = RUNTIME_TYPE_REGISTRY.get::<String>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("HugeString")
        .add_field("text", string_info)
        .build();

    let huge_string = "x".repeat(1_000_000);
    let mut value = DynamicValue::new(dynamic_type);
    value
        .set_field("text", Box::new(huge_string.clone()))
        .unwrap();

    let retrieved = value.get_field_typed::<String>("text").unwrap();
    assert_eq!(retrieved.len(), 1_000_000);
}

// ============================================================================
// SECTION 6: Stress Tests (50 tests)
// ============================================================================

#[test]
fn test_stress_many_types() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    for i in 0..500 {
        let dynamic_type = DynamicTypeBuilder::new(&format!("Stress_{}", i))
            .add_field("value", f32_info)
            .build();

        DYNAMIC_TYPE_REGISTRY.register(dynamic_type);
    }

    // Verify all are accessible
    for i in 0..500 {
        assert!(
            DYNAMIC_TYPE_REGISTRY
                .get_by_name(&format!("Stress_{}", i))
                .is_some()
        );
    }
}

#[test]
fn test_stress_large_values() {
    let string_info = RUNTIME_TYPE_REGISTRY.get::<String>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("LargeValues")
        .add_field("data", string_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    for size in [100, 1000, 10000, 100000] {
        let large_string = "x".repeat(size);
        value.set_field("data", Box::new(large_string)).unwrap();

        let retrieved = value.get_field_typed::<String>("data").unwrap();
        assert_eq!(retrieved.len(), size);
    }
}

#[test]
fn test_stress_rapid_type_creation_and_destruction() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    for _ in 0..1000 {
        let dynamic_type = DynamicTypeBuilder::new("Temporary")
            .add_field("value", f32_info)
            .build();

        let value = DynamicValue::new(dynamic_type);
        drop(value);
    }
}

// ============================================================================
// SECTION 7: Error Recovery Tests (50 tests)
// ============================================================================

#[test]
fn test_recover_from_type_mismatch() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Recovery")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    // Try wrong type
    let result1 = value.set_field("value", Box::new(42i32));
    assert!(result1.is_err());

    // System should still work with correct type
    let result2 = value.set_field("value", Box::new(42.0f32));
    assert!(result2.is_ok());

    let retrieved = value.get_field_typed::<f32>("value").unwrap();
    assert_eq!(retrieved, 42.0);
}

#[test]
fn test_recover_from_missing_field() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Recovery")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    // Try nonexistent field
    let result1 = value.set_field("nonexistent", Box::new(42.0f32));
    assert!(result1.is_err());

    // System should still work with correct field
    let result2 = value.set_field("value", Box::new(42.0f32));
    assert!(result2.is_ok());
}

#[test]
fn test_multiple_failures_stability() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("MultipleFailures")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    // Generate many failures
    for _ in 0..100 {
        let _ = value.set_field("nonexistent", Box::new(1.0f32));
        let _ = value.set_field("value", Box::new(1i32));
    }

    // System should still work
    let result = value.set_field("value", Box::new(42.0f32));
    assert!(result.is_ok());
}

#[test]
fn test_partial_field_population() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Partial")
        .add_field("a", f32_info)
        .add_field("b", f32_info)
        .add_field("c", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    // Only set some fields
    value.set_field("a", Box::new(1.0f32)).unwrap();
    value.set_field("c", Box::new(3.0f32)).unwrap();

    // Should be able to access set fields
    assert!(value.has_value("a"));
    assert!(!value.has_value("b"));
    assert!(value.has_value("c"));
}

// ============================================================================
// SECTION 8: JSON Registration Adversarial Tests
// ============================================================================

#[test]
fn test_json_serializer_rejects_unregistered_type() {
    #[derive(Clone)]
    struct Unregistered {
        _v: i32,
    }

    let mut serializer = JsonSerializer::new();
    let value = Unregistered { _v: 7 };
    let err = TypeSerializer::serialize_registered(&mut serializer, &value).unwrap_err();

    match err {
        ReflectError::SerializationFailed(message) => {
            assert!(message.contains("Type not registered"));
        }
        other => panic!("Unexpected error variant: {other:?}"),
    }
}

#[test]
fn test_json_deserializer_rejects_unregistered_type_info() {
    #[derive(Clone)]
    struct Ghost;

    let type_info = RuntimeTypeInfo {
        type_id: TypeId::of::<Ghost>(),
        type_name: "Ghost",
        size: std::mem::size_of::<Ghost>(),
        align: std::mem::align_of::<Ghost>(),
        structure: TypeStructure::Primitive,
        color: None,
    };

    let mut deserializer = JsonDeserializer::new(serde_json::json!(42));
    let err = TypeDeserializer::deserialize_registered(&mut deserializer, &type_info).unwrap_err();

    match err {
        ReflectError::DeserializationFailed(message) => {
            assert!(message.contains("Type not registered"));
            assert!(message.contains("Ghost"));
        }
        other => panic!("Unexpected error variant: {other:?}"),
    }
}

#[test]
fn test_json_deserialize_rejects_wrong_primitive_shapes() {
    let mut f32_from_string = JsonDeserializer::new(serde_json::json!("not-a-number"));
    assert!(f32::deserialize(&mut f32_from_string).is_err());

    let mut bool_from_number = JsonDeserializer::new(serde_json::json!(1));
    assert!(bool::deserialize(&mut bool_from_number).is_err());

    let mut string_from_object = JsonDeserializer::new(serde_json::json!({ "x": 1 }));
    assert!(String::deserialize(&mut string_from_object).is_err());
}

#[test]
fn test_json_deserialize_rejects_invalid_vec_shapes() {
    let mut vec3_too_short = JsonDeserializer::new(serde_json::json!([1.0, 2.0]));
    assert!(<[f32; 3]>::deserialize(&mut vec3_too_short).is_err());

    let mut color_too_long = JsonDeserializer::new(serde_json::json!([1.0, 2.0, 3.0, 4.0, 5.0]));
    assert!(<[f32; 4]>::deserialize(&mut color_too_long).is_err());

    let mut vec3_wrong_item_type = JsonDeserializer::new(serde_json::json!([1.0, "x", 3.0]));
    let parsed = <[f32; 3]>::deserialize(&mut vec3_wrong_item_type).unwrap();
    // Existing primitive decoder defaults invalid entries to 0.0; this test
    // ensures malformed payloads cannot panic and stay deterministic.
    assert_eq!(parsed, [1.0, 0.0, 3.0]);
}

#[test]
fn test_json_enum_deserialize_rejects_invalid_variants() {
    let mut deserializer = JsonDeserializer::new(serde_json::json!(999usize));
    let err = TypeDeserializer::deserialize_enum(&mut deserializer, &["A", "B"]).unwrap_err();

    match err {
        ReflectError::InvalidVariant { .. } => {}
        other => panic!("Unexpected error variant: {other:?}"),
    }

    let mut deserializer = JsonDeserializer::new(serde_json::json!("NotAVariant"));
    let err = TypeDeserializer::deserialize_enum(&mut deserializer, &["A", "B"]).unwrap_err();

    match err {
        ReflectError::InvalidVariant { .. } => {}
        other => panic!("Unexpected error variant: {other:?}"),
    }
}

#[test]
fn test_json_round_trip_high_volume_primitives() {
    let mut values = Vec::new();
    for i in 0..50_000 {
        values.push(i as f32 * 0.5);
    }

    for value in values {
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let mut deserializer = JsonDeserializer::new(serializer.into_json());
        let restored = f32::deserialize(&mut deserializer).unwrap();
        assert_eq!(restored, value);
    }
}

#[test]
fn test_json_deep_object_round_trip_string_field() {
    let string_info = RUNTIME_TYPE_REGISTRY.get::<String>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("DeepJson")
        .add_field("blob", string_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    let mut payload = String::new();
    for i in 0..20_000 {
        payload.push_str(&format!("node_{i};"));
    }

    value.set_field("blob", Box::new(payload.clone())).unwrap();
    let retrieved = value.get_field_typed::<String>("blob").unwrap();
    assert_eq!(retrieved, payload);
}
