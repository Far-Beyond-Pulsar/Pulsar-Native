//! bool primitive type implementation

use crate::pulsar_type;

// M3-alpha Task 2 (audit follow-up): `editor = bool_editor` registers a
// GPUI-typed property editor. `gpui-ce`/`ui` are optional deps of this crate
// behind `prims-gpui` (see Cargo.toml), so the `editor` argument — and the
// factory it names — must only be present when that feature is enabled.
#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_bool_json,
        deserialize_json_with = deserialize_bool_json,
        editor = bool_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_bool_json,
        deserialize_json_with = deserialize_bool_json
    )
)]
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

// ── Editor ────────────────────────────────────────────────────────────────────

/// Property editor for `bool` — a switch.
///
/// Needs no child entities: the switch is an element, so the click handler can
/// write back directly.
#[cfg(feature = "prims-gpui")]
pub struct BoolEditor {
    label: String,
    id: gpui::SharedString,
    value: bool,
    write_back: crate::PropertyWriteBack,
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for BoolEditor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use ui::{Sizable, switch::Switch};

        let write_back = self.write_back.clone();
        crate::prims::editor_row(
            &self.label,
            Switch::new(self.id.clone())
                .checked(self.value)
                .small()
                .on_click(move |checked, window, cx| {
                    (write_back)(Box::new(*checked), window, cx);
                }),
            cx,
        )
    }
}

#[cfg(feature = "prims-gpui")]
fn bool_editor(
    args: &crate::PropertyEditorArgs<'_>,
    _window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let label = args.display_name.to_string();
    let id: gpui::SharedString = format!(
        "bool-{}-{}-{}",
        args.id_prefix, args.class_name, args.prop_name
    )
    .into();
    let value = args.current_value.downcast_ref::<bool>().copied().unwrap_or(false);
    let write_back = args.write_back.clone();

    let entity = cx.new(|_| BoolEditor {
        label,
        id,
        value,
        write_back,
    });

    crate::BoundPropertyEditor::new(entity, |editor: &mut BoolEditor, value: &bool, _window, cx| {
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
