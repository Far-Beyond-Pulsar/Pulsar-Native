//! i32 primitive type implementation

use crate::pulsar_type;

// M3-alpha Task 2 (audit follow-up): see bool.rs — `editor` is GPUI-typed and
// only present behind `prims-gpui`.
#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_i32_json,
        deserialize_json_with = deserialize_i32_json,
        editor = i32_editor
    )
)]
#[cfg_attr(
    not(feature = "prims-gpui"),
    pulsar_type(
        serialize_json_with = serialize_i32_json,
        deserialize_json_with = deserialize_i32_json
    )
)]
type RegisteredI32 = i32;

fn serialize_i32_json(value: &i32) -> crate::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(*value))
}

fn deserialize_i32_json(value: serde_json::Value) -> crate::ReflectResult<i32> {
    value
        .as_i64()
        .map(|v| v as i32)
        .ok_or_else(|| crate::ReflectError::TypeMismatch {
            expected: "i32",
            found: format!("{:?}", value),
        })
}

// ── Editor ────────────────────────────────────────────────────────────────────

/// Property editor for `i32` — a stepping number input.
///
/// Owns its `InputState` child entity and both of the subscriptions that feed
/// it: text edits and increment/decrement clicks.
#[cfg(feature = "prims-gpui")]
pub struct I32Editor {
    label: String,
    input: gpui::Entity<ui::input::InputState>,
    value: i32,
    write_back: crate::PropertyWriteBack,
    _subs: Vec<gpui::Subscription>,
}

#[cfg(feature = "prims-gpui")]
impl I32Editor {
    fn new(
        args: &crate::PropertyEditorArgs<'_>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        use gpui::AppContext as _;
        use ui::input::{InputEvent, InputState, NumberInputEvent, StepAction};

        let value = args.current_value.downcast_ref::<i32>().copied().unwrap_or(0);
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(value.to_string(), window, cx);
        });

        let subs = vec![
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, input, event: &InputEvent, window, cx| {
                    if !matches!(event, InputEvent::Change | InputEvent::Blur) {
                        return;
                    }
                    let text = input.read(cx).text().to_string();
                    let Ok(parsed) = text.trim().parse::<i32>() else {
                        return;
                    };
                    this.commit(parsed, window, cx);
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _input, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step { action, .. } = event;
                    let stepped = match action {
                        StepAction::Increment => this.value.saturating_add(1),
                        StepAction::Decrement => this.value.saturating_sub(1),
                    };
                    this.value = stepped;
                    this.input.update(cx, |state, cx| {
                        state.set_value(stepped.to_string(), window, cx);
                    });
                    (this.write_back)(Box::new(stepped), window, cx);
                },
            ),
        ];

        Self {
            label: args.display_name.to_string(),
            input,
            value,
            write_back: args.write_back.clone(),
            _subs: subs,
        }
    }

    /// Record a value the user typed and push it back to the data source.
    ///
    /// Does not rewrite the input text — the user is mid-edit and owns it.
    fn commit(&mut self, value: i32, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.value == value {
            return;
        }
        self.value = value;
        (self.write_back)(Box::new(value), window, cx);
    }

    /// Accept a value that changed elsewhere.  The equality guard stops the
    /// framework's per-render push from echoing back through the subscription.
    fn set_value(&mut self, value: i32, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.value == value {
            return;
        }
        self.value = value;
        self.input.update(cx, |state, cx| {
            state.set_value(value.to_string(), window, cx);
        });
    }
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for I32Editor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::Styled as _;
        use ui::{Sizable, input::NumberInput};

        crate::prims::editor_row(
            &self.label,
            NumberInput::new(&self.input).xsmall().w(gpui::px(92.0)),
            cx,
        )
    }
}

#[cfg(feature = "prims-gpui")]
fn i32_editor(
    args: &crate::PropertyEditorArgs<'_>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let entity = cx.new(|cx| I32Editor::new(args, window, cx));
    crate::BoundPropertyEditor::new(
        entity,
        |editor: &mut I32Editor, value: &i32, window, cx| editor.set_value(*value, window, cx),
    )
}

#[cfg(test)]
mod tests {
    use crate::{JsonDeserializer, JsonSerializer, RUNTIME_TYPE_REGISTRY, Reflectable};

    #[test]
    fn test_i32_registered() {
        let info = RUNTIME_TYPE_REGISTRY.get::<i32>().unwrap();
        assert_eq!(info.type_name, "i32");
        assert_eq!(info.size, 4);
        assert_eq!(info.align, 4);
    }

    #[test]
    fn test_i32_serialization() {
        let value: i32 = -12345;
        let mut serializer = JsonSerializer::new();
        value.serialize(&mut serializer).unwrap();

        let json = serializer.as_json();
        assert_eq!(json.as_i64().unwrap(), value as i64);
    }

    #[test]
    fn test_i32_deserialization() {
        let json = serde_json::json!(42);
        let mut deserializer = JsonDeserializer::new(json);
        let value = i32::deserialize(&mut deserializer).unwrap();
        assert_eq!(value, 42);
    }

    #[test]
    fn test_i32_clone_any() {
        let value: i32 = -999;
        let boxed = value.clone_any();
        assert_eq!(*boxed.downcast::<i32>().unwrap(), -999);
    }
}
