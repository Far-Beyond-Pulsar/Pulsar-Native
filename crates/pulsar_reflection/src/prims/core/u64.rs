//! u64 primitive type implementation

use crate::pulsar_type;

#[pulsar_type(
    serialize_json_with = serialize_u64_json,
    deserialize_json_with = deserialize_u64_json
)]
#[allow(dead_code)]
type RegisteredU64 = u64;

fn serialize_u64_json(value: &u64) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(*value))
}

fn deserialize_u64_json(value: serde_json::Value) -> crate::ReflectResult<u64> {
    value
        .as_u64()
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "u64",
            found: format!("{:?}", value),
        })
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_u64_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<u64>().unwrap();
        assert_eq!(info.type_name, "u64");
        assert_eq!(info.size, 8);
        assert_eq!(info.align, 8);
    }

    #[test]
    fn test_u64_serialization() {
        let value: u64 = 18446744073709551615;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_u64().unwrap(), value);
    }

    #[test]
    fn test_u64_deserialization() {
        let json = serde_json::json!(999999);
        let mut deserializer = JsonDeserializer::new(json);
        let value = u64::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, 999999);
    }

    #[test]
    fn test_u64_clone_any() {
        let value: u64 = 12345678;
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<u64>().unwrap(), 12345678);
    }
}
