//! u64 primitive type implementation

use crate::pulsar_type;

#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_u64_json,
        deserialize_json_with = deserialize_u64_json,
        editor = u64_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_u64_json,
        deserialize_json_with = deserialize_u64_json
    )
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

// ── Editor ────────────────────────────────────────────────────────────────────

/// Property editor for `u64`.
///
/// Renders a variant dropdown when the registered type info describes an enum
/// (enums are stored as their discriminant), and a read-only number otherwise.
/// Needs no child entities — both forms are plain elements.
#[cfg(feature = "prims-gpui")]
pub struct U64Editor {
    label: String,
    id: gpui::SharedString,
    value: u64,
    variants: Option<&'static [&'static str]>,
    write_back: crate::PropertyWriteBack,
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for U64Editor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        use ui::{ActiveTheme, Sizable, button::Button, menu::PopupMenuItem};

        let Some(variants) = self.variants else {
            return crate::prims::editor_row(
                &self.label,
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(self.value.to_string()),
                cx,
            );
        };

        let selected_ix = (self.value as usize).min(variants.len().saturating_sub(1));
        let label = variants.get(selected_ix).copied().unwrap_or("Select");
        let write_back = self.write_back.clone();

        crate::prims::editor_row(
            &self.label,
            Button::new(self.id.clone())
                .label(label)
                .xsmall()
                .outline()
                .dropdown_caret(true)
                .dropdown_menu_with_anchor(gpui::Corner::BottomRight, move |menu, _window, _cx| {
                    let mut menu = menu;
                    for (ix, option) in variants.iter().enumerate() {
                        let write_back = write_back.clone();
                        menu = menu.item(
                            PopupMenuItem::new(option.to_string())
                                .checked(ix == selected_ix)
                                .on_click(move |_event, window, cx| {
                                    (write_back)(Box::new(ix as u64), window, cx);
                                }),
                        );
                    }
                    menu
                }),
            cx,
        )
    }
}

#[cfg(feature = "prims-gpui")]
fn u64_editor(
    args: &crate::PropertyEditorArgs<'_>,
    _window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let label = args.display_name.to_string();
    let id: gpui::SharedString = format!(
        "u64-{}-{}-{}",
        args.id_prefix, args.class_name, args.prop_name
    )
    .into();
    let value = args.current_value.downcast_ref::<u64>().copied().unwrap_or(0);
    let variants = args.type_info.enum_variants();
    let write_back = args.write_back.clone();

    let entity = cx.new(|_| U64Editor {
        label,
        id,
        value,
        variants,
        write_back,
    });

    crate::BoundPropertyEditor::new(entity, |editor: &mut U64Editor, value: &u64, _window, cx| {
        if editor.value != *value {
            editor.value = *value;
            cx.notify();
        }
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
