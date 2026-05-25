//! [f32; 4] primitive type implementation (Color)

use crate::pulsar_type;

fn serialize_color_json(value: &[f32; 4]) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!([value[0], value[1], value[2], value[3]]))
}

fn deserialize_color_json(value: serde_json::Value) -> crate::ReflectResult<[f32; 4]> {
    let arr = value
        .as_array()
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "[f32; 4]",
            found: format!("{:?}", value),
        })?;

    if arr.len() != 4 {
        return Err(crate::ReflectError::TypeMismatch {
            expected: "[f32; 4]",
            found: format!("array of length {}", arr.len()),
        });
    }

    Ok([
        arr[0].as_f64().unwrap_or(0.0) as f32,
        arr[1].as_f64().unwrap_or(0.0) as f32,
        arr[2].as_f64().unwrap_or(0.0) as f32,
        arr[3].as_f64().unwrap_or(0.0) as f32,
    ])
}

#[pulsar_type(
    primitive,
    serialize_json_with = serialize_color_json,
    deserialize_json_with = deserialize_color_json
)]
#[allow(dead_code)]
type RegisteredColor = [f32; 4];

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, Reflectable, RUNTIME_TYPE_REGISTRY};

    #[test]
    fn test_vec4_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<[f32; 4]>().unwrap();
        assert_eq!(info.type_name, "[f32; 4]");
        assert_eq!(info.size, 16);
        assert_eq!(info.align, 4);
    }

    #[test]
    fn test_vec4_serialization() {
        let value: [f32; 4] = [0.5, 0.6, 0.7, 1.0];
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 4);
    }

    #[test]
    fn test_vec4_deserialization() {
        let json = serde_json::json!([1.0, 0.5, 0.25, 0.8]);
        let mut deserializer = JsonDeserializer::new(json);
        let value = <[f32; 4]>::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, [1.0, 0.5, 0.25, 0.8]);
    }
}
