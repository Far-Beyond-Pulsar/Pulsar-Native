//! serde_json::Value primitive type implementation

use crate::pulsar_type;

fn serialize_json_value_json(value: &serde_json::Value) -> crate::ReflectResult<serde_json::Value> {
    Ok(value.clone())
}

fn deserialize_json_value_json(
    value: serde_json::Value,
) -> crate::ReflectResult<serde_json::Value> {
    Ok(value)
}

#[pulsar_type(
    serialize_json_with = serialize_json_value_json,
    deserialize_json_with = deserialize_json_value_json
)]
#[allow(dead_code)]
type RegisteredJsonValue = serde_json::Value;

#[cfg(test)]
mod tests {
    use crate::{
        JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable, TypeStructure,
    };

    #[test]
    fn test_json_value_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<serde_json::Value>().unwrap();
        assert_eq!(info.type_name, "serde_json :: Value");
        assert!(matches!(info.structure, TypeStructure::Primitive));
    }

    #[test]
    fn test_json_value_round_trip() {
        let value = serde_json::json!({
            "name": "Pulsar",
            "items": [1, true, "x"],
            "nested": { "ok": true }
        });

        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();
        assert_eq!(serializer.as_json(), &value);

        let mut deserializer = JsonDeserializer::new(value.clone());
        let restored = serde_json::Value::deserialize(&mut deserializer).unwrap();
        assert_eq!(restored, value);
    }
}
