//! String type implementation

use crate::pulsar_type;

fn serialize_string_json(value: &String) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(value))
}

fn deserialize_string_json(value: serde_json::Value) -> crate::ReflectResult<String> {
    value
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "String",
            found: format!("{:?}", value),
        })
}

#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_string_json,
        deserialize_json_with = deserialize_string_json,
        editor = string_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_string_json,
        deserialize_json_with = deserialize_string_json
    )
)]
#[allow(dead_code)]
type RegisteredString = String;

// ── Editor ────────────────────────────────────────────────────────────────────

/// Property editor for `String` — a single-line text input.
///
/// Owns its `InputState` child entity and the subscription that turns edits
/// into write-backs.
#[cfg(feature = "prims-gpui")]
pub struct StringEditor {
    label: String,
    input: gpui::Entity<ui::input::InputState>,
    value: String,
    write_back: crate::PropertyWriteBack,
    _subs: Vec<gpui::Subscription>,
}

#[cfg(feature = "prims-gpui")]
impl StringEditor {
    fn new(
        args: &crate::PropertyEditorArgs<'_>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        use gpui::AppContext as _;
        use ui::input::{InputEvent, InputState};

        let value = args
            .current_value
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();

        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(value.clone(), window, cx);
        });

        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::Change | InputEvent::Blur) {
                    return;
                }
                let text = input.read(cx).text().to_string();
                if this.value == text {
                    return;
                }
                this.value = text.clone();
                (this.write_back)(Box::new(text), window, cx);
            },
        )];

        Self {
            label: args.display_name.to_string(),
            input,
            value,
            write_back: args.write_back.clone(),
            _subs: subs,
        }
    }

    /// Accept a string that changed elsewhere.  The equality guard stops the
    /// framework's per-render push from echoing back through the subscription.
    fn set_value(&mut self, value: &str, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.value == value {
            return;
        }
        self.value = value.to_string();
        self.input.update(cx, |state, cx| {
            state.set_value(value.to_string(), window, cx);
        });
    }
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for StringEditor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::Styled as _;
        use ui::{Sizable, input::Input};

        crate::prims::editor_row(
            &self.label,
            Input::new(&self.input).xsmall().w(gpui::px(160.0)),
            cx,
        )
    }
}

#[cfg(feature = "prims-gpui")]
fn string_editor(
    args: &crate::PropertyEditorArgs<'_>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let entity = cx.new(|cx| StringEditor::new(args, window, cx));
    crate::BoundPropertyEditor::new(
        entity,
        |editor: &mut StringEditor, value: &String, window, cx| {
            editor.set_value(value, window, cx)
        },
    )
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_string_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<String>().unwrap();
        assert_eq!(info.type_name, "String");
        assert_eq!(info.size, std::mem::size_of::<String>());
        assert_eq!(info.align, std::mem::align_of::<String>());
    }

    #[test]
    fn test_string_serialization() {
        let value = "Hello, Pulsar!".to_string();
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_str().unwrap(), "Hello, Pulsar!");
    }

    #[test]
    fn test_string_deserialization() {
        let json = serde_json::json!("Test String");
        let mut deserializer = JsonDeserializer::new(json);
        let value = String::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, "Test String");
    }

    #[test]
    fn test_string_clone_any() {
        let value = "Cloneable".to_string();
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<String>().unwrap(), "Cloneable");
    }

    #[test]
    fn test_empty_string() {
        let value = String::new();
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_str().unwrap(), "");
    }

    #[test]
    fn test_unicode_string() {
        let value = "Hello 世界 🌍".to_string();
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_str().unwrap(), "Hello 世界 🌍");
    }
}
