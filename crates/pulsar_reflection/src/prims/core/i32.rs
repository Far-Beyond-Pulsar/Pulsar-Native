//! i32 primitive type implementation

use crate::pulsar_type;

fn serialize_i32_json(value: &i32) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(*value))
}

fn deserialize_i32_json(value: serde_json::Value) -> crate::ReflectResult<i32> {
    value
        .as_i64()
        .map(|v| v as i32)
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "i32",
            found: format!("{:?}", value),
        })
}

#[pulsar_type(
    primitive,
    serialize_json_with = serialize_i32_json,
    deserialize_json_with = deserialize_i32_json
)]
#[allow(dead_code)]
type RegisteredI32 = i32;

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, Reflectable, RUNTIME_TYPE_REGISTRY};

    #[test]
    fn test_i32_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<i32>().unwrap();
        assert_eq!(info.type_name, "i32");
        assert_eq!(info.size, 4);
        assert_eq!(info.align, 4);
    }

    #[test]
    fn test_i32_serialization() {
        let value: i32 = -12345;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_i64().unwrap(), value as i64);
    }

    #[test]
    fn test_i32_deserialization() {
        let json = serde_json::json!(42);
        let mut deserializer = JsonDeserializer::new(json);
        let value = i32::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, 42);
    }

    #[test]
    fn test_i32_clone_any() {
        let value: i32 = -999;
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<i32>().unwrap(), -999);
    }
}
