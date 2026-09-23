//! u32 primitive type implementation

use crate::pulsar_type;

#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_u32_json,
        deserialize_json_with = deserialize_u32_json,
        editor = u32_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_u32_json,
        deserialize_json_with = deserialize_u32_json
    )
)]
#[allow(dead_code)]
type RegisteredU32 = u32;

fn serialize_u32_json(value: &u32) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(*value))
}

fn deserialize_u32_json(value: serde_json::Value) -> crate::ReflectResult<u32> {
    value
        .as_u64()
        .map(|value| value as u32)
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "u32",
            found: format!("{:?}", value),
        })
}

// ── Editor ────────────────────────────────────────────────────────────────────

/// Property editor for `u32`.
///
/// Renders a variant dropdown when the registered type info describes an enum
/// (enums are stored as their discriminant), and a read-only number otherwise.
/// Needs no child entities — both forms are plain elements.
#[cfg(feature = "prims-gpui")]
pub struct U32Editor {
    label: String,
    id: gpui::SharedString,
    value: u32,
    variants: Option<&'static [&'static str]>,
    write_back: crate::PropertyWriteBack,
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for U32Editor {
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
                                    (write_back)(Box::new(ix as u32), window, cx);
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
fn u32_editor(
    args: &crate::PropertyEditorArgs<'_>,
    _window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let label = args.display_name.to_string();
    let id: gpui::SharedString = format!(
        "u32-{}-{}-{}",
        args.id_prefix, args.class_name, args.prop_name
    )
    .into();
    let value = args.current_value.downcast_ref::<u32>().copied().unwrap_or(0);
    let variants = args.type_info.enum_variants();
    let write_back = args.write_back.clone();

    let entity = cx.new(|_| U32Editor {
        label,
        id,
        value,
        variants,
        write_back,
    });

    crate::BoundPropertyEditor::new(entity, |editor: &mut U32Editor, value: &u32, _window, cx| {
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
    fn test_u32_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<u32>().unwrap();
        assert_eq!(info.type_name, "u32");
        assert_eq!(info.size, std::mem::size_of::<u32>());
        assert_eq!(info.align, std::mem::align_of::<u32>());
    }

    #[test]
    fn test_u32_serialization() {
        let value: u32 = u32::MAX;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_u64(), Some(u64::from(value)));
    }

    #[test]
    fn test_u32_deserialization() {
        let json = serde_json::json!(999999);
        let mut deserializer = JsonDeserializer::new(json);
        let value = u32::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, 999999);
    }

    #[test]
    fn test_u32_clone_any() {
        let value: u32 = 12345678;
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<u32>().unwrap(), 12345678);
    }
}

