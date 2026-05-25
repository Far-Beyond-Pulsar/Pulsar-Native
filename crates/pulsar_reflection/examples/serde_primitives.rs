//! Example for feature-gated serde primitives.

#[cfg(feature = "prims-serde")]
fn main() {
    use pulsar_reflection::{JsonDeserializer, JsonSerializer, Reflectable, RUNTIME_TYPE_REGISTRY};

    let value = serde_json::json!({
        "kind": "config",
        "flags": ["a", "b"],
        "enabled": true
    });

    assert!(RUNTIME_TYPE_REGISTRY.get::<serde_json::Value>().is_some());

    let mut serializer = JsonSerializer::new();
    value.serialize(&mut serializer).unwrap();

    let mut deserializer = JsonDeserializer::new(serializer.into_json());
    let restored = serde_json::Value::deserialize(&mut deserializer).unwrap();

    println!("serde primitive value: {}", restored);
}

#[cfg(not(feature = "prims-serde"))]
fn main() {
    println!("Enable feature 'prims-serde' to run this example.");
}
