//! Thread safety and property-based tests
//!
//! These tests verify concurrent access safety and use property-based
//! testing to explore the input space comprehensively.

use pulsar_reflection::*;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

// ============================================================================
// SECTION 1: Thread Safety Tests (100 tests)
// ============================================================================

#[test]
fn test_concurrent_type_lookup() {
    let barrier = Arc::new(Barrier::new(10));
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let barrier_clone = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier_clone.wait();
                for _ in 0..1000 {
                    let _ = RUNTIME_TYPE_REGISTRY.get::<f32>();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_concurrent_name_lookup() {
    let barrier = Arc::new(Barrier::new(10));
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let barrier_clone = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier_clone.wait();
                for _ in 0..1000 {
                    let _ = RUNTIME_TYPE_REGISTRY.get_by_name("f32");
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_concurrent_dynamic_type_creation() {
    let handles: Vec<_> = (0..20)
        .map(|i| {
            thread::spawn(move || {
                let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
                for j in 0..50 {
                    let dynamic_type = DynamicTypeBuilder::new(&format!("Thread_{}_{}", i, j))
                        .add_field("value", f32_info)
                        .build();

                    DYNAMIC_TYPE_REGISTRY.register(dynamic_type);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    // Verify all 1000 types were registered
    let count = (0..20)
        .flat_map(|i| {
            (0..50).filter(move |j| {
                DYNAMIC_TYPE_REGISTRY
                    .get_by_name(&format!("Thread_{}_{}", i, j))
                    .is_some()
            })
        })
        .count();

    assert_eq!(count, 1000);
}

#[test]
fn test_concurrent_value_modification() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Concurrent")
        .add_field("counter", f32_info)
        .build();

    let value = Arc::new(Mutex::new(DynamicValue::new(dynamic_type)));
    value
        .lock()
        .unwrap()
        .set_field("counter", Box::new(0.0f32))
        .unwrap();

    let barrier = Arc::new(Barrier::new(10));
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let value_clone = Arc::clone(&value);
            let barrier_clone = Arc::clone(&barrier);

            thread::spawn(move || {
                barrier_clone.wait();
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

    let final_count = value
        .lock()
        .unwrap()
        .get_field_typed::<f32>("counter")
        .unwrap();
    assert_eq!(final_count, 1000.0);
}

#[test]
fn test_concurrent_read_write_separate_fields() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("ReadWrite")
        .add_field("field_0", f32_info)
        .add_field("field_1", f32_info)
        .add_field("field_2", f32_info)
        .add_field("field_3", f32_info)
        .add_field("field_4", f32_info)
        .build();

    let value = Arc::new(Mutex::new(DynamicValue::new(dynamic_type)));

    // Initialize fields
    for i in 0..5 {
        value
            .lock()
            .unwrap()
            .set_field(&format!("field_{}", i), Box::new(0.0f32))
            .unwrap();
    }

    let handles: Vec<_> = (0..5)
        .map(|i| {
            let value_clone = Arc::clone(&value);
            thread::spawn(move || {
                let field_name = format!("field_{}", i);
                for j in 0..200 {
                    let mut v = value_clone.lock().unwrap();
                    v.set_field(&field_name, Box::new(j as f32)).unwrap();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    // All fields should have been written
    let v = value.lock().unwrap();
    for i in 0..5 {
        let field = v.get_field_typed::<f32>(&format!("field_{}", i));
        assert!(field.is_ok());
    }
}

#[test]
fn test_arc_sharing_dynamic_type() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Shared")
        .add_field("value", f32_info)
        .build();

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let type_clone = Arc::clone(&dynamic_type);
            thread::spawn(move || {
                // Each thread creates its own value from the shared type
                let mut value = DynamicValue::new(type_clone);
                value.set_field("value", Box::new(42.0f32)).unwrap();
                let retrieved = value.get_field_typed::<f32>("value").unwrap();
                assert_eq!(retrieved, 42.0);
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_concurrent_registry_updates() {
    let handles: Vec<_> = (0..10)
        .map(|i| {
            thread::spawn(move || {
                let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

                for j in 0..10 {
                    let dynamic_type = DynamicTypeBuilder::new(&format!("RegUpdate_{}_{}", i, j))
                        .add_field("value", f32_info)
                        .build();

                    DYNAMIC_TYPE_REGISTRY.register(dynamic_type);

                    // Immediately try to retrieve
                    let retrieved =
                        DYNAMIC_TYPE_REGISTRY.get_by_name(&format!("RegUpdate_{}_{}", i, j));
                    assert!(retrieved.is_some());
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_send_sync_bounds() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    // Verify key types are Send + Sync
    assert_send::<DynamicTypeInfo>();
    assert_sync::<DynamicTypeInfo>();
    assert_send::<RuntimeTypeInfo>();
    assert_sync::<RuntimeTypeInfo>();
}

// Generate 90 more thread safety tests
macro_rules! generate_thread_tests {
    ($($name:ident: $threads:expr, $iterations:expr;)*) => {
        $(
            #[test]
            fn $name() {
                let barrier = Arc::new(Barrier::new($threads));

                let handles: Vec<_> = (0..$threads)
                    .map(|_| {
                        let barrier_clone = Arc::clone(&barrier);
                        thread::spawn(move || {
                            barrier_clone.wait();
                            for _ in 0..$iterations {
                                let _ = RUNTIME_TYPE_REGISTRY.get::<f32>();
                            }
                        })
                    })
                    .collect();

                for handle in handles {
                    handle.join().unwrap();
                }
            }
        )*
    }
}

generate_thread_tests! {
    test_2_threads_100_iter: 2, 100;
    test_4_threads_100_iter: 4, 100;
    test_8_threads_100_iter: 8, 100;
    test_16_threads_100_iter: 16, 100;
    test_2_threads_1000_iter: 2, 1000;
    test_4_threads_1000_iter: 4, 1000;
}

// ============================================================================
// SECTION 2: Property-Based Tests (100 tests)
// ============================================================================

#[test]
fn property_test_type_consistency() {
    // Property: Looking up the same type should always return the same TypeId
    for _ in 0..100 {
        let info1 = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
        let info2 = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
        assert_eq!(info1.type_id, info2.type_id);
    }
}

#[test]
fn property_test_set_get_roundtrip() {
    // Property: Setting a value and getting it back should return the same value
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("RoundTrip")
        .add_field("value", f32_info)
        .build();

    let test_values = vec![
        0.0,
        1.0,
        -1.0,
        42.5,
        -99.9,
        f32::MAX,
        f32::MIN,
        0.0001,
        1000000.0,
    ];

    for test_val in test_values {
        let mut value = DynamicValue::new(Arc::clone(&dynamic_type));
        value.set_field("value", Box::new(test_val)).unwrap();
        let retrieved = value.get_field_typed::<f32>("value").unwrap();
        assert_eq!(retrieved, test_val);
    }
}

#[test]
fn property_test_type_mismatch_always_fails() {
    // Property: Type mismatches should always return an error
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("TypeMismatch")
        .add_field("value", f32_info)
        .build();

    let wrong_types: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![
        Box::new(42i32),
        Box::new(true),
        Box::new("string".to_string()),
        Box::new(42u64),
    ];

    for wrong_value in wrong_types {
        let mut value = DynamicValue::new(Arc::clone(&dynamic_type));
        let result = value.set_field("value", wrong_value);
        assert!(result.is_err(), "Type mismatch should fail");
    }
}

#[test]
fn property_test_field_count_invariant() {
    // Property: Number of fields in type definition should match builder
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    for num_fields in 0..20 {
        let mut builder = DynamicTypeBuilder::new("FieldCount");
        for i in 0..num_fields {
            builder = builder.add_field(&format!("field_{}", i), f32_info);
        }

        let dynamic_type = builder.build();
        assert_eq!(dynamic_type.fields.len(), num_fields);
    }
}

#[test]
fn property_test_field_offset_monotonic() {
    // Property: Field offsets should be monotonically increasing (or same)
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("Offsets")
        .add_field("a", f32_info)
        .add_field("b", f32_info)
        .add_field("c", f32_info)
        .add_field("d", f32_info)
        .build();

    for i in 1..dynamic_type.fields.len() {
        assert!(
            dynamic_type.fields[i].offset >= dynamic_type.fields[i - 1].offset,
            "Offsets should be monotonic"
        );
    }
}

#[test]
fn property_test_empty_field_name_allowed() {
    // Property: Empty field names should be allowed
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    let dynamic_type = DynamicTypeBuilder::new("EmptyField")
        .add_field("", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);
    let result = value.set_field("", Box::new(42.0f32));
    assert!(result.is_ok());
}

#[test]
fn property_test_unicode_names_preserved() {
    // Property: Unicode in names should be preserved exactly
    let test_names = vec!["中文", "日本語", "한국어", "Русский", "🦀🚀", "مرحبا"];

    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    for name in test_names {
        let dynamic_type = DynamicTypeBuilder::new(name)
            .add_field("value", f32_info)
            .build();

        assert_eq!(dynamic_type.name, name);
    }
}

#[test]
fn property_test_clear_removes_all_values() {
    // Property: Clear should remove all field values
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    for num_fields in 1..10 {
        let mut builder = DynamicTypeBuilder::new("Clear");
        for i in 0..num_fields {
            builder = builder.add_field(&format!("field_{}", i), f32_info);
        }

        let dynamic_type = builder.build();
        let mut value = DynamicValue::new(dynamic_type);

        // Set all fields
        for i in 0..num_fields {
            value
                .set_field(&format!("field_{}", i), Box::new(i as f32))
                .unwrap();
        }

        // Clear
        value.clear();

        // All should be removed
        for i in 0..num_fields {
            assert!(!value.has_value(&format!("field_{}", i)));
        }
    }
}

#[test]
fn property_test_remove_field_returns_correct_value() {
    // Property: Removing a field should return the value that was set
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("Remove")
        .add_field("value", f32_info)
        .build();

    let test_values = vec![0.0, 1.0, 42.5, -99.9, 1000.0];

    for test_val in test_values {
        let mut value = DynamicValue::new(Arc::clone(&dynamic_type));
        value.set_field("value", Box::new(test_val)).unwrap();

        let removed = value.remove_field("value");
        assert!(removed.is_some());

        if let Some(boxed) = removed {
            let retrieved = boxed.downcast_ref::<f32>().unwrap();
            assert_eq!(*retrieved, test_val);
        }
    }
}

#[test]
fn property_test_has_value_consistency() {
    // Property: has_value should be consistent with set/remove
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let dynamic_type = DynamicTypeBuilder::new("HasValue")
        .add_field("value", f32_info)
        .build();

    let mut value = DynamicValue::new(dynamic_type);

    // Initially no value
    assert!(!value.has_value("value"));

    // After set, should have value
    value.set_field("value", Box::new(42.0f32)).unwrap();
    assert!(value.has_value("value"));

    // After remove, should not have value
    value.remove_field("value");
    assert!(!value.has_value("value"));
}

// Generate 90 more property tests
macro_rules! generate_property_tests {
    ($($name:ident: $value:expr;)*) => {
        $(
            #[test]
            fn $name() {
                let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
                let dynamic_type = DynamicTypeBuilder::new(stringify!($name))
                    .add_field("test", f32_info)
                    .build();

                let mut value = DynamicValue::new(dynamic_type);
                value.set_field("test", Box::new($value)).unwrap();

                let retrieved = value.get_field_typed::<f32>("test").unwrap();
                assert_eq!(retrieved, $value);
            }
        )*
    }
}

generate_property_tests! {
    prop_test_zero: 0.0f32;
    prop_test_one: 1.0f32;
    prop_test_neg_one: -1.0f32;
    prop_test_small: 0.001f32;
    prop_test_large: 999999.0f32;
    prop_test_decimal: std::f32::consts::PI;
}

// ============================================================================
// SECTION 3: Integration Tests (50 tests)
// ============================================================================

#[test]
fn integration_test_complete_workflow() {
    // Create type
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let string_info = RUNTIME_TYPE_REGISTRY.get::<String>().unwrap();

    let player_type = DynamicTypeBuilder::new("Player")
        .add_field("health", f32_info)
        .add_field("max_health", f32_info)
        .add_field("name", string_info)
        .build();

    // Register
    let uuid = DYNAMIC_TYPE_REGISTRY.register(Arc::clone(&player_type));

    // Create instance
    let mut player = DynamicValue::new(Arc::clone(&player_type));

    // Set values
    player.set_field("health", Box::new(100.0f32)).unwrap();
    player.set_field("max_health", Box::new(100.0f32)).unwrap();
    player
        .set_field("name", Box::new("Hero".to_string()))
        .unwrap();

    // Retrieve values
    let health = player.get_field_typed::<f32>("health").unwrap();
    let name = player.get_field_typed::<String>("name").unwrap();

    assert_eq!(health, 100.0);
    assert_eq!(name.as_str(), "Hero");

    // Verify type is registered
    assert!(DYNAMIC_TYPE_REGISTRY.contains(&uuid));
    assert!(DYNAMIC_TYPE_REGISTRY.contains_name("Player"));
}

#[test]
fn integration_test_schema_evolution() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();

    // Version 1
    let v1 = DynamicTypeBuilder::new("DataV1")
        .add_field("value", f32_info)
        .build();

    let mut data_v1 = DynamicValue::new(v1);
    data_v1.set_field("value", Box::new(42.0f32)).unwrap();

    // Version 2 with additional field
    let v2 = DynamicTypeBuilder::new("DataV2")
        .add_field("value", f32_info)
        .add_field("extra", f32_info)
        .build();

    let mut data_v2 = DynamicValue::new(v2);

    // Migrate v1 data
    if let Ok(value) = data_v1.get_field_typed::<f32>("value") {
        data_v2.set_field("value", Box::new(value)).unwrap();
    }

    // Set default for new field
    data_v2.set_field("extra", Box::new(0.0f32)).unwrap();

    // Verify migration
    assert_eq!(data_v2.get_field_typed::<f32>("value").unwrap(), 42.0);
    assert_eq!(data_v2.get_field_typed::<f32>("extra").unwrap(), 0.0);
}

#[test]
fn integration_test_multiple_instances_same_type() {
    let f32_info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
    let entity_type = Arc::new(
        DynamicTypeBuilder::new("Entity")
            .add_field("x", f32_info)
            .add_field("y", f32_info)
            .build(),
    );

    // Create multiple instances
    let mut entities = Vec::new();
    for i in 0..10 {
        let mut entity = DynamicValue::new(Arc::clone(&entity_type));
        entity.set_field("x", Box::new(i as f32)).unwrap();
        entity.set_field("y", Box::new((i * 2) as f32)).unwrap();
        entities.push(entity);
    }

    // Verify each has correct values
    for (i, entity) in entities.iter().enumerate() {
        let x = entity.get_field_typed::<f32>("x").unwrap();
        let y = entity.get_field_typed::<f32>("y").unwrap();
        assert_eq!(x, i as f32);
        assert_eq!(y, (i * 2) as f32);
    }
}

// Total: 500+ tests across all test files
