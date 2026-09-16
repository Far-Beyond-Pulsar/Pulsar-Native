//! i64 primitive type implementation

use crate::pulsar_type;

#[pulsar_type(
    serialize_json_with = serialize_i64_json,
    deserialize_json_with = deserialize_i64_json
)]
#[allow(dead_code)]
type RegisteredI64 = i64;

fn serialize_i64_json(value: &i64) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(*value))
}

fn deserialize_i64_json(value: serde_json::Value) -> crate::ReflectResult<i64> {
    value
        .as_i64()
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "i64",
            found: format!("{:?}", value),
        })
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_i64_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<i64>().unwrap();
        assert_eq!(info.type_name, "i64");
        assert_eq!(info.size, 8);
        assert_eq!(info.align, 8);
    }

    #[test]
    fn test_i64_serialization() {
        let value: i64 = 42;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_i64().unwrap(), value);
    }

    #[test]
    fn test_i64_deserialization() {
        let expected: i64 = -1234567890;
        let json = serde_json::json!(expected);
        let mut deserializer = JsonDeserializer::new(json);
        let value = i64::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, expected);
    }

    #[test]
    fn test_i64_clone_any() {
        let value: i64 = 999;
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<i64>().unwrap(), 999);
    }
}
