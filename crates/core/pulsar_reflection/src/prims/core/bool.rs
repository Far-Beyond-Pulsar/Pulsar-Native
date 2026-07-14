//! bool primitive type implementation

use crate::pulsar_type;

#[cfg_attr(feature = "ui", pulsar_type(
    serialize_json_with = serialize_bool_json,
    deserialize_json_with = deserialize_bool_json,
    editor = render_bool_editor
))]
#[cfg_attr(not(feature = "ui"), pulsar_type(
    serialize_json_with = serialize_bool_json,
    deserialize_json_with = deserialize_bool_json
))]
#[allow(dead_code)]
type RegisteredBool = bool;

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

#[cfg(feature = "ui")]
fn render_bool_editor(args: &crate::PropertyEditorArgs<'_>, cx: &gpui::App) -> gpui::AnyElement {
    use gpui::{prelude::*, *};
    use ui::{ActiveTheme, Sizable, h_flex, switch::Switch};

    let value = args.current_json.as_bool().unwrap_or(false);
    let on_toggle = args.on_bool_toggle.clone();
    let id = format!(
        "bool-{}-{}-{}",
        args.id_prefix, args.class_name, args.prop_name
    );
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
            Switch::new(id)
                .checked(value)
                .small()
                .on_click(move |checked, window, cx| {
                    (on_toggle)(*checked, window, cx);
                }),
        )
        .into_any_element()
}

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
