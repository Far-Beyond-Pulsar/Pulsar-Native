//! [f32; 3] primitive type implementation (Vec3)

use crate::pulsar_type;

#[cfg_attr(feature = "ui", pulsar_type(
    serialize_json_with = serialize_vec3_json,
    deserialize_json_with = deserialize_vec3_json,
    editor = render_vec3_editor
))]
#[cfg_attr(not(feature = "ui"), pulsar_type(
    serialize_json_with = serialize_vec3_json,
    deserialize_json_with = deserialize_vec3_json
))]
#[allow(dead_code)]
type RegisteredVec3 = [f32; 3];

fn serialize_vec3_json(value: &[f32; 3]) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!([value[0], value[1], value[2]]))
}

fn deserialize_vec3_json(value: serde_json::Value) -> crate::ReflectResult<[f32; 3]> {
    let arr = value
        .as_array()
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "[f32; 3]",
            found: format!("{:?}", value),
        })?;

    if arr.len() != 3 {
        return Err(crate::ReflectError::TypeMismatch {
            expected: "[f32; 3]",
            found: format!("array of length {}", arr.len()),
        });
    }

    Ok([
        arr[0].as_f64().unwrap_or(0.0) as f32,
        arr[1].as_f64().unwrap_or(0.0) as f32,
        arr[2].as_f64().unwrap_or(0.0) as f32,
    ])
}

#[cfg(feature = "ui")]
fn render_vec3_editor(args: &crate::PropertyEditorArgs<'_>, cx: &gpui::App) -> gpui::AnyElement {
    use gpui::{prelude::*, *};
    use ui::{ActiveTheme, h_flex};

    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(args.display_name.to_string()),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(format!("{:?}", args.current_json)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_vec3_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<[f32; 3]>().unwrap();
        assert_eq!(info.type_name, "[f32; 3]");
        assert_eq!(info.size, 12);
        assert_eq!(info.align, 4);
    }

    #[test]
    fn test_vec3_serialization() {
        let value: [f32; 3] = [1.0, 2.0, 3.0];
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_f64().unwrap(), 1.0);
        assert_eq!(arr[1].as_f64().unwrap(), 2.0);
        assert_eq!(arr[2].as_f64().unwrap(), 3.0);
    }

    #[test]
    fn test_vec3_deserialization() {
        let json = serde_json::json!([4.0, 5.0, 6.0]);
        let mut deserializer = JsonDeserializer::new(json);
        let value = <[f32; 3]>::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, [4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_vec3_clone_any() {
        let value: [f32; 3] = [7.0, 8.0, 9.0];
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<[f32; 3]>().unwrap(), [7.0, 8.0, 9.0]);
    }
}
