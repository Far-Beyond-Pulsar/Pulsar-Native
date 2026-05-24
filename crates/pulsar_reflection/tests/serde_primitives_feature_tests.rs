use pulsar_reflection::{JsonDeserializer, JsonSerializer, Reflectable, RUNTIME_TYPE_REGISTRY};

#[cfg(not(feature = "prims-serde"))]
#[test]
fn test_serde_json_value_not_registered_without_feature() {
    assert!(RUNTIME_TYPE_REGISTRY.get::<serde_json::Value>().is_none());
}

#[cfg(feature = "prims-serde")]
#[test]
fn test_serde_json_value_registered_with_feature() {
    let info = RUNTIME_TYPE_REGISTRY.get::<serde_json::Value>().unwrap();
    assert_eq!(info.type_name, "serde_json :: Value");
}

#[cfg(feature = "prims-serde")]
#[test]
fn test_serde_json_value_round_trip_with_feature() {
    let value = serde_json::json!({"a": 1, "b": [true, "x"]});

    let mut serializer = JsonSerializer::new();
    value.serialize(&mut serializer).unwrap();
    assert_eq!(serializer.as_json(), &value);

    let mut deserializer = JsonDeserializer::new(value.clone());
    let restored = serde_json::Value::deserialize(&mut deserializer).unwrap();
    assert_eq!(restored, value);
}
