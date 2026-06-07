//! String type implementation

use crate::pulsar_type;

fn serialize_string_json(value: &String) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(value))
}

fn deserialize_string_json(value: serde_json::Value) -> crate::ReflectResult<String> {
    value
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "String",
            found: format!("{:?}", value),
        })
}

#[pulsar_type(
    serialize_json_with = serialize_string_json,
    deserialize_json_with = deserialize_string_json
)]
#[allow(dead_code)]
type RegisteredString = String;

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_string_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<String>().unwrap();
        assert_eq!(info.type_name, "String");
        assert_eq!(info.size, std::mem::size_of::<String>());
        assert_eq!(info.align, std::mem::align_of::<String>());
    }

    #[test]
    fn test_string_serialization() {
        let value = "Hello, Pulsar!".to_string();
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_str().unwrap(), "Hello, Pulsar!");
    }

    #[test]
    fn test_string_deserialization() {
        let json = serde_json::json!("Test String");
        let mut deserializer = JsonDeserializer::new(json);
        let value = String::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, "Test String");
    }

    #[test]
    fn test_string_clone_any() {
        let value = "Cloneable".to_string();
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<String>().unwrap(), "Cloneable");
    }

    #[test]
    fn test_empty_string() {
        let value = String::new();
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_str().unwrap(), "");
    }

    #[test]
    fn test_unicode_string() {
        let value = "Hello 世界 🌍".to_string();
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_str().unwrap(), "Hello 世界 🌍");
    }
}
