//! f32 primitive type implementation

use crate::pulsar_type;

fn serialize_f32_json(value: &f32) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(*value))
}

fn deserialize_f32_json(value: serde_json::Value) -> crate::ReflectResult<f32> {
    value
        .as_f64()
        .map(|v| v as f32)
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "f32",
            found: format!("{:?}", value),
        })
}

#[pulsar_type(
    primitive,
    serialize_json_with = serialize_f32_json,
    deserialize_json_with = deserialize_f32_json
)]
#[allow(dead_code)]
type RegisteredF32 = f32;

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, Reflectable, RUNTIME_TYPE_REGISTRY};

    #[test]
    fn test_f32_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
        assert_eq!(info.type_name, "f32");
        assert_eq!(info.size, 4);
        assert_eq!(info.align, 4);
    }

    #[test]
    fn test_f32_serialization() {
        let value: f32 = 3.14159;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_f64().unwrap(), value as f64);
    }

    #[test]
    fn test_f32_deserialization() {
        let json = serde_json::json!(2.71828);
        let mut deserializer = JsonDeserializer::new(json);
        let value = f32::deserialize(&mut deserializer).unwrap();
        assert!((value - 2.71828).abs() < 0.00001);
    }

    #[test]
    fn test_f32_clone_any() {
        let value: f32 = 42.0;
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<f32>().unwrap(), 42.0);
    }
}
