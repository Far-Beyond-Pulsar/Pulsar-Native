//! f32 primitive type implementation

use crate::pulsar_type;

// M3-alpha Task 2 (audit follow-up): see bool.rs — `editor` is GPUI-typed and
// only present behind `prims-gpui`.
#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_f32_json,
        deserialize_json_with = deserialize_f32_json,
        editor = render_f32_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_f32_json,
        deserialize_json_with = deserialize_f32_json
    )
)]
type RegisteredF32 = f32;

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

#[cfg(feature = "prims-gpui")]
fn render_f32_editor(args: &crate::PropertyEditorArgs<'_>, cx: &gpui::App) -> gpui::AnyElement {
    use gpui::{prelude::*, *};
    use ui::{ActiveTheme, Sizable, h_flex, input::NumberInput};

    let value = args.current_json.as_f64().unwrap_or(0.0) as f32;
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
        .child(h_flex().items_center().gap_2().child(
            if let Some(input) = args.get_widget::<gpui::Entity<ui::input::InputState>>() {
                NumberInput::new(&input)
                    .xsmall()
                    .w(gpui::px(92.0))
                    .into_any_element()
            } else {
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(format!("{:.3}", value))
                    .into_any_element()
            },
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_f32_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<f32>().unwrap();
        assert_eq!(info.type_name, "f32");
        assert_eq!(info.size, 4);
        assert_eq!(info.align, 4);
    }

    #[test]
    fn test_f32_serialization() {
        let value: f32 = std::f32::consts::PI;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_f64().unwrap(), value as f64);
    }

    #[test]
    fn test_f32_deserialization() {
        let expected = std::f32::consts::E;
        let json = serde_json::json!(expected);
        let mut deserializer = JsonDeserializer::new(json);
        let value = f32::deserialize(&mut deserializer).unwrap();
        assert!((value - expected).abs() < 0.00001);
    }

    #[test]
    fn test_f32_clone_any() {
        let value: f32 = 42.0;
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<f32>().unwrap(), 42.0);
    }
}
