//! f32 primitive type implementation

use crate::pulsar_type;

// M3-alpha Task 2 (audit follow-up): see bool.rs — `editor` is GPUI-typed and
// only present behind `prims-gpui`.
#[cfg_attr(
    feature = "prims-gpui",
    pulsar_type(
        serialize_json_with = serialize_f32_json,
        deserialize_json_with = deserialize_f32_json,
        editor = f32_editor
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

// ── Editor ────────────────────────────────────────────────────────────────────

/// Step applied per increment/decrement click on the number input.
#[cfg(feature = "prims-gpui")]
const STEP: f32 = 1.0;

/// Property editor for `f32` — a stepping number input.
///
/// Owns its `InputState` child entity and both of the subscriptions that feed
/// it: text edits and increment/decrement clicks.
#[cfg(feature = "prims-gpui")]
pub struct F32Editor {
    label: String,
    input: gpui::Entity<ui::input::InputState>,
    value: f32,
    write_back: crate::PropertyWriteBack,
    _subs: Vec<gpui::Subscription>,
}

#[cfg(feature = "prims-gpui")]
impl F32Editor {
    fn new(
        args: &crate::PropertyEditorArgs<'_>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        use gpui::AppContext as _;
        use ui::input::{InputEvent, InputState, NumberInputEvent, StepAction};

        let value = args.current_value.downcast_ref::<f32>().copied().unwrap_or(0.0);
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(format_f32(value), window, cx);
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
                    let Ok(parsed) = text.trim().parse::<f32>() else {
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
                        StepAction::Increment => this.value + STEP,
                        StepAction::Decrement => this.value - STEP,
                    };
                    this.value = stepped;
                    this.input.update(cx, |state, cx| {
                        state.set_value(format_f32(stepped), window, cx);
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
    fn commit(&mut self, value: f32, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.value == value {
            return;
        }
        self.value = value;
        (self.write_back)(Box::new(value), window, cx);
    }

    /// Accept a value that changed elsewhere (undo, another panel, a gizmo).
    ///
    /// The equality guard is what stops the framework's per-render push from
    /// echoing back through the `InputEvent::Change` subscription above.
    fn set_value(&mut self, value: f32, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        if self.value == value {
            return;
        }
        self.value = value;
        self.input.update(cx, |state, cx| {
            state.set_value(format_f32(value), window, cx);
        });
    }
}

#[cfg(feature = "prims-gpui")]
fn format_f32(value: f32) -> String {
    format!("{:.3}", value)
}

#[cfg(feature = "prims-gpui")]
impl gpui::Render for F32Editor {
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
fn f32_editor(
    args: &crate::PropertyEditorArgs<'_>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> crate::BoundPropertyEditor {
    use gpui::AppContext as _;

    let entity = cx.new(|cx| F32Editor::new(args, window, cx));
    crate::BoundPropertyEditor::new(
        entity,
        |editor: &mut F32Editor, value: &f32, window, cx| editor.set_value(*value, window, cx),
    )
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
