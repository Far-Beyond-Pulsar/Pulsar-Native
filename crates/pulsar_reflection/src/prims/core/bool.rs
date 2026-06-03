//! bool primitive type implementation

use crate::pulsar_type;

fn serialize_bool_json(value: &bool) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(*value))
}

fn deserialize_bool_json(value: serde_json::Value) -> crate::ReflectResult<bool> {
    value
        .as_bool()
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "bool",
            found: format!("{:?}", value),
        })
}

#[pulsar_type(
    primitive,
    serialize_json_with = serialize_bool_json,
    deserialize_json_with = deserialize_bool_json
)]
#[allow(dead_code)]
type RegisteredBool = bool;

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_bool_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<bool>().unwrap();
        assert_eq!(info.type_name, "bool");
        assert_eq!(info.size, 1);
        assert_eq!(info.align, 1);
    }

    #[test]
    fn test_bool_serialization_true() {
        let value = true;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert!(json.as_bool().unwrap());
    }

    #[test]
    fn test_bool_serialization_false() {
        let value = false;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert!(!json.as_bool().unwrap());
    }

    #[test]
    fn test_bool_deserialization() {
        let json = serde_json::json!(true);
        let mut deserializer = JsonDeserializer::new(json);
        let value = bool::deserialize(&mut deserializer).unwrap();
        assert!(value);
    }

    #[test]
    fn test_bool_clone_any() {
        let value = true;
        let boxed = value.clone_any();
        assert!(*boxed.downcast::<bool>().unwrap());
    }
}
